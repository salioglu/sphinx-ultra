#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

use flate2::read::ZlibDecoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Warnings,
    SearchIndex,
    NeedsJson,
    ObjectsInventory,
    TextCrlf,
    ExactBytes,
}

#[derive(Debug, Clone, Copy)]
pub struct WarningRoots<'a> {
    pub source_root: Option<&'a Path>,
    pub output_root: Option<&'a Path>,
    pub doctree_root: Option<&'a Path>,
    pub case_root: Option<&'a Path>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub category: String,
    pub logical_path: String,
    pub first_expected_line: Option<usize>,
    pub expected: String,
    pub actual: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaseResult {
    pub profile: String,
    pub source_set: String,
    pub case_id: String,
    pub html_status: String,
    pub exception_type: Option<String>,
    pub needs_status: Option<String>,
    pub passed: bool,
    pub run_dir: String,
    pub rerun_filter: String,
    pub common_mismatch_count: usize,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InventoryRecord {
    pub name: String,
    pub domain_role: String,
    pub priority: i32,
    pub uri: String,
    pub display_name: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct IndexDocument {
    pub schema_version: u32,
    pub generator: String,
    pub profiles: BTreeMap<String, ProfileRecord>,
    pub cases: Vec<CaseRecord>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ProfileRecord {
    pub sphinx: String,
    pub docutils: String,
    pub platform: String,
    pub needs_version: Option<String>,
    pub needs_commit: Option<String>,
    pub needs_tree: Option<String>,
    pub lock_path: String,
    pub lock_sha256: String,
    pub determinism_shims: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct OriginRecord {
    pub source_set: String,
    pub origin_path: String,
    pub pytest_node_ids: Vec<String>,
    pub variants_not_captured: bool,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FileStorage {
    Input,
    Ref,
    Blob,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct FileRecord {
    pub logical_path: String,
    pub storage: FileStorage,
    pub storage_path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CaseStatus {
    Built,
    BuildError,
    ReferenceCrash,
    ExcludedNetwork,
    ExcludedPlantuml,
}

impl CaseStatus {
    pub fn is_excluded(self) -> bool {
        matches!(self, Self::ExcludedNetwork | Self::ExcludedPlantuml)
    }

    pub fn is_runnable(self) -> bool {
        matches!(self, Self::Built | Self::BuildError)
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CaseRecord {
    pub profile: String,
    pub source_set: String,
    pub case_id: String,
    pub status: CaseStatus,
    pub exception_type: Option<String>,
    pub exit_code: Option<i32>,
    pub warnings: String,
    pub excluded_reason: Option<String>,
    pub origin: OriginRecord,
    pub input_files: Vec<FileRecord>,
    pub input_sha256: String,
    pub tree_sha256: String,
    pub files: Vec<FileRecord>,
    pub needs_json: Option<FileRecord>,
    pub needs_status: Option<CaseStatus>,
    pub needs_exit_code: Option<i32>,
    pub needs_warnings: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceCase {
    pub profile: String,
    pub source_set: String,
    pub case_id: String,
    pub status: String,
    pub exception_type: Option<String>,
}

impl IndexDocument {
    pub fn from_value(value: Value) -> Result<Self, String> {
        let document: Self = serde_json::from_value(value).map_err(|error| error.to_string())?;
        document.validate_shape()?;
        Ok(document)
    }

    pub fn load(index_path: &Path) -> Result<Self, String> {
        let bytes = fs::read(index_path)
            .map_err(|error| format!("read {}: {error}", index_path.display()))?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse {}: {error}", index_path.display()))?;
        let document = Self::from_value(value)?;
        let profile_root = index_path
            .parent()
            .ok_or_else(|| format!("index has no parent: {}", index_path.display()))?;
        document.validate_files(profile_root)?;
        Ok(document)
    }

    pub fn validate_shape(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "unsupported schema_version {}",
                self.schema_version
            ));
        }
        if self.generator != "html-oracle/1" {
            return Err(format!("unexpected generator {:?}", self.generator));
        }
        for profile_name in self.profiles.keys() {
            if !matches!(profile_name.as_str(), "core" | "local_needs") {
                return Err(format!("unknown profile {profile_name:?}"));
            }
        }
        for (profile_name, profile) in &self.profiles {
            validate_sha256(
                &profile.lock_sha256,
                &format!("profile {profile_name} lock_sha256"),
            )?;
            validate_relative_path(&profile.lock_path, "lock_path")?;
        }

        let mut previous_key: Option<(&str, &str, &str)> = None;
        let mut keys = BTreeSet::new();
        for case in &self.cases {
            let key = (
                case.profile.as_str(),
                case.source_set.as_str(),
                case.case_id.as_str(),
            );
            if !keys.insert(key) {
                return Err(format!("duplicate case key {}/{}/{}", key.0, key.1, key.2));
            }
            if let Some(previous) = previous_key {
                if key < previous {
                    return Err(format!(
                        "cases are not sorted: {}/{}/{} follows {}/{}/{}",
                        key.0, key.1, key.2, previous.0, previous.1, previous.2
                    ));
                }
            }
            previous_key = Some(key);

            self.profiles
                .get(&case.profile)
                .ok_or_else(|| format!("case references unknown profile {:?}", case.profile))?;
            if case.origin.source_set != case.source_set {
                return Err(format!(
                    "origin source_set {:?} does not match case source_set {:?}",
                    case.origin.source_set, case.source_set
                ));
            }
            validate_case(case, profile_name_is_local_needs(&case.profile))?;
        }
        Ok(())
    }

    pub fn validate_files(&self, profile_root: &Path) -> Result<(), String> {
        let mut expected = BTreeSet::new();
        for case in &self.cases {
            let input_entries = case
                .input_files
                .iter()
                .map(|record| self.validate_file_record(profile_root, record, FileStorage::Input))
                .collect::<Result<Vec<_>, _>>()?;
            let output_entries = case
                .files
                .iter()
                .map(|record| self.validate_file_record(profile_root, record, record.storage))
                .collect::<Result<Vec<_>, _>>()?;
            if case.status.is_excluded() {
                continue;
            }
            let input_hash = canonical_hash(&input_entries);
            if input_hash != case.input_sha256 {
                return Err(format!(
                    "input_sha256 mismatch for {}/{}/{}: expected {}, got {}",
                    case.profile, case.source_set, case.case_id, case.input_sha256, input_hash
                ));
            }
            let tree_hash = canonical_hash(&output_entries);
            if tree_hash != case.tree_sha256 {
                return Err(format!(
                    "tree_sha256 mismatch for {}/{}/{}: expected {}, got {}",
                    case.profile, case.source_set, case.case_id, case.tree_sha256, tree_hash
                ));
            }
            for record in case.input_files.iter().chain(case.files.iter()) {
                expected.insert(record.storage_path.clone());
            }
            if let Some(record) = &case.needs_json {
                self.validate_file_record(profile_root, record, FileStorage::Ref)?;
                expected.insert(record.storage_path.clone());
            }
        }
        validate_reverse_artifacts(profile_root, &expected)
    }

    fn validate_file_record(
        &self,
        profile_root: &Path,
        record: &FileRecord,
        expected_storage: FileStorage,
    ) -> Result<(String, String), String> {
        if record.storage != expected_storage {
            return Err(format!(
                "storage kind mismatch for {}: expected {:?}, got {:?}",
                record.logical_path, expected_storage, record.storage
            ));
        }
        validate_relative_path(&record.logical_path, "logical_path")?;
        validate_relative_path(&record.storage_path, "storage_path")?;
        validate_sha256(&record.sha256, &format!("{} sha256", record.logical_path))?;
        let expected_prefix = match record.storage {
            FileStorage::Input => "inputs/",
            FileStorage::Ref => "refs/",
            FileStorage::Blob => "blobs/",
        };
        if !record.storage_path.starts_with(expected_prefix) {
            return Err(format!(
                "{} storage path must start with {expected_prefix:?}",
                record.logical_path
            ));
        }
        let path = profile_root.join(
            record
                .storage_path
                .replace('/', std::path::MAIN_SEPARATOR_STR),
        );
        reject_symlink_components(profile_root, &path)?;
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("read metadata {}: {error}", path.display()))?;
        if !metadata.is_file() {
            return Err(format!("{} is not a regular file", path.display()));
        }
        if metadata.len() != record.size {
            return Err(format!(
                "size mismatch for {}: ledger {}, file {}",
                record.logical_path,
                record.size,
                metadata.len()
            ));
        }
        let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        let actual_hash = sha256_hex(&bytes);
        if actual_hash != record.sha256 {
            return Err(format!(
                "hash mismatch for {}: ledger {}, file {}",
                record.logical_path, record.sha256, actual_hash
            ));
        }
        Ok((record.logical_path.clone(), actual_hash))
    }
}

fn validate_case(case: &CaseRecord, local_needs: bool) -> Result<(), String> {
    validate_sha256(&case.input_sha256, "input_sha256")?;
    validate_sha256(&case.tree_sha256, "tree_sha256")?;
    validate_file_records_sorted(&case.input_files, "input_files")?;
    validate_file_records_sorted(&case.files, "files")?;
    if case.origin.origin_path.is_empty() {
        return Err(format!("empty origin_path for {}", case.case_id));
    }
    validate_status_fields(case)?;

    if !local_needs
        && (case.needs_json.is_some()
            || case.needs_status.is_some()
            || case.needs_exit_code.is_some()
            || case.needs_warnings.is_some())
    {
        return Err(format!("core case {} has local-needs fields", case.case_id));
    }
    if local_needs
        && case.status.is_runnable()
        && (case.needs_status.is_none()
            || case.needs_exit_code.is_none()
            || case.needs_warnings.is_none())
    {
        return Err(format!(
            "runnable local-needs case {} has incomplete second-build fields",
            case.case_id
        ));
    }
    if let Some(needs_status) = case.needs_status {
        if !local_needs || !case.status.is_runnable() || needs_status.is_excluded() {
            return Err(format!(
                "needs status is not valid for case {}",
                case.case_id
            ));
        }
        match needs_status {
            CaseStatus::Built if case.needs_exit_code != Some(0) => {
                return Err(format!(
                    "built needs run for {} must have exit_code 0",
                    case.case_id
                ));
            }
            CaseStatus::BuildError
                if case.needs_exit_code.is_none() || case.needs_exit_code == Some(0) =>
            {
                return Err(format!(
                    "build-error needs run for {} must have a nonzero exit_code",
                    case.case_id
                ));
            }
            _ => {}
        }
    } else if case.needs_json.is_some()
        || case.needs_exit_code.is_some()
        || case.needs_warnings.is_some()
    {
        return Err(format!(
            "case {} has needs fields without needs_status",
            case.case_id
        ));
    }
    if let Some(record) = &case.needs_json {
        if record.storage != FileStorage::Ref {
            return Err(format!(
                "needs_json for {} must use ref storage",
                case.case_id
            ));
        }
        validate_relative_path(&record.logical_path, "needs_json.logical_path")?;
        validate_relative_path(&record.storage_path, "needs_json.storage_path")?;
        if !record.logical_path.ends_with("needs.json") {
            return Err(format!(
                "needs_json for {} has unexpected logical path",
                case.case_id
            ));
        }
    }
    if !local_needs && !case.input_files.is_empty() {
        // Core inputs are still valid; this branch documents that only the
        // four needs fields, not input materialization, are profile-specific.
    }
    Ok(())
}

fn validate_status_fields(case: &CaseRecord) -> Result<(), String> {
    match case.status {
        CaseStatus::Built => {
            if case.exit_code != Some(0) {
                return Err(format!("built case {} must have exit_code 0", case.case_id));
            }
            if case.exception_type.is_some() {
                return Err(format!(
                    "built case {} must have a null exception_type",
                    case.case_id
                ));
            }
            if case.excluded_reason.is_some() {
                return Err(format!(
                    "built case {} has an excluded reason",
                    case.case_id
                ));
            }
            Ok(())
        }
        CaseStatus::BuildError => {
            if case.exit_code.is_none() || case.exit_code == Some(0) {
                return Err(format!(
                    "build-error case {} must have a nonzero exit_code",
                    case.case_id
                ));
            }
            if case.exception_type.as_deref() == Some("") {
                return Err(format!(
                    "build-error case {} has an empty exception_type",
                    case.case_id
                ));
            }
            if case.excluded_reason.is_some() {
                return Err(format!(
                    "build-error case {} has an excluded reason",
                    case.case_id
                ));
            }
            Ok(())
        }
        CaseStatus::ReferenceCrash => {
            if case.exit_code.is_none() || case.exit_code == Some(0) {
                return Err(format!(
                    "reference-crash case {} must have a nonzero exit_code",
                    case.case_id
                ));
            }
            if case.exception_type.as_deref().is_none_or(str::is_empty) {
                return Err(format!(
                    "reference-crash case {} must have a nonempty exception_type",
                    case.case_id
                ));
            }
            if case.excluded_reason.is_some() {
                return Err(format!(
                    "reference-crash case {} has an excluded reason",
                    case.case_id
                ));
            }
            Ok(())
        }
        status if status.is_excluded() => {
            if case.exit_code.is_some() {
                return Err(format!("excluded case {} has an exit_code", case.case_id));
            }
            if case.exception_type.is_some() {
                return Err(format!(
                    "excluded case {} must have a null exception_type",
                    case.case_id
                ));
            }
            if case.excluded_reason.is_none() {
                return Err(format!("excluded case {} has no reason", case.case_id));
            }
            if !case.warnings.is_empty() || !case.input_files.is_empty() || !case.files.is_empty() {
                return Err(format!(
                    "excluded case {} has captured artifacts",
                    case.case_id
                ));
            }
            Ok(())
        }
        _ => {
            if case.status.is_runnable() && case.excluded_reason.is_some() {
                return Err(format!(
                    "runnable case {} has an excluded reason",
                    case.case_id
                ));
            }
            Ok(())
        }
    }
}

fn validate_file_records_sorted(records: &[FileRecord], field: &str) -> Result<(), String> {
    let mut previous: Option<&str> = None;
    let mut logical_paths = BTreeSet::new();
    for record in records {
        validate_relative_path(&record.logical_path, &format!("{field}.logical_path"))?;
        validate_relative_path(&record.storage_path, &format!("{field}.storage_path"))?;
        if !logical_paths.insert(record.logical_path.as_str()) {
            return Err(format!(
                "duplicate logical path in {field}: {}",
                record.logical_path
            ));
        }
        if let Some(previous) = previous {
            if record.logical_path.as_str() < previous {
                return Err(format!("{field} are not sorted"));
            }
        }
        previous = Some(&record.logical_path);
    }
    Ok(())
}

fn validate_relative_path(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\\')
        || (value.len() >= 2 && value.as_bytes()[1] == b':')
    {
        return Err(format!("{field} is not a relative slash path: {value:?}"));
    }
    let path = Path::new(value);
    for component in path.components() {
        match component {
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(format!("{field} contains an escape: {value:?}"));
            }
            Component::Normal(_) => {}
        }
    }
    if value.split('/').any(|component| component.is_empty()) {
        return Err(format!("{field} contains an empty component: {value:?}"));
    }
    Ok(())
}

fn validate_sha256(value: &str, field: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{field} is not a SHA-256 hex digest: {value:?}"));
    }
    Ok(())
}

fn profile_name_is_local_needs(profile: &str) -> bool {
    profile == "local_needs"
}

fn reject_symlink_components(root: &Path, path: &Path) -> Result<(), String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("path escapes root: {}", path.display()))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if let Component::Normal(part) = component {
            current.push(part);
            if fs::symlink_metadata(&current)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
            {
                return Err(format!("symlink in captured path: {}", current.display()));
            }
        }
    }
    Ok(())
}

fn validate_reverse_artifacts(
    profile_root: &Path,
    expected: &BTreeSet<String>,
) -> Result<(), String> {
    for directory in ["inputs", "refs", "blobs"] {
        let root = profile_root.join(directory);
        if !root.exists() {
            continue;
        }
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            for entry in fs::read_dir(&current)
                .map_err(|error| format!("walk {}: {error}", current.display()))?
            {
                let entry = entry.map_err(|error| format!("read directory entry: {error}"))?;
                let path = entry.path();
                if entry
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_dir()
                {
                    stack.push(path);
                } else if entry
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_file()
                {
                    let relative = path
                        .strip_prefix(profile_root)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/");
                    if relative.ends_with("/warnings.txt") {
                        continue;
                    }
                    if !expected.contains(&relative) {
                        return Err(format!("unreferenced artifact {relative}"));
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn canonical_hash(entries: &[(String, String)]) -> String {
    let mut sorted = entries.to_vec();
    sorted.sort();
    let mut payload = String::new();
    for (path, digest) in sorted {
        payload.push_str(&path);
        payload.push('\0');
        payload.push_str(&digest);
        payload.push('\n');
    }
    sha256_hex(payload.as_bytes())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn profile_root(fixtures_root: &Path, profile: &str) -> PathBuf {
    fixtures_root.join(profile)
}

pub fn policy_for_path(logical_path: &str) -> Policy {
    if logical_path == "searchindex.js" {
        Policy::SearchIndex
    } else if logical_path.rsplit('/').next() == Some("needs.json") {
        Policy::NeedsJson
    } else if logical_path == "objects.inv" {
        Policy::ObjectsInventory
    } else if logical_path.ends_with(".buildinfo") {
        Policy::ExactBytes
    } else if logical_path.starts_with("_sources/")
        || [".html", ".css", ".js", ".json", ".xml", ".txt"]
            .iter()
            .any(|suffix| logical_path.ends_with(suffix))
    {
        Policy::TextCrlf
    } else {
        Policy::ExactBytes
    }
}

pub fn compare_file(
    logical_path: &str,
    expected: &[u8],
    actual: &[u8],
    source_root: Option<&Path>,
) -> Vec<Diagnostic> {
    match policy_for_path(logical_path) {
        Policy::SearchIndex => compare_json_file(
            logical_path,
            expected,
            actual,
            "searchindex-value",
            "invalid-searchindex",
            true,
            source_root,
        ),
        Policy::NeedsJson => compare_json_file(
            logical_path,
            expected,
            actual,
            "needs-json-value",
            "invalid-needs-json",
            false,
            None,
        ),
        Policy::ObjectsInventory => compare_inventory_file(logical_path, expected, actual),
        Policy::TextCrlf => {
            let expected = normalize_crlf(expected);
            let actual = replace_source_root_token(&normalize_crlf(actual), source_root, true);
            if expected == actual {
                Vec::new()
            } else {
                vec![value_diagnostic(
                    "text-value",
                    logical_path,
                    &expected,
                    &actual,
                )]
            }
        }
        Policy::Warnings | Policy::ExactBytes => {
            if expected == actual {
                Vec::new()
            } else {
                vec![value_diagnostic(
                    "bytes-value",
                    logical_path,
                    expected,
                    actual,
                )]
            }
        }
    }
}

pub fn compare_warnings(
    expected: &[u8],
    actual: &[u8],
    expected_source_root: Option<&Path>,
    actual_source_root: Option<&Path>,
) -> Vec<Diagnostic> {
    compare_warnings_with_roots(
        expected,
        actual,
        expected_source_root.map(|source_root| WarningRoots {
            source_root: Some(source_root),
            output_root: None,
            doctree_root: None,
            case_root: None,
        }),
        actual_source_root.map(|source_root| WarningRoots {
            source_root: Some(source_root),
            output_root: None,
            doctree_root: None,
            case_root: None,
        }),
    )
}

pub fn compare_warnings_with_roots(
    expected: &[u8],
    actual: &[u8],
    expected_roots: Option<WarningRoots<'_>>,
    actual_roots: Option<WarningRoots<'_>>,
) -> Vec<Diagnostic> {
    let _policy = Policy::Warnings;
    let expected = normalize_warning_bytes_with_roots(expected, expected_roots);
    let actual = normalize_warning_bytes_with_roots(actual, actual_roots);
    if expected == actual {
        Vec::new()
    } else {
        vec![value_diagnostic(
            "warning",
            "warnings",
            expected.as_bytes(),
            actual.as_bytes(),
        )]
    }
}

pub fn compare_trees(
    expected: &BTreeMap<String, Vec<u8>>,
    actual: &BTreeMap<String, Vec<u8>>,
    expected_warnings: Option<&[u8]>,
    actual_warnings: Option<&[u8]>,
    expected_status: Option<CaseStatus>,
    actual_status: Option<CaseStatus>,
) -> Vec<Diagnostic> {
    let mut paths = BTreeSet::new();
    paths.extend(expected.keys().cloned());
    paths.extend(actual.keys().cloned());
    let mut diagnostics = Vec::new();
    for path in paths {
        match (expected.get(&path), actual.get(&path)) {
            (None, Some(actual)) => diagnostics.push(Diagnostic {
                category: "unexpected-file".to_string(),
                logical_path: path,
                first_expected_line: None,
                expected: String::new(),
                actual: display_bytes(actual),
                detail: "file exists only in actual output".to_string(),
            }),
            (Some(expected), None) => diagnostics.push(Diagnostic {
                category: "missing-file".to_string(),
                logical_path: path,
                first_expected_line: Some(1),
                expected: display_bytes(expected),
                actual: String::new(),
                detail: "file exists only in expected output".to_string(),
            }),
            (Some(expected), Some(actual)) => {
                diagnostics.extend(compare_file(&path, expected, actual, None));
            }
            (None, None) => unreachable!(),
        }
    }
    if let (Some(expected), Some(actual)) = (expected_warnings, actual_warnings) {
        diagnostics.extend(compare_warnings(expected, actual, None, None));
    }
    if let (Some(expected), Some(actual)) = (expected_status, actual_status) {
        let expected_is_error = expected == CaseStatus::BuildError;
        let actual_is_error = actual == CaseStatus::BuildError;
        if expected_is_error != actual_is_error {
            diagnostics.push(Diagnostic {
                category: "status".to_string(),
                logical_path: String::new(),
                first_expected_line: None,
                expected: status_name(expected).to_string(),
                actual: status_name(actual).to_string(),
                detail: "build status class differs".to_string(),
            });
        }
    }
    diagnostics.sort_by(diagnostic_order);
    diagnostics
}

fn compare_json_file(
    logical_path: &str,
    expected: &[u8],
    actual: &[u8],
    value_category: &str,
    invalid_category: &str,
    searchindex_wrapper: bool,
    actual_source_root: Option<&Path>,
) -> Vec<Diagnostic> {
    let expected_value = match parse_json_value(expected, searchindex_wrapper) {
        Ok(value) => value,
        Err(error) => {
            return vec![invalid_diagnostic(
                invalid_category,
                logical_path,
                format!("expected JSON is invalid: {error}"),
            )]
        }
    };
    let actual = if searchindex_wrapper {
        replace_source_root_token(actual, actual_source_root, true)
    } else {
        actual.to_vec()
    };
    let actual_value = match parse_json_value(&actual, searchindex_wrapper) {
        Ok(value) => value,
        Err(error) => {
            return vec![invalid_diagnostic(
                invalid_category,
                logical_path,
                format!("actual JSON is invalid: {error}"),
            )]
        }
    };
    if expected_value == actual_value {
        Vec::new()
    } else {
        vec![value_diagnostic(
            value_category,
            logical_path,
            serde_json::to_string_pretty(&expected_value)
                .unwrap_or_default()
                .as_bytes(),
            serde_json::to_string_pretty(&actual_value)
                .unwrap_or_default()
                .as_bytes(),
        )]
    }
}

pub fn parse_json_value(bytes: &[u8], searchindex_wrapper: bool) -> Result<Value, String> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => normalize_crlf(text.as_bytes()),
        Err(error) => {
            return Err(format!("not UTF-8: {error}"));
        }
    };
    let text = String::from_utf8(text).map_err(|error| error.to_string())?;
    let payload = if searchindex_wrapper {
        let trimmed = text.trim_end();
        let prefix = "Search.setIndex(";
        if !trimmed.starts_with(prefix) || !trimmed.ends_with(");") {
            return Err("missing Search.setIndex(...) wrapper".to_string());
        }
        &trimmed[prefix.len()..trimmed.len() - 2]
    } else {
        text.as_str()
    };
    serde_json::from_str(payload).map_err(|error| error.to_string())
}

fn compare_inventory_file(logical_path: &str, expected: &[u8], actual: &[u8]) -> Vec<Diagnostic> {
    let expected_inventory = match parse_inventory(expected) {
        Ok(value) => value,
        Err(error) => {
            return vec![invalid_diagnostic(
                "invalid-objects-inventory",
                logical_path,
                format!("expected inventory is invalid: {error}"),
            )]
        }
    };
    let actual_inventory = match parse_inventory(actual) {
        Ok(value) => value,
        Err(error) => {
            return vec![invalid_diagnostic(
                "invalid-objects-inventory",
                logical_path,
                format!("actual inventory is invalid: {error}"),
            )]
        }
    };
    if expected_inventory == actual_inventory {
        Vec::new()
    } else {
        vec![value_diagnostic(
            "objects-inventory-value",
            logical_path,
            format_inventory(&expected_inventory).as_bytes(),
            format_inventory(&actual_inventory).as_bytes(),
        )]
    }
}

pub fn parse_inventory(bytes: &[u8]) -> Result<(Vec<u8>, Vec<InventoryRecord>), String> {
    const HEADER: &[u8] = b"# Sphinx inventory version 2\n# Project: ";
    if !bytes.starts_with(HEADER) {
        return Err("missing Sphinx inventory header".to_string());
    }
    let mut newline_positions = Vec::new();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            newline_positions.push(index + 1);
            if newline_positions.len() == 4 {
                break;
            }
        }
    }
    if newline_positions.len() != 4 {
        return Err("inventory has fewer than four header lines".to_string());
    }
    let header_end = newline_positions[3];
    let header = bytes[..header_end].to_vec();
    let mut decoder = ZlibDecoder::new(Cursor::new(&bytes[header_end..]));
    let mut records_text = String::new();
    decoder
        .read_to_string(&mut records_text)
        .map_err(|error| format!("zlib decode failed: {error}"))?;
    let mut records = Vec::new();
    for (line_number, line) in records_text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let fields = line.splitn(5, ' ').collect::<Vec<_>>();
        if fields.len() != 5 || fields.iter().any(|field| field.is_empty()) {
            return Err(format!("invalid record on line {}", line_number + 1));
        }
        let priority = fields[2]
            .parse::<i32>()
            .map_err(|error| format!("invalid priority on line {}: {error}", line_number + 1))?;
        records.push(InventoryRecord {
            name: fields[0].to_string(),
            domain_role: fields[1].to_string(),
            priority,
            uri: fields[3].to_string(),
            display_name: fields[4].to_string(),
        });
    }
    records.sort();
    Ok((header, records))
}

fn format_inventory(inventory: &(Vec<u8>, Vec<InventoryRecord>)) -> String {
    let header = String::from_utf8_lossy(&inventory.0);
    let records = inventory
        .1
        .iter()
        .map(|record| {
            format!(
                "{} {} {} {} {}",
                record.name, record.domain_role, record.priority, record.uri, record.display_name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{header}{records}")
}

fn normalize_warning_bytes(bytes: &[u8], source_root: Option<&Path>) -> String {
    normalize_warning_bytes_with_roots(
        bytes,
        source_root.map(|source_root| WarningRoots {
            source_root: Some(source_root),
            output_root: None,
            doctree_root: None,
            case_root: None,
        }),
    )
}

fn normalize_warning_bytes_with_roots(bytes: &[u8], roots: Option<WarningRoots<'_>>) -> String {
    let mut normalized = normalize_crlf(bytes);
    if let Some(roots) = roots {
        let mut replacements = [
            (roots.source_root, b"<SRCDIR>".as_slice()),
            (roots.output_root, b"<OUTDIR>".as_slice()),
            (roots.doctree_root, b"<DOCTREEDIR>".as_slice()),
            (roots.case_root, b"<CASEDIR>".as_slice()),
        ];
        replacements
            .sort_by_key(|(root, _)| root.map(|root| root.as_os_str().len()).unwrap_or_default());
        for (root, token) in replacements.into_iter().rev() {
            if let Some(root) = root {
                normalized = replace_path_token(&normalized, root, token, true);
            }
        }
    }
    let normalized = replace_sphinx_error_logs(&normalized);
    let normalized = String::from_utf8_lossy(&normalized);
    reduce_sphinx_error_report(&normalized)
}

fn reduce_sphinx_error_report(warnings: &str) -> String {
    let lines = warnings.split_inclusive('\n').collect::<Vec<_>>();
    let header_index = lines.iter().position(|line| {
        let line = line.trim_end_matches(['\r', '\n']);
        let Some(prefix) = line.strip_suffix(" error!") else {
            return false;
        };
        !prefix.is_empty()
            && prefix
                .chars()
                .all(|character| character.is_ascii_alphabetic() || character == ' ')
    });
    let report_start = header_index.or_else(|| {
        lines.iter().enumerate().find_map(|(index, line)| {
            (line.trim_end_matches(['\r', '\n']) == "Versions"
                && lines
                    .get(index + 1)
                    .is_some_and(|line| line.trim_end_matches(['\r', '\n']) == "========"))
            .then_some(index)
        })
    });
    let Some(report_start) = report_start else {
        return warnings.to_string();
    };
    let Some(traceback_index) =
        lines
            .iter()
            .enumerate()
            .skip(report_start + 1)
            .find_map(|(index, line)| {
                (line.trim_end_matches(['\r', '\n']) == "Traceback").then_some(index)
            })
    else {
        return warnings.to_string();
    };
    let Some(exception_index) = lines
        .iter()
        .enumerate()
        .skip(traceback_index + 1)
        .rev()
        .find_map(|(index, line)| is_exception_message_line(line).then_some(index))
    else {
        return warnings.to_string();
    };
    let mut exception_end = exception_index + 1;
    while exception_end < lines.len()
        && !lines[exception_end].trim().is_empty()
        && lines[exception_end].starts_with("    ")
    {
        exception_end += 1;
    }
    let prefix = lines[..report_start].concat();
    let exception = lines[exception_index..exception_end]
        .concat()
        .trim_end_matches(['\r', '\n'])
        .to_string();
    let Some(header_index) = header_index else {
        return format!("{prefix}{exception}\n");
    };
    let header = lines[header_index].trim_end_matches(['\r', '\n']);
    format!("{prefix}{header}\n\n{exception}\n")
}

fn is_exception_message_line(line: &str) -> bool {
    let line = line.trim_end_matches(['\r', '\n']);
    let Some(type_name) = line
        .strip_prefix("    ")
        .and_then(|line| line.split_once(':').map(|(name, _)| name))
    else {
        return false;
    };
    !type_name.is_empty()
        && type_name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '.'))
        && type_name
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
}

fn replace_source_root_token(
    bytes: &[u8],
    source_root: Option<&Path>,
    json_escaped: bool,
) -> Vec<u8> {
    let Some(source_root) = source_root else {
        return bytes.to_vec();
    };
    replace_path_token(bytes, source_root, b"<SRCDIR>", json_escaped)
}

fn replace_path_token(bytes: &[u8], root: &Path, token: &[u8], json_escaped: bool) -> Vec<u8> {
    let root = root.to_string_lossy();
    if root.is_empty() {
        return bytes.to_vec();
    }
    let mut spellings = vec![root.to_string()];
    let slash = root.replace('\\', "/");
    if slash != root {
        spellings.push(slash);
    }
    if json_escaped && root.contains('\\') {
        spellings.push(root.replace('\\', "\\\\"));
    }
    spellings.sort_by_key(|spelling| spelling.len());
    spellings
        .into_iter()
        .rev()
        .fold(bytes.to_vec(), |bytes, spelling| {
            replace_token(&bytes, spelling.as_bytes(), token)
        })
}

fn replace_sphinx_error_logs(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        let Some(relative) = bytes[cursor..]
            .windows(b"sphinx-err-".len())
            .position(|window| window == b"sphinx-err-")
        else {
            output.extend_from_slice(&bytes[cursor..]);
            break;
        };
        let marker = cursor + relative;
        let start = bytes[..marker]
            .iter()
            .rposition(|byte| byte.is_ascii_whitespace())
            .map_or(0, |index| index + 1);
        let end = bytes[marker..]
            .iter()
            .position(|byte| byte.is_ascii_whitespace())
            .map_or(bytes.len(), |index| marker + index);
        let candidate = &bytes[start..end];
        let absolute = candidate.starts_with(b"/")
            || (candidate.len() >= 3
                && candidate[1] == b':'
                && matches!(candidate[2], b'/' | b'\\'));
        if absolute && candidate.ends_with(b".log") {
            output.extend_from_slice(&bytes[cursor..start]);
            output.extend_from_slice(b"<SPHINX_ERR_LOG>");
            cursor = end;
        } else {
            output.extend_from_slice(&bytes[cursor..marker + b"sphinx-err-".len()]);
            cursor = marker + b"sphinx-err-".len();
        }
    }
    output
}

fn replace_token(bytes: &[u8], token: &[u8], replacement: &[u8]) -> Vec<u8> {
    if token.is_empty() {
        return bytes.to_vec();
    }
    let mut output = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        let matches = bytes[cursor..].starts_with(token);
        let boundary = bytes.get(cursor + token.len()).is_none_or(|byte| {
            *byte == b'/'
                || *byte == b'\\'
                || !byte.is_ascii_alphanumeric() && !matches!(*byte, b'.' | b'_' | b'-')
        });
        if matches && boundary {
            output.extend_from_slice(replacement);
            cursor += token.len();
        } else {
            output.push(bytes[cursor]);
            cursor += 1;
        }
    }
    output
}

fn normalize_crlf(bytes: &[u8]) -> Vec<u8> {
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            normalized.push(b'\n');
            index += 2;
        } else {
            normalized.push(bytes[index]);
            index += 1;
        }
    }
    normalized
}

fn value_diagnostic(
    category: &str,
    logical_path: &str,
    expected: &[u8],
    actual: &[u8],
) -> Diagnostic {
    let expected = display_bytes(expected);
    let actual = display_bytes(actual);
    Diagnostic {
        category: category.to_string(),
        logical_path: logical_path.to_string(),
        first_expected_line: first_difference_line(&expected, &actual),
        expected,
        actual,
        detail: "normalized values differ".to_string(),
    }
}

fn invalid_diagnostic(category: &str, logical_path: &str, detail: String) -> Diagnostic {
    Diagnostic {
        category: category.to_string(),
        logical_path: logical_path.to_string(),
        first_expected_line: None,
        expected: String::new(),
        actual: String::new(),
        detail,
    }
}

fn display_bytes(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn first_difference_line(expected: &str, actual: &str) -> Option<usize> {
    let mut expected_lines = expected.lines();
    let mut actual_lines = actual.lines();
    let mut line_number = 1;
    loop {
        match (expected_lines.next(), actual_lines.next()) {
            (None, None) => return None,
            (left, right) if left == right => line_number += 1,
            _ => return Some(line_number),
        }
    }
}

pub fn status_name(status: CaseStatus) -> &'static str {
    match status {
        CaseStatus::Built => "built",
        CaseStatus::BuildError => "build-error",
        CaseStatus::ReferenceCrash => "reference-crash",
        CaseStatus::ExcludedNetwork => "excluded-network",
        CaseStatus::ExcludedPlantuml => "excluded-plantuml",
    }
}

fn diagnostic_order(left: &Diagnostic, right: &Diagnostic) -> std::cmp::Ordering {
    left.logical_path
        .cmp(&right.logical_path)
        .then_with(|| left.category.cmp(&right.category))
        .then_with(|| left.first_expected_line.cmp(&right.first_expected_line))
        .then_with(|| left.detail.cmp(&right.detail))
}

pub fn diagnose_html(expected: &str, actual: &str) -> Diagnostic {
    let expected = String::from_utf8_lossy(&normalize_crlf(expected.as_bytes())).into_owned();
    let actual = String::from_utf8_lossy(&normalize_crlf(actual.as_bytes())).into_owned();
    let expected_regions = split_html_regions(&expected);
    let actual_regions = split_actual_html_regions(&actual);
    let (expected_regions, actual_regions) = match (expected_regions, actual_regions) {
        (Some(expected), Some(actual)) => (expected, actual),
        _ => {
            return Diagnostic {
                category: "html-unstructured".to_string(),
                logical_path: String::new(),
                first_expected_line: first_difference_line(&expected, &actual),
                expected: expected.clone(),
                actual: actual.clone(),
                detail: unified_diff(&expected, &actual),
            }
        }
    };
    let body_differs = expected_regions.body != actual_regions.body;
    let chrome_differs = expected_regions.chrome != actual_regions.chrome;
    let category = if body_differs && actual_regions.fallback {
        "html-body-fallback"
    } else {
        match (body_differs, chrome_differs) {
            (true, false) => "html-body",
            (false, true) => "html-chrome",
            (true, true) => "html-both",
            (false, false) => "html-body",
        }
    };
    let mut detail = String::new();
    if body_differs {
        detail.push_str(&unified_diff(&expected_regions.body, &actual_regions.body));
    }
    if chrome_differs && !actual_regions.fallback {
        if !detail.is_empty() {
            detail.push('\n');
        }
        detail.push_str(&unified_diff(
            &expected_regions.chrome,
            &actual_regions.chrome,
        ));
    }
    Diagnostic {
        category: category.to_string(),
        logical_path: String::new(),
        first_expected_line: first_difference_line(&expected, &actual),
        expected,
        actual,
        detail: cap_text(&detail),
    }
}

pub fn diagnose_needs_json(expected: &Value, actual: &Value) -> Vec<Diagnostic> {
    let (expected_object, actual_object) = match (expected.as_object(), actual.as_object()) {
        (Some(expected), Some(actual)) => (expected, actual),
        _ => {
            return vec![invalid_diagnostic(
                "needs-json-path",
                "",
                "needs.json root must be an object".to_string(),
            )]
        }
    };
    let expected_versions = match expected_object.get("versions").and_then(Value::as_array) {
        Some(versions) => versions,
        None => {
            return vec![invalid_diagnostic(
                "needs-json-path",
                "versions",
                "versions must be an array".to_string(),
            )]
        }
    };
    let actual_versions = match actual_object.get("versions").and_then(Value::as_array) {
        Some(versions) => versions,
        None => {
            return vec![invalid_diagnostic(
                "needs-json-path",
                "versions",
                "versions must be an array".to_string(),
            )]
        }
    };
    let mut diagnostics = Vec::new();
    diagnostics.extend(diagnose_object_keys(
        expected_object,
        actual_object,
        "needs-top-level",
        "",
    ));
    if expected_versions.len() != actual_versions.len() {
        diagnostics.push(Diagnostic {
            category: "needs-top-level".to_string(),
            logical_path: "versions".to_string(),
            first_expected_line: None,
            expected: expected_versions.len().to_string(),
            actual: actual_versions.len().to_string(),
            detail: "version array lengths differ".to_string(),
        });
    }
    for index in 0..expected_versions.len().min(actual_versions.len()) {
        let expected_version = match expected_versions[index].as_object() {
            Some(version) => version,
            None => {
                diagnostics.push(invalid_diagnostic(
                    "needs-json-path",
                    &format!("versions[{index}]"),
                    "version must be an object".to_string(),
                ));
                continue;
            }
        };
        let actual_version = match actual_versions[index].as_object() {
            Some(version) => version,
            None => {
                diagnostics.push(invalid_diagnostic(
                    "needs-json-path",
                    &format!("versions[{index}]"),
                    "version must be an object".to_string(),
                ));
                continue;
            }
        };
        diagnostics.extend(diagnose_object_keys(
            expected_version,
            actual_version,
            "needs-top-level",
            &format!("versions[{index}]"),
        ));
        let expected_needs = match expected_version.get("needs").and_then(Value::as_object) {
            Some(needs) => needs,
            None => {
                diagnostics.push(invalid_diagnostic(
                    "needs-json-path",
                    &format!("versions[{index}].needs"),
                    "needs must be an object".to_string(),
                ));
                continue;
            }
        };
        let actual_needs = match actual_version.get("needs").and_then(Value::as_object) {
            Some(needs) => needs,
            None => {
                diagnostics.push(invalid_diagnostic(
                    "needs-json-path",
                    &format!("versions[{index}].needs"),
                    "needs must be an object".to_string(),
                ));
                continue;
            }
        };
        let mut need_ids = BTreeSet::new();
        need_ids.extend(expected_needs.keys().cloned());
        need_ids.extend(actual_needs.keys().cloned());
        for need_id in need_ids {
            let path = format!("versions[{index}].needs[{:?}]", need_id);
            match (expected_needs.get(&need_id), actual_needs.get(&need_id)) {
                (Some(expected_need), None) => diagnostics.push(Diagnostic {
                    category: "missing-need".to_string(),
                    logical_path: path,
                    first_expected_line: None,
                    expected: json_compact(expected_need),
                    actual: String::new(),
                    detail: "need is absent from actual output".to_string(),
                }),
                (None, Some(actual_need)) => diagnostics.push(Diagnostic {
                    category: "extra-need".to_string(),
                    logical_path: path,
                    first_expected_line: None,
                    expected: String::new(),
                    actual: json_compact(actual_need),
                    detail: "need exists only in actual output".to_string(),
                }),
                (Some(expected_need), Some(actual_need)) => {
                    let (expected_need, actual_need) =
                        match (expected_need.as_object(), actual_need.as_object()) {
                            (Some(expected), Some(actual)) => (expected, actual),
                            _ => {
                                diagnostics.push(invalid_diagnostic(
                                    "needs-json-path",
                                    &path,
                                    "need record must be an object".to_string(),
                                ));
                                continue;
                            }
                        };
                    let mut field_names = BTreeSet::new();
                    field_names.extend(expected_need.keys().cloned());
                    field_names.extend(actual_need.keys().cloned());
                    for field in field_names {
                        let expected_field = expected_need.get(&field);
                        let actual_field = actual_need.get(&field);
                        if expected_field != actual_field {
                            diagnostics.push(Diagnostic {
                                category: "need-field".to_string(),
                                logical_path: path.clone(),
                                first_expected_line: None,
                                expected: expected_field.map(json_compact).unwrap_or_default(),
                                actual: actual_field.map(json_compact).unwrap_or_default(),
                                detail: format!("field {field:?} differs"),
                            });
                        }
                    }
                }
                (None, None) => unreachable!(),
            }
        }
    }
    diagnostics.sort_by(diagnostic_order);
    diagnostics
}

pub fn diagnose_searchindex(expected: &Value, actual: &Value) -> Vec<Diagnostic> {
    let (expected, actual) = match (expected.as_object(), actual.as_object()) {
        (Some(expected), Some(actual)) => (expected, actual),
        _ => {
            return vec![invalid_diagnostic(
                "searchindex-value",
                "searchindex.js",
                "searchindex value must be an object".to_string(),
            )]
        }
    };
    let mut keys = BTreeSet::new();
    keys.extend(expected.keys().cloned());
    keys.extend(actual.keys().cloned());
    let mut diagnostics = Vec::new();
    for key in keys {
        match (expected.get(&key), actual.get(&key)) {
            (Some(expected), None) => diagnostics.push(Diagnostic {
                category: "searchindex-missing-key".to_string(),
                logical_path: key.clone(),
                first_expected_line: None,
                expected: json_compact(expected),
                actual: String::new(),
                detail: format!("searchindex key {key:?} missing from actual"),
            }),
            (None, Some(actual)) => diagnostics.push(Diagnostic {
                category: "searchindex-extra-key".to_string(),
                logical_path: key.clone(),
                first_expected_line: None,
                expected: String::new(),
                actual: json_compact(actual),
                detail: format!("searchindex key {key:?} is extra in actual"),
            }),
            (Some(expected), Some(actual)) if expected != actual => diagnostics.push(Diagnostic {
                category: "searchindex-changed-key".to_string(),
                logical_path: key.clone(),
                first_expected_line: None,
                expected: json_compact(expected),
                actual: json_compact(actual),
                detail: format!("searchindex key {key:?} changed"),
            }),
            (Some(_), Some(_)) | (None, None) => {}
        }
    }
    diagnostics.sort_by(diagnostic_order);
    diagnostics
}

pub fn diagnose_inventory(
    expected: &[InventoryRecord],
    actual: &[InventoryRecord],
) -> Vec<Diagnostic> {
    let expected = expected
        .iter()
        .map(|record| (inventory_identity(record), record))
        .collect::<BTreeMap<_, _>>();
    let actual = actual
        .iter()
        .map(|record| (inventory_identity(record), record))
        .collect::<BTreeMap<_, _>>();
    let mut identities = BTreeSet::new();
    identities.extend(expected.keys().cloned());
    identities.extend(actual.keys().cloned());
    let mut diagnostics = Vec::new();
    for identity in identities {
        match (expected.get(&identity), actual.get(&identity)) {
            (Some(expected), None) => diagnostics.push(Diagnostic {
                category: "inventory-missing-record".to_string(),
                logical_path: identity.0.clone(),
                first_expected_line: None,
                expected: inventory_record_text(expected),
                actual: String::new(),
                detail: "inventory record is absent from actual output".to_string(),
            }),
            (None, Some(actual)) => diagnostics.push(Diagnostic {
                category: "inventory-extra-record".to_string(),
                logical_path: identity.0.clone(),
                first_expected_line: None,
                expected: String::new(),
                actual: inventory_record_text(actual),
                detail: "inventory record exists only in actual output".to_string(),
            }),
            (Some(expected), Some(actual)) if expected != actual => diagnostics.push(Diagnostic {
                category: "inventory-changed-record".to_string(),
                logical_path: identity.0.clone(),
                first_expected_line: None,
                expected: inventory_record_text(expected),
                actual: inventory_record_text(actual),
                detail: "inventory URI or display name changed".to_string(),
            }),
            (Some(_), Some(_)) | (None, None) => {}
        }
    }
    diagnostics.sort_by(diagnostic_order);
    diagnostics
}

pub fn diagnose_warnings(expected: &str, actual: &str) -> Vec<Diagnostic> {
    let expected = String::from_utf8_lossy(&normalize_crlf(expected.as_bytes())).into_owned();
    let actual = String::from_utf8_lossy(&normalize_crlf(actual.as_bytes())).into_owned();
    let expected = expected.lines().collect::<Vec<_>>();
    let actual = actual.lines().collect::<Vec<_>>();
    let mut diagnostics = Vec::new();
    for index in 0..expected.len().max(actual.len()) {
        match (expected.get(index), actual.get(index)) {
            (Some(expected), Some(actual)) if expected != actual => diagnostics.push(Diagnostic {
                category: "warning-changed-line".to_string(),
                logical_path: "warnings".to_string(),
                first_expected_line: Some(index + 1),
                expected: (*expected).to_string(),
                actual: (*actual).to_string(),
                detail: "warning line differs".to_string(),
            }),
            (Some(expected), None) => diagnostics.push(Diagnostic {
                category: "warning-missing-line".to_string(),
                logical_path: "warnings".to_string(),
                first_expected_line: Some(index + 1),
                expected: (*expected).to_string(),
                actual: String::new(),
                detail: "warning line is absent from actual output".to_string(),
            }),
            (None, Some(actual)) => diagnostics.push(Diagnostic {
                category: "warning-extra-line".to_string(),
                logical_path: "warnings".to_string(),
                first_expected_line: Some(index + 1),
                expected: String::new(),
                actual: (*actual).to_string(),
                detail: "warning line exists only in actual output".to_string(),
            }),
            (None, None) | (Some(_), Some(_)) => {}
        }
    }
    diagnostics.sort_by(diagnostic_order);
    diagnostics
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    Failed,
    All,
}

impl Keep {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::All => "all",
        }
    }
}

pub fn parse_keep(value: Option<&str>) -> Result<Keep, String> {
    match value.unwrap_or("failed") {
        "failed" => Ok(Keep::Failed),
        "all" => Ok(Keep::All),
        other => Err(format!(
            "HTML_ORACLE_KEEP must be failed or all, got {other:?}"
        )),
    }
}

pub fn resolve_ultra_binary(override_value: Option<&str>, default: &str) -> String {
    override_value
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_string()
}

pub fn rerun_filter(case_key: &str, filter: Option<&str>) -> Option<String> {
    match filter {
        Some(filter) if !case_key.contains(filter) => None,
        _ => Some(case_key.to_string()),
    }
}

#[derive(Debug)]
pub struct FixtureSuite {
    pub root: PathBuf,
    pub profiles: BTreeMap<String, IndexDocument>,
}

pub fn load_fixture_suite(root: &Path) -> Result<FixtureSuite, String> {
    let required = [
        ("core", root.join("core").join("index.json")),
        ("local_needs", root.join("local_needs").join("index.json")),
    ];
    let missing = required
        .iter()
        .filter(|(_, path)| !path.is_file())
        .map(|(profile, path)| format!("{profile}/index.json ({})", path.display()))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "HTML oracle fixture corpus is missing required ledgers: {}. Generate the profile corpus before running html_oracle_exhaustive.",
            missing.join(", ")
        ));
    }
    let mut profiles = BTreeMap::new();
    for (profile, path) in required {
        let document = IndexDocument::load(&path)?;
        if !document.profiles.contains_key(profile) {
            return Err(format!(
                "{} does not contain its selected profile {profile:?}",
                path.display()
            ));
        }
        profiles.insert(profile.to_string(), document);
    }
    Ok(FixtureSuite {
        root: root.to_path_buf(),
        profiles,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MismatchKey {
    category: String,
    logical_path: String,
    expected_sha256: String,
    actual_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
struct ReportDiagnostic {
    category: String,
    logical_path: String,
    first_expected_line: Option<usize>,
    expected_sha256: String,
    expected_size: usize,
    actual_sha256: String,
    actual_size: usize,
    diff: String,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
struct CommonMismatch {
    category: String,
    logical_path: String,
    expected_sha256: String,
    expected_size: usize,
    actual_sha256: String,
    actual_size: usize,
    count: usize,
    source_set_count: usize,
    sample_cases: Vec<String>,
    diff: String,
    detail: String,
}

struct CommonMismatchAggregate {
    case_keys: BTreeSet<String>,
    source_sets: BTreeSet<String>,
    diagnostic: ReportDiagnostic,
}

fn report_diagnostic(diagnostic: &Diagnostic) -> ReportDiagnostic {
    let diff = if diagnostic.detail.starts_with("--- expected") {
        diagnostic.detail.clone()
    } else {
        unified_diff(&diagnostic.expected, &diagnostic.actual)
    };
    ReportDiagnostic {
        category: diagnostic.category.clone(),
        logical_path: diagnostic.logical_path.clone(),
        first_expected_line: diagnostic.first_expected_line,
        expected_sha256: sha256_hex(diagnostic.expected.as_bytes()),
        expected_size: diagnostic.expected.len(),
        actual_sha256: sha256_hex(diagnostic.actual.as_bytes()),
        actual_size: diagnostic.actual.len(),
        diff: cap_text(&diff),
        detail: cap_text(&diagnostic.detail),
    }
}

fn mismatch_key(diagnostic: &Diagnostic) -> MismatchKey {
    let report = report_diagnostic(diagnostic);
    MismatchKey {
        category: report.category,
        logical_path: report.logical_path,
        expected_sha256: report.expected_sha256,
        actual_sha256: report.actual_sha256,
    }
}

fn report_case_value(result: &CaseResult) -> Value {
    serde_json::json!({
        "profile": result.profile,
        "source_set": result.source_set,
        "case_id": result.case_id,
        "html_status": result.html_status,
        "exception_type": result.exception_type,
        "needs_status": result.needs_status,
        "passed": result.passed,
        "run_dir": result.run_dir,
        "rerun_filter": result.rerun_filter,
        "common_mismatch_count": result.common_mismatch_count,
        "diagnostics": result.diagnostics.iter().map(report_diagnostic).collect::<Vec<_>>(),
    })
}

fn common_mismatches(results: &[CaseResult]) -> (Vec<CommonMismatch>, BTreeSet<MismatchKey>) {
    let mut grouped = BTreeMap::<MismatchKey, CommonMismatchAggregate>::new();
    for result in results {
        let case_key = format!(
            "{}/{}/{}",
            result.profile, result.source_set, result.case_id
        );
        for diagnostic in &result.diagnostics {
            let key = mismatch_key(diagnostic);
            let entry = grouped
                .entry(key.clone())
                .or_insert_with(|| CommonMismatchAggregate {
                    case_keys: BTreeSet::new(),
                    source_sets: BTreeSet::new(),
                    diagnostic: report_diagnostic(diagnostic),
                });
            entry.case_keys.insert(case_key.clone());
            entry.source_sets.insert(result.source_set.clone());
        }
    }
    let mut common = Vec::new();
    let mut common_keys = BTreeSet::new();
    for (key, aggregate) in grouped {
        if aggregate.case_keys.len() < 2 {
            continue;
        }
        common_keys.insert(key.clone());
        let sample_cases = aggregate.case_keys.iter().take(3).cloned().collect();
        common.push(CommonMismatch {
            category: key.category,
            logical_path: key.logical_path,
            expected_sha256: key.expected_sha256,
            expected_size: aggregate.diagnostic.expected_size,
            actual_sha256: key.actual_sha256,
            actual_size: aggregate.diagnostic.actual_size,
            count: aggregate.case_keys.len(),
            source_set_count: aggregate.source_sets.len(),
            sample_cases,
            diff: aggregate.diagnostic.diff,
            detail: aggregate.diagnostic.detail,
        });
    }
    common.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.category.cmp(&right.category))
            .then_with(|| left.logical_path.cmp(&right.logical_path))
            .then_with(|| left.expected_sha256.cmp(&right.expected_sha256))
            .then_with(|| left.actual_sha256.cmp(&right.actual_sha256))
    });
    (common, common_keys)
}

fn file_kind(diagnostic: &Diagnostic) -> Option<&'static str> {
    let path = diagnostic.logical_path.as_str();
    if diagnostic.category.starts_with("warning") || path == "warnings" {
        Some("warnings")
    } else if path.ends_with("needs.json") {
        Some("needs.json")
    } else if path == "searchindex.js" {
        Some("searchindex")
    } else if path == "objects.inv" {
        Some("objects.inv")
    } else if path.ends_with(".buildinfo") {
        Some("buildinfo")
    } else if path.starts_with("_static/") {
        Some("static")
    } else if path.starts_with("_sources/") {
        Some("sources")
    } else if path.ends_with(".html") {
        Some("html")
    } else {
        None
    }
}

fn failing_cases_by_file_kind(results: &[CaseResult]) -> BTreeMap<String, usize> {
    let kinds = [
        "html",
        "searchindex",
        "objects.inv",
        "buildinfo",
        "static",
        "sources",
        "warnings",
        "needs.json",
    ];
    let mut counts = kinds
        .into_iter()
        .map(|kind| (kind.to_string(), 0usize))
        .collect::<BTreeMap<_, _>>();
    for result in results.iter().filter(|result| !result.passed) {
        let mut seen = BTreeSet::new();
        for diagnostic in &result.diagnostics {
            if let Some(kind) = file_kind(diagnostic) {
                seen.insert(kind);
            }
        }
        for kind in seen {
            *counts.get_mut(kind).expect("file kind is initialized") += 1;
        }
    }
    counts
}

pub fn build_report(
    total_cases: usize,
    excluded_cases: usize,
    reference_crash_cases: usize,
    results: &[CaseResult],
) -> (Value, String) {
    build_report_with_platforms(
        total_cases,
        excluded_cases,
        reference_crash_cases,
        results,
        &BTreeMap::new(),
    )
}

pub fn build_report_with_platforms(
    total_cases: usize,
    excluded_cases: usize,
    reference_crash_cases: usize,
    results: &[CaseResult],
    reference_platforms: &BTreeMap<String, String>,
) -> (Value, String) {
    build_report_with_platforms_and_reference_cases(
        total_cases,
        excluded_cases,
        reference_crash_cases,
        results,
        reference_platforms,
        &[],
    )
}

pub fn build_report_with_platforms_and_reference_cases(
    total_cases: usize,
    excluded_cases: usize,
    reference_crash_cases: usize,
    results: &[CaseResult],
    reference_platforms: &BTreeMap<String, String>,
    reference_cases: &[ReferenceCase],
) -> (Value, String) {
    build_report_with_platforms_reference_cases_and_bin(
        total_cases,
        excluded_cases,
        reference_crash_cases,
        results,
        reference_platforms,
        reference_cases,
        None,
    )
}

pub fn build_report_with_platforms_reference_cases_and_bin(
    total_cases: usize,
    excluded_cases: usize,
    reference_crash_cases: usize,
    results: &[CaseResult],
    reference_platforms: &BTreeMap<String, String>,
    reference_cases: &[ReferenceCase],
    ultra_bin: Option<&str>,
) -> (Value, String) {
    let mut results = results.to_vec();
    results.sort_by(case_result_order);
    let passed_cases = results.iter().filter(|result| result.passed).count();
    let failed_cases = results.len() - passed_cases;

    let mut summary = BTreeMap::<(String, String), (usize, usize, usize)>::new();
    let mut counts_by_source_set = BTreeMap::<String, (usize, usize, usize)>::new();
    let mut counts_by_category = BTreeMap::<String, usize>::new();
    let mut all_diagnostics = Vec::new();
    for result in &results {
        let summary_entry = summary
            .entry((result.profile.clone(), result.source_set.clone()))
            .or_default();
        summary_entry.0 += 1;
        if result.passed {
            summary_entry.1 += 1;
        } else {
            summary_entry.2 += 1;
        }
        let source_entry = counts_by_source_set
            .entry(result.source_set.clone())
            .or_default();
        source_entry.0 += 1;
        if result.passed {
            source_entry.1 += 1;
        } else {
            source_entry.2 += 1;
        }
        for diagnostic in &result.diagnostics {
            *counts_by_category
                .entry(diagnostic.category.clone())
                .or_default() += 1;
            all_diagnostics.push(diagnostic.clone());
        }
    }
    all_diagnostics.sort_by(diagnostic_order);
    let first_divergences = group_first_divergences(&all_diagnostics);
    let failing_cases_by_file_kind = failing_cases_by_file_kind(&results);
    let (common_mismatches, common_keys) = common_mismatches(&results);
    for result in &mut results {
        result.common_mismatch_count = result
            .diagnostics
            .iter()
            .filter(|diagnostic| common_keys.contains(&mismatch_key(diagnostic)))
            .count();
        result
            .diagnostics
            .retain(|diagnostic| !common_keys.contains(&mismatch_key(diagnostic)));
    }
    let summary_json = summary
        .iter()
        .map(|((profile, source_set), (scheduled, passed, failed))| {
            serde_json::json!({
                "profile": profile,
                "source_set": source_set,
                "scheduled": scheduled,
                "passed": passed,
                "failed": failed,
            })
        })
        .collect::<Vec<_>>();
    let source_counts_json = counts_by_source_set
        .iter()
        .map(|(source_set, (scheduled, passed, failed))| {
            (
                source_set.clone(),
                serde_json::json!({
                    "scheduled": scheduled,
                    "passed": passed,
                    "failed": failed,
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let report = serde_json::json!({
        "total_cases": total_cases,
        "scheduled_cases": results.len(),
        "passed_cases": passed_cases,
        "failed_cases": failed_cases,
        "excluded_cases": excluded_cases,
        "reference_crash_cases": reference_crash_cases,
        "counts_by_source_set": source_counts_json,
        "counts_by_category": counts_by_category,
        "summary_by_profile_source_set": summary_json,
        "categories": counts_by_category,
        "first_divergences": first_divergences,
        "common_mismatches": common_mismatches,
        "failing_cases_by_file_kind": failing_cases_by_file_kind,
        "reference_platforms": reference_platforms,
        "host_platform": std::env::consts::OS,
        "reference_cases": reference_cases,
        "ultra_bin": ultra_bin,
        "cases": results.iter().map(report_case_value).collect::<Vec<_>>(),
    });
    let markdown = render_report_markdown(&report, &results, &first_divergences);
    (report, markdown)
}

pub fn bounded_assertion_message(
    report_markdown: &Path,
    report_json: &Path,
    details: &str,
) -> String {
    let prefix = format!(
        "HTML oracle comparison failed. Complete reports: {} and {}.\n\n",
        report_markdown.display(),
        report_json.display()
    );
    cap_text(&format!("{prefix}{details}"))
}

pub fn apply_retention(run_dir: &Path, passed: bool, keep: Keep) -> Result<(), String> {
    if passed && keep == Keep::Failed && run_dir.exists() {
        fs::remove_dir_all(run_dir)
            .map_err(|error| format!("remove passing run {}: {error}", run_dir.display()))?;
    }
    Ok(())
}

fn case_result_order(left: &CaseResult, right: &CaseResult) -> std::cmp::Ordering {
    left.profile
        .cmp(&right.profile)
        .then_with(|| left.source_set.cmp(&right.source_set))
        .then_with(|| left.case_id.cmp(&right.case_id))
}

fn render_report_markdown(
    report: &Value,
    results: &[CaseResult],
    first_divergences: &[FirstDivergenceGroup],
) -> String {
    let mut output = String::new();
    output.push_str("# HTML Oracle Report\n\n");
    if let Some(ultra_bin) = report["ultra_bin"].as_str() {
        output.push_str(&format!("Ultra binary: `{ultra_bin}`.\n\n"));
    }
    output.push_str("## Summary\n\n");
    output.push_str("| profile | source_set | scheduled | passed | failed |\n");
    output.push_str("| --- | --- | ---: | ---: | ---: |\n");
    for row in report["summary_by_profile_source_set"]
        .as_array()
        .into_iter()
        .flatten()
    {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            row["profile"].as_str().unwrap_or_default(),
            row["source_set"].as_str().unwrap_or_default(),
            row["scheduled"],
            row["passed"],
            row["failed"],
        ));
    }
    output.push_str(&format!(
        "\nTotal: {} scheduled, {} passed, {} failed; {} excluded, {} reference crashes.\n\n",
        report["scheduled_cases"],
        report["passed_cases"],
        report["failed_cases"],
        report["excluded_cases"],
        report["reference_crash_cases"]
    ));
    if let Some(platforms) = report["reference_platforms"].as_object() {
        if !platforms.is_empty() {
            output.push_str("## Reference platform\n\n");
            output.push_str("| profile | platform |\n| --- | --- |\n");
            for (profile, platform) in platforms {
                output.push_str(&format!(
                    "| {profile} | {} |\n",
                    platform.as_str().unwrap_or_default()
                ));
            }
            if platforms.len() == 1 {
                let platform = platforms
                    .values()
                    .next()
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                output.push_str(&format!("\nReference platform: `{platform}`.\n"));
            }
            let host = report["host_platform"].as_str().unwrap_or_default();
            output.push_str(&format!("\nHost platform: `{host}`.\n"));
            if platforms
                .values()
                .any(|platform| platform.as_str() != Some(host))
            {
                output.push_str(
                    "Separator and path differences are expected when the reference platform differs from the host.\n\n",
                );
            } else {
                output.push('\n');
            }
        }
    }
    if let Some(reference_cases) = report["reference_cases"].as_array() {
        if !reference_cases.is_empty() {
            output.push_str("## Reference-only cases\n\n");
            output.push_str(
                "| profile | source_set | case_id | status | exception_type |\n| --- | --- | --- | --- | --- |\n",
            );
            for case in reference_cases {
                output.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    case["profile"].as_str().unwrap_or_default(),
                    case["source_set"].as_str().unwrap_or_default(),
                    case["case_id"].as_str().unwrap_or_default(),
                    case["status"].as_str().unwrap_or_default(),
                    case["exception_type"].as_str().unwrap_or("null"),
                ));
            }
            output.push('\n');
        }
    }
    output.push_str("## Common mismatches\n\n");
    output.push_str(
        "| category | logical path | count | source sets | sample cases |\n| --- | --- | ---: | ---: | --- |\n",
    );
    if let Some(common) = report["common_mismatches"].as_array() {
        for mismatch in common {
            let sample_cases = mismatch["sample_cases"]
                .as_array()
                .map(|cases| {
                    cases
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            output.push_str(&format!(
                "| {} | {} | {} | {} | {sample_cases} |\n",
                mismatch["category"].as_str().unwrap_or_default(),
                mismatch["logical_path"].as_str().unwrap_or_default(),
                mismatch["count"],
                mismatch["source_set_count"],
            ));
        }
    }
    if report["common_mismatches"]
        .as_array()
        .is_none_or(Vec::is_empty)
    {
        output.push_str("| none | | 0 | 0 | |\n");
    }
    output.push('\n');
    output.push_str("## Failing cases by file kind\n\n");
    output.push_str("| file kind | failing cases |\n| --- | ---: |\n");
    if let Some(kinds) = report["failing_cases_by_file_kind"].as_object() {
        for (kind, count) in kinds {
            output.push_str(&format!("| {kind} | {count} |\n"));
        }
    }
    output.push('\n');
    output.push_str("## Category counts\n\n");
    output.push_str("| category | count |\n| --- | ---: |\n");
    if let Some(categories) = report["counts_by_category"].as_object() {
        for (category, count) in categories {
            output.push_str(&format!("| {category} | {count} |\n"));
        }
    }
    output.push_str("\n## Most common first-divergence\n\n");
    output.push_str(
        "| expected line | expected first differing line | actual first differing line | count | sample files |\n| ---: | --- | --- | ---: | --- |\n",
    );
    for group in first_divergences {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            group
                .expected_line
                .map_or_else(|| "-".to_string(), |line| line.to_string()),
            group.expected_text,
            group.actual_text,
            group.count,
            group.sample_files.join(", ")
        ));
    }
    for result in results {
        let key = format!(
            "{}/{}/{}",
            result.profile, result.source_set, result.case_id
        );
        output.push_str(&format!("\n## {key}\n\n"));
        let status = if matches!(
            result.html_status.as_str(),
            "build-error" | "reference-crash"
        ) {
            format!(
                "{} (exception_type: `{}`)",
                result.html_status,
                result.exception_type.as_deref().unwrap_or("null")
            )
        } else {
            result.html_status.clone()
        };
        output.push_str(&format!(
            "- result: {}\n- status: {status}\n- run directory: {}\n- rerun: HTML_ORACLE_FILTER={} HTML_ORACLE_KEEP=all cargo test --test html_differential html_oracle_exhaustive -- --ignored --nocapture\n",
            if result.passed { "passed" } else { "failed" },
            result.run_dir,
            result.rerun_filter
        ));
        if result.common_mismatch_count > 0 {
            output.push_str(&format!(
                "+{} common mismatches (see Common mismatches)\n",
                result.common_mismatch_count
            ));
        }
        for diagnostic in &result.diagnostics {
            output.push_str(&format!(
                "\n### {} — {}\n\n{}\n",
                diagnostic.category,
                diagnostic.logical_path,
                cap_text(&diagnostic.detail)
            ));
        }
    }
    output
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FirstDivergenceGroup {
    pub expected_line: Option<usize>,
    pub expected_text: String,
    pub actual_text: String,
    pub count: usize,
    pub sample_files: Vec<String>,
}

pub fn group_first_divergences(diagnostics: &[Diagnostic]) -> Vec<FirstDivergenceGroup> {
    let mut grouped = BTreeMap::<(String, String), (usize, BTreeSet<String>, Option<usize>)>::new();
    for diagnostic in diagnostics {
        if diagnostic.category.starts_with("html-") {
            if let Some((computed_line, expected, actual)) =
                first_difference_text(&diagnostic.expected, &diagnostic.actual)
            {
                let expected_line = diagnostic.first_expected_line.unwrap_or(computed_line);
                let (count, sample_files, first_line) =
                    grouped.entry((expected, actual)).or_default();
                *count += 1;
                sample_files.insert(diagnostic.logical_path.clone());
                if first_line.is_none() {
                    *first_line = Some(expected_line);
                }
            }
        }
    }
    let mut groups = grouped
        .into_iter()
        .map(
            |((expected_text, actual_text), (count, sample_files, expected_line))| {
                let sample_files = sample_files.into_iter().take(3).collect();
                FirstDivergenceGroup {
                    expected_line,
                    expected_text,
                    actual_text,
                    count,
                    sample_files,
                }
            },
        )
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.expected_text.cmp(&right.expected_text))
            .then_with(|| left.actual_text.cmp(&right.actual_text))
    });
    groups.truncate(25);
    groups
}

struct HtmlRegions {
    body: String,
    chrome: String,
    fallback: bool,
}

fn split_html_regions(value: &str) -> Option<HtmlRegions> {
    const MARKER: &str = "<div class=\"body\" role=\"main\">";
    let body_start = value.find(MARKER)?;
    split_element_region(value, body_start, "div", false)
}

fn split_actual_html_regions(value: &str) -> Option<HtmlRegions> {
    if let Some(regions) = split_html_regions(value) {
        return Some(regions);
    }
    let candidates = [
        find_opening_tag(value, |tag| {
            tag.contains("role=\"main\"") || tag.contains("role='main'")
        }),
        find_opening_tag(value, |tag| opening_tag_name(tag) == Some("main")),
        find_opening_tag(value, |tag| opening_tag_name(tag) == Some("body")),
    ];
    for candidate in candidates.into_iter().flatten() {
        let (start, tag_name) = candidate;
        if let Some(regions) = split_element_region(value, start, &tag_name, true) {
            return Some(regions);
        }
    }
    None
}

fn find_opening_tag<F>(value: &str, predicate: F) -> Option<(usize, String)>
where
    F: Fn(&str) -> bool,
{
    let mut cursor = 0;
    while let Some(relative_start) = value[cursor..].find('<') {
        let start = cursor + relative_start;
        let end = start + value[start..].find('>')? + 1;
        let tag = &value[start..end];
        if !tag.starts_with("</") && !tag.starts_with("<!") && predicate(tag) {
            return Some((start, opening_tag_name(tag)?.to_string()));
        }
        cursor = end;
    }
    None
}

fn opening_tag_name(tag: &str) -> Option<&str> {
    let tag = tag.strip_prefix('<')?;
    let tag = tag.trim_start();
    if tag.starts_with('/') || tag.starts_with('!') {
        return None;
    }
    let end = tag
        .find(|character: char| {
            character.is_ascii_whitespace() || character == '>' || character == '/'
        })
        .unwrap_or(tag.len());
    Some(&tag[..end])
}

fn split_element_region(
    value: &str,
    body_start: usize,
    tag_name: &str,
    fallback: bool,
) -> Option<HtmlRegions> {
    let mut depth = 0usize;
    let mut cursor = body_start;
    let mut body_end = None;
    while let Some(relative_start) = value[cursor..].find('<') {
        let tag_start = cursor + relative_start;
        let relative_end = value[tag_start..].find('>')?;
        let tag_end = tag_start + relative_end + 1;
        let tag = &value[tag_start..tag_end];
        if tag.starts_with(&format!("</{tag_name}")) {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                body_end = Some(tag_end);
                break;
            }
        } else if opening_tag_name(tag) == Some(tag_name) && !tag.trim_end().ends_with("/>") {
            depth += 1;
        }
        cursor = tag_end;
    }
    let body_end = body_end?;
    Some(HtmlRegions {
        body: value[body_start..body_end].to_string(),
        chrome: format!("{}{}", &value[..body_start], &value[body_end..]),
        fallback,
    })
}

fn first_difference_text(expected: &str, actual: &str) -> Option<(usize, String, String)> {
    let expected = normalize_crlf(expected.as_bytes());
    let actual = normalize_crlf(actual.as_bytes());
    let expected = String::from_utf8_lossy(&expected);
    let actual = String::from_utf8_lossy(&actual);
    let expected_lines = expected.lines().collect::<Vec<_>>();
    let actual_lines = actual.lines().collect::<Vec<_>>();
    let first = (0..expected_lines.len().max(actual_lines.len()))
        .find(|index| expected_lines.get(*index) != actual_lines.get(*index))?;
    Some((
        first + 1,
        cap_text(expected_lines.get(first).copied().unwrap_or_default()),
        cap_text(actual_lines.get(first).copied().unwrap_or_default()),
    ))
}

fn diagnose_object_keys(
    expected: &serde_json::Map<String, Value>,
    actual: &serde_json::Map<String, Value>,
    category: &str,
    prefix: &str,
) -> Vec<Diagnostic> {
    let mut keys = BTreeSet::new();
    keys.extend(expected.keys().cloned());
    keys.extend(actual.keys().cloned());
    let mut diagnostics = Vec::new();
    for key in keys {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match (expected.get(&key), actual.get(&key)) {
            (Some(expected), None) => diagnostics.push(Diagnostic {
                category: category.to_string(),
                logical_path: path,
                first_expected_line: None,
                expected: json_compact(expected),
                actual: String::new(),
                detail: "key missing from actual output".to_string(),
            }),
            (None, Some(actual)) => diagnostics.push(Diagnostic {
                category: category.to_string(),
                logical_path: path,
                first_expected_line: None,
                expected: String::new(),
                actual: json_compact(actual),
                detail: "extra key in actual output".to_string(),
            }),
            (Some(expected), Some(actual)) if expected != actual => diagnostics.push(Diagnostic {
                category: category.to_string(),
                logical_path: path,
                first_expected_line: None,
                expected: json_compact(expected),
                actual: json_compact(actual),
                detail: "key value differs".to_string(),
            }),
            (Some(_), Some(_)) | (None, None) => {}
        }
    }
    diagnostics
}

fn json_compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string())
}

fn inventory_identity(record: &InventoryRecord) -> (String, String, i32) {
    (
        record.name.clone(),
        record.domain_role.clone(),
        record.priority,
    )
}

fn inventory_record_text(record: &InventoryRecord) -> String {
    format!(
        "{} {} {} {} {}",
        record.name, record.domain_role, record.priority, record.uri, record.display_name
    )
}

fn unified_diff(expected: &str, actual: &str) -> String {
    let expected_lines = expected.lines().collect::<Vec<_>>();
    let actual_lines = actual.lines().collect::<Vec<_>>();
    let first = (0..expected_lines.len().max(actual_lines.len()))
        .find(|index| expected_lines.get(*index) != actual_lines.get(*index));
    let Some(first) = first else {
        return String::new();
    };
    let start = first.saturating_sub(3);
    let end = (first + 4).min(expected_lines.len().max(actual_lines.len()));
    let mut output = String::from("--- expected\n+++ actual\n");
    for index in start..end {
        match (expected_lines.get(index), actual_lines.get(index)) {
            (Some(expected), Some(actual)) if expected == actual => {
                output.push_str(&format!(" {expected}\n"));
            }
            (Some(expected), Some(actual)) => {
                output.push_str(&format!("-{expected}\n+{actual}\n"));
            }
            (Some(expected), None) => output.push_str(&format!("-{expected}\n")),
            (None, Some(actual)) => output.push_str(&format!("+{actual}\n")),
            (None, None) => {}
        }
    }
    cap_text(&output)
}

fn cap_text(value: &str) -> String {
    const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
    if value.len() <= MAX_DIAGNOSTIC_BYTES {
        value.to_string()
    } else {
        const MARKER: &[u8] = b"\n[output truncated]\n";
        let prefix_len = MAX_DIAGNOSTIC_BYTES.saturating_sub(MARKER.len());
        let mut capped = value.as_bytes()[..prefix_len].to_vec();
        capped.extend_from_slice(MARKER);
        String::from_utf8_lossy(&capped).into_owned()
    }
}

pub fn case_key(case: &CaseRecord) -> String {
    format!("{}/{}/{}", case.profile, case.source_set, case.case_id)
}

pub fn materialize_case_inputs(
    case: &CaseRecord,
    profile_root: &Path,
    destination: &Path,
) -> Result<(), String> {
    for record in &case.input_files {
        materialize_record(record, profile_root, destination)?;
    }
    Ok(())
}

pub fn materialize_case_expected(
    case: &CaseRecord,
    profile_root: &Path,
    destination: &Path,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut tree = BTreeMap::new();
    for record in &case.files {
        let bytes = read_record(record, profile_root)?;
        write_logical_file(destination, &record.logical_path, &bytes)?;
        tree.insert(record.logical_path.clone(), bytes);
    }
    Ok(tree)
}

pub fn read_record(record: &FileRecord, profile_root: &Path) -> Result<Vec<u8>, String> {
    let path = profile_root.join(native_relative_path(&record.storage_path));
    fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))
}

pub fn write_logical_file(root: &Path, logical_path: &str, bytes: &[u8]) -> Result<(), String> {
    validate_relative_path(logical_path, "logical_path")?;
    let path = root.join(native_relative_path(logical_path));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    fs::write(&path, bytes).map_err(|error| format!("write {}: {error}", path.display()))
}

pub fn walk_tree(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut tree = BTreeMap::new();
    if !root.exists() {
        return Ok(tree);
    }
    walk_tree_inner(root, root, &mut tree)?;
    Ok(tree)
}

fn walk_tree_inner(
    root: &Path,
    current: &Path,
    tree: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    for entry in
        fs::read_dir(current).map_err(|error| format!("walk {}: {error}", current.display()))?
    {
        let entry = entry.map_err(|error| format!("read directory entry: {error}"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("metadata {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("symlink in output tree: {}", path.display()));
        }
        if metadata.is_dir() {
            walk_tree_inner(root, &path, tree)?;
        } else if metadata.is_file() {
            let logical_path = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            let bytes =
                fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
            tree.insert(logical_path, bytes);
        }
    }
    Ok(())
}

pub fn mismatch_diagnostics(logical_path: &str, expected: &[u8], actual: &[u8]) -> Vec<Diagnostic> {
    mismatch_diagnostics_with_source_root(logical_path, expected, actual, None)
}

pub fn mismatch_diagnostics_with_source_root(
    logical_path: &str,
    expected: &[u8],
    actual: &[u8],
    source_root: Option<&Path>,
) -> Vec<Diagnostic> {
    if compare_file(logical_path, expected, actual, source_root).is_empty() {
        return Vec::new();
    }
    let mut diagnostics = if logical_path.ends_with(".html") {
        let expected = normalize_crlf(expected);
        let actual = replace_source_root_token(&normalize_crlf(actual), source_root, false);
        match (std::str::from_utf8(&expected), std::str::from_utf8(&actual)) {
            (Ok(expected), Ok(actual)) => vec![diagnose_html(expected, actual)],
            _ => compare_file(
                logical_path,
                expected.as_slice(),
                actual.as_slice(),
                source_root,
            ),
        }
    } else if logical_path == "searchindex.js" {
        let actual = replace_source_root_token(actual, source_root, true);
        match (
            parse_json_value(expected, true),
            parse_json_value(&actual, true),
        ) {
            (Ok(expected), Ok(actual)) => diagnose_searchindex(&expected, &actual),
            _ => compare_file(logical_path, expected, &actual, source_root),
        }
    } else if logical_path.rsplit('/').next() == Some("needs.json") {
        match (
            parse_json_value(expected, false),
            parse_json_value(actual, false),
        ) {
            (Ok(expected), Ok(actual)) => diagnose_needs_json(&expected, &actual),
            _ => compare_file(logical_path, expected, actual, source_root),
        }
    } else if logical_path == "objects.inv" {
        match (parse_inventory(expected), parse_inventory(actual)) {
            (Ok((expected_header, expected)), Ok((actual_header, actual)))
                if expected_header == actual_header =>
            {
                let diagnostics = diagnose_inventory(&expected, &actual);
                if diagnostics.is_empty() {
                    compare_file(
                        logical_path,
                        expected_header.as_slice(),
                        actual_header.as_slice(),
                        source_root,
                    )
                } else {
                    diagnostics
                }
            }
            _ => compare_file(logical_path, expected, actual, source_root),
        }
    } else {
        compare_file(logical_path, expected, actual, source_root)
    };
    for diagnostic in &mut diagnostics {
        diagnostic.logical_path = logical_path.to_string();
    }
    diagnostics
}

pub fn warning_diagnostics(
    expected: &[u8],
    actual: &[u8],
    expected_source_root: Option<&Path>,
    actual_source_root: Option<&Path>,
) -> Vec<Diagnostic> {
    warning_diagnostics_with_roots(
        expected,
        actual,
        expected_source_root.map(|source_root| WarningRoots {
            source_root: Some(source_root),
            output_root: None,
            doctree_root: None,
            case_root: None,
        }),
        actual_source_root.map(|source_root| WarningRoots {
            source_root: Some(source_root),
            output_root: None,
            doctree_root: None,
            case_root: None,
        }),
    )
}

pub fn warning_diagnostics_with_roots(
    expected: &[u8],
    actual: &[u8],
    expected_roots: Option<WarningRoots<'_>>,
    actual_roots: Option<WarningRoots<'_>>,
) -> Vec<Diagnostic> {
    if compare_warnings_with_roots(expected, actual, expected_roots, actual_roots).is_empty() {
        Vec::new()
    } else {
        diagnose_warnings(
            &normalize_warning_bytes_with_roots(expected, expected_roots),
            &normalize_warning_bytes_with_roots(actual, actual_roots),
        )
    }
}

pub fn needs_builder_diagnostic(
    expected_status: CaseStatus,
    actual_status: CaseStatus,
    actual_tree_empty: bool,
) -> Option<Diagnostic> {
    if expected_status == CaseStatus::Built
        && actual_status == CaseStatus::BuildError
        && actual_tree_empty
    {
        Some(Diagnostic {
            category: "needs-builder".to_string(),
            logical_path: "needs.json".to_string(),
            first_expected_line: None,
            expected: "built".to_string(),
            actual: "build-error".to_string(),
            detail: "Ultra rejected the needs builder before producing output".to_string(),
        })
    } else {
        None
    }
}

fn materialize_record(
    record: &FileRecord,
    profile_root: &Path,
    destination: &Path,
) -> Result<(), String> {
    let bytes = read_record(record, profile_root)?;
    write_logical_file(destination, &record.logical_path, &bytes)
}

fn native_relative_path(value: &str) -> PathBuf {
    value
        .split('/')
        .fold(PathBuf::new(), |mut path, component| {
            path.push(component);
            path
        })
}
