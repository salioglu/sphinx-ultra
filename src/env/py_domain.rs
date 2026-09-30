//! The `py` domain: object and module registration with Sphinx's
//! duplicate semantics — `PythonDomain.note_object` / `note_module` /
//! `clear_doc` / `merge_domaindata`
//! (`sphinx/domains/python/__init__.py:780-832` and `:744-757`) — and the
//! resolution half: [`find_obj`] / [`resolve_xref`] (`:855-994`) plus the
//! [`builtin_resolver`] missing-reference listener (`:1077-1098`).
//!
//! Registrations replay from the parse layer's records
//! ([`crate::rst::RegistryExport::py_objects`]/[`py_modules`]) inside
//! [`crate::env::std_domain::replay_registrations`]: in Sphinx every one of
//! these calls fires *while the directive runs*, so a document's py
//! duplicate warnings interleave with its std description/term duplicates
//! — and its reporter and logger records — in creation order, which each
//! record's `seq` carries — probe-verified against sphinx 9.1.0 (a doc
//! with an envvar duplicate at line 8, a py duplicate at line 15 and a
//! term duplicate at line 18 warns 8 → 15 → 18).
//! `PythonDomain` defines **no** `process_doc` hook at all, so the `py`
//! slot of `_DomainsContainer._process_doc` (dispatch order `c, changeset,
//! citation, cpp, index, js, math, py, rst, std`) contributes nothing of
//! its own.
//!
//! [`py_modules`]: crate::rst::RegistryExport::py_modules

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::env::std_domain::{source_path_of, DocumentSource};
use crate::env::BuildEnvironment;
use crate::error::{BuildWarning, WarningType};

/// Sphinx's `ObjectEntry` (`__init__.py:60-65`), keyed by fullname in
/// [`PyDomainData::objects`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PyObjectEntry {
    pub docname: String,
    pub node_id: String,
    pub objtype: String,
    /// `:canonical:` alias registrations carry `true`; resolve-time
    /// disambiguation prefers non-aliased entries, and the duplicate rules
    /// below treat aliased entries as overridable.
    pub aliased: bool,
}

/// Sphinx's `ModuleEntry` (`__init__.py:67-73`), keyed by module name in
/// [`PyDomainData::modules`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PyModuleEntry {
    pub docname: String,
    pub node_id: String,
    pub synopsis: String,
    pub platform: String,
    pub deprecated: bool,
}

/// Python-domain (`py`) registries: `domaindata['py']['objects']` and
/// `['modules']`.
///
/// INSERTION-ORDERED, not a plain `BTreeMap`: Sphinx's fuzzy resolution
/// pass iterates the objects dict in **insertion order**
/// (`__init__.py:901-908`) and ambiguity takes the FIRST match, with the
/// candidates listed in match order — lexicographic iteration would
/// diverge on both the resolved target and the warning bytes whenever
/// registration order isn't alphabetical. Registration order is the
/// docname-ordered merge, record order within a document — and Python
/// dict assignment on an existing key keeps the original insertion slot,
/// so every overwrite here is **in place** (probe: a `:canonical:` alias
/// registered between two real definitions keeps its middle slot after
/// the second definition overwrites it).
///
/// The side `*_index` maps give O(log n) exact lookup; they always name
/// the entry's position in the paired `Vec` and carry no information of
/// their own.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PyDomainData {
    /// fullname -> entry, in registration order.
    pub objects: Vec<(String, PyObjectEntry)>,
    /// fullname -> index into [`Self::objects`].
    pub objects_index: BTreeMap<String, usize>,
    /// modname -> entry, in registration order.
    pub modules: Vec<(String, PyModuleEntry)>,
    /// modname -> index into [`Self::modules`].
    pub modules_index: BTreeMap<String, usize>,
}

impl PyDomainData {
    /// `PythonDomain.note_object` (`__init__.py:780-813`). Returns the
    /// docname of the entry the caller must warn about — `Some` exactly
    /// when Sphinx's `logger.warning` fires. The aliased-vs-real matrix
    /// (each cell probe-verified against sphinx 9.1.0):
    ///
    /// | existing \ new | real                    | aliased                 |
    /// |----------------|-------------------------|-------------------------|
    /// | real           | warn + overwrite        | silent keep (no write)  |
    /// | aliased        | silent overwrite        | warn + overwrite        |
    ///
    /// Overwrites land **in place** (Python dict assignment keeps the
    /// original insertion slot).
    pub fn note_object(&mut self, name: &str, entry: PyObjectEntry) -> Option<String> {
        if let Some(&index) = self.objects_index.get(name) {
            let other = &self.objects[index].1;
            if !other.aliased && entry.aliased {
                // "The original definition is already registered" — the
                // alias is dropped without touching the real entry.
                return None;
            }
            // `other.aliased && !entry.aliased`: "The original definition
            // found. Override it!" — silently. Every other combination
            // falls through Sphinx's `else` and warns; both overwrite.
            let warn = (other.aliased == entry.aliased).then(|| other.docname.clone());
            self.objects[index].1 = entry;
            warn
        } else {
            self.objects_index
                .insert(name.to_string(), self.objects.len());
            self.objects.push((name.to_string(), entry));
            None
        }
    }

    /// `PythonDomain.note_module` (`__init__.py:819-832`) — an
    /// unconditional dict assignment: never warns, last value wins, an
    /// existing name keeps its insertion slot. (The duplicate-module
    /// *warning* comes from the `note_object(modname, 'module', ...)` call
    /// `PyModule.run` makes alongside this one.)
    pub fn note_module(&mut self, name: &str, entry: PyModuleEntry) {
        if let Some(&index) = self.modules_index.get(name) {
            self.modules[index].1 = entry;
        } else {
            self.modules_index
                .insert(name.to_string(), self.modules.len());
            self.modules.push((name.to_string(), entry));
        }
    }

    /// `PythonDomain.clear_doc` (`__init__.py:744-751`): drop every entry
    /// the document owns. Survivors keep their relative order — deleting
    /// from a Python dict never reorders what stays — and the indices are
    /// rebuilt to match.
    pub fn clear_doc(&mut self, docname: &str) {
        self.objects.retain(|(_, entry)| entry.docname != docname);
        self.modules.retain(|(_, entry)| entry.docname != docname);
        self.rebuild_indices();
    }

    /// `PythonDomain.merge_domaindata` (`__init__.py:753-757`): fold in
    /// `other`'s entries whose docname is in `docnames`, in `other`'s
    /// registration order. Like Sphinx's, this is a plain dict assignment
    /// per entry — no duplicate checks ("XXX check duplicates?"), an
    /// existing name is overwritten in place, a new one appended.
    pub fn merge(&mut self, other: &PyDomainData, docnames: &BTreeSet<String>) {
        for (name, entry) in &other.objects {
            if !docnames.contains(&entry.docname) {
                continue;
            }
            if let Some(&index) = self.objects_index.get(name) {
                self.objects[index].1 = entry.clone();
            } else {
                self.objects_index.insert(name.clone(), self.objects.len());
                self.objects.push((name.clone(), entry.clone()));
            }
        }
        for (name, entry) in &other.modules {
            if !docnames.contains(&entry.docname) {
                continue;
            }
            if let Some(&index) = self.modules_index.get(name) {
                self.modules[index].1 = entry.clone();
            } else {
                self.modules_index.insert(name.clone(), self.modules.len());
                self.modules.push((name.clone(), entry.clone()));
            }
        }
    }

    fn rebuild_indices(&mut self) {
        self.objects_index = self
            .objects
            .iter()
            .enumerate()
            .map(|(index, (name, _))| (name.clone(), index))
            .collect();
        self.modules_index = self
            .modules
            .iter()
            .enumerate()
            .map(|(index, (name, _))| (name.clone(), index))
            .collect();
    }
}

// ---------------------------------------------------------------------------
// Resolution ([PY §3.2/§3.3/§3.5])
// ---------------------------------------------------------------------------

/// `PythonDomain.object_types`' keys, in declaration order — the objtype
/// universe `find_obj` uses when `:any:`-style resolution passes no role
/// (`type is None` → `list(self.object_types)`).
const OBJECT_TYPES: &[&str] = &[
    "function",
    "data",
    "class",
    "exception",
    "method",
    "classmethod",
    "staticmethod",
    "attribute",
    "property",
    "type",
    "module",
];

/// `Domain.objtypes_for_role` for the py domain: the `_role2type` reverse
/// map `Domain.__init__` builds from `object_types` (each ObjType's roles,
/// appended in `object_types` declaration order). `None` for a role no
/// ObjType names — `deco` and `const` — which in refspecific search mode
/// disables the whole candidate walk *and* the fuzzy pass (probe:
/// `:py:deco:`.mydeco`` never resolves while `:py:deco:`pkg.mydeco``
/// does).
pub(crate) fn objtypes_for_role(role: &str) -> Option<&'static [&'static str]> {
    Some(match role {
        "func" => &["function"],
        "data" => &["data"],
        "class" => &["class", "exception", "type"],
        "exc" => &["class", "exception"],
        "meth" => &["method", "classmethod", "staticmethod"],
        "attr" => &["attribute", "property"],
        // The "secret role only for internal look-up" behind the
        // meth→property fallback.
        "_prop" => &["property"],
        "type" => &["type"],
        "mod" => &["module"],
        "obj" => OBJECT_TYPES,
        _ => return None,
    })
}

/// `Domain.role_for_objtype`: `_role2type`'s inverse — each ObjType's FIRST
/// role (`sphinx/domains/__init__.py`, `_type2role[name] = roles[0]`).
/// `None` is the defensive stand-in for an objtype no directive of ours can
/// register (Sphinx would raise concatenating `'py:' + None`).
pub(crate) fn role_for_objtype(objtype: &str) -> Option<&'static str> {
    Some(match objtype {
        "function" => "func",
        "data" => "data",
        "class" => "class",
        "exception" => "exc",
        "method" | "classmethod" | "staticmethod" => "meth",
        "attribute" | "property" => "attr",
        "type" => "type",
        "module" => "mod",
        _ => return None,
    })
}

/// `PythonDomain.find_obj` (`__init__.py:855-928`): find candidates for
/// `name`, perhaps using the given module/class context. Returns `(fullname,
/// entry)` pairs in match order.
///
/// - The `()` strip is the FIRST statement (`:868`), so every caller —
///   `resolve_xref`'s fallback retries and a future `resolve_any_xref` —
///   inherits it.
/// - **searchmode 0 (exact)**: `name` → `classname.name` → `modname.name` →
///   `modname.classname.name`, object type NOT checked; a `mod` role takes
///   only the bare-name match (`:913-915`) — which may be a non-module
///   object, since the type isn't checked.
/// - **searchmode 1 (refspecific)**: candidates gated on
///   [`objtypes_for_role`], reversed order `modname.classname.name` →
///   `modname.name` → `name`; only when every exact candidate failed, the
///   fuzzy pass collects each registered object whose fullname ends with
///   `.name` — iterating [`PyDomainData::objects`] in REGISTRATION order,
///   which is what makes the ambiguity warning's candidate list and the
///   first-match winner reproducible.
pub fn find_obj<'a>(
    data: &'a PyDomainData,
    modname: Option<&str>,
    classname: Option<&str>,
    name: &str,
    typ: Option<&str>,
    searchmode: u8,
) -> Vec<(String, &'a PyObjectEntry)> {
    // skip parens
    let name = name.strip_suffix("()").unwrap_or(name);
    if name.is_empty() {
        return Vec::new();
    }
    // Python truthiness: an empty modname/classname never joins a candidate.
    let modname = modname.filter(|m| !m.is_empty());
    let classname = classname.filter(|c| !c.is_empty());

    let entry_of = |fullname: &str| {
        data.objects_index
            .get(fullname)
            .map(|&index| &data.objects[index].1)
    };

    let newname: Option<String> = if searchmode == 1 {
        let objtypes = match typ {
            None => Some(OBJECT_TYPES),
            Some(role) => objtypes_for_role(role),
        };
        let Some(objtypes) = objtypes else {
            // A role with no objtypes matches nothing in this mode.
            return Vec::new();
        };
        let gated = |fullname: &str| {
            entry_of(fullname).is_some_and(|entry| objtypes.contains(&entry.objtype.as_str()))
        };
        let qualified = match (modname, classname) {
            (Some(modname), Some(classname)) => {
                Some(format!("{modname}.{classname}.{name}")).filter(|fullname| gated(fullname))
            }
            _ => None,
        };
        if qualified.is_some() {
            qualified
        } else if let Some(dotted) = modname
            .map(|modname| format!("{modname}.{name}"))
            .filter(|dotted| gated(dotted))
        {
            Some(dotted)
        } else if gated(name) {
            Some(name.to_string())
        } else {
            // "fuzzy" searching mode (`:901-908`), reached only when every
            // exact candidate failed.
            let searchname = format!(".{name}");
            return data
                .objects
                .iter()
                .filter(|(oname, entry)| {
                    oname.ends_with(&searchname) && objtypes.contains(&entry.objtype.as_str())
                })
                .map(|(oname, entry)| (oname.clone(), entry))
                .collect();
        }
    } else {
        // NOTE: searching for exact match, object type is not considered.
        if entry_of(name).is_some() {
            Some(name.to_string())
        } else if typ == Some("mod") {
            // only exact matches allowed for modules
            return Vec::new();
        } else {
            [
                classname.map(|classname| format!("{classname}.{name}")),
                modname.map(|modname| format!("{modname}.{name}")),
                match (modname, classname) {
                    (Some(modname), Some(classname)) => {
                        Some(format!("{modname}.{classname}.{name}"))
                    }
                    _ => None,
                },
            ]
            .into_iter()
            .flatten()
            .find(|candidate| entry_of(candidate).is_some())
        }
    };
    newname
        .map(|newname| {
            let entry = entry_of(&newname).expect("candidate was just found");
            vec![(newname, entry)]
        })
        .unwrap_or_default()
}

/// A resolved py cross-reference: what the resolver needs to build the
/// `reference` node `make_refnode` / `_make_module_refnode` would.
#[derive(Debug, PartialEq)]
pub struct PyXrefTarget<'a> {
    pub docname: &'a str,
    pub node_id: &'a str,
    /// `make_refnode`'s title: the matched fullname, or for modules
    /// `{name}[: {synopsis}][ (deprecated)][ ({platform})]` — deprecated
    /// BEFORE platform (`_make_module_refnode`, `:1039-1054`; probe: a
    /// module with all three shows
    /// `both: Some synopsis. (deprecated) (Unix, Windows)`).
    pub reftitle: String,
    /// A module target keeps the content node even when the pending_xref
    /// carries `pending_xref_condition` children (`:983-984` passes
    /// `contnode` straight through).
    pub is_module: bool,
}

/// `PythonDomain.resolve_xref` minus the node plumbing (`:930-994`): the
/// type-fallback retries, the ambiguity rule, and the module/object split.
/// Returns the target (None = dangling, silent here — the warning is the
/// resolver's) and the ambiguity warning to log, `type='ref',
/// subtype='python'` → `[ref.python]`, which fires even on a successful
/// resolution.
pub fn resolve_xref<'a>(
    data: &'a PyDomainData,
    modname: Option<&str>,
    classname: Option<&str>,
    reftype: &str,
    target: &str,
    searchmode: u8,
) -> (Option<PyXrefTarget<'a>>, Option<String>) {
    let retry = |typ: &str| find_obj(data, modname, classname, target, Some(typ), searchmode);
    let mut matches = retry(reftype);
    if matches.is_empty() && reftype == "class" {
        // fallback to data/attr (for type aliases)
        matches = retry("data");
        if matches.is_empty() {
            matches = retry("attr");
        }
    }
    if matches.is_empty() && reftype == "attr" {
        // fallback to meth (for property; Sphinx 2.4.x)
        matches = retry("meth");
    }
    if matches.is_empty() && reftype == "meth" {
        // fallback to attr (for property), via the secret `_prop` role.
        matches = retry("_prop");
    }

    if matches.is_empty() {
        return (None, None);
    }
    let mut warning = None;
    let (name, entry) = if matches.len() > 1 {
        let canonicals: Vec<&(String, &PyObjectEntry)> =
            matches.iter().filter(|(_, entry)| !entry.aliased).collect();
        if canonicals.len() == 1 {
            // Exactly one non-aliased match wins silently.
            let (name, entry) = canonicals[0];
            (name.clone(), *entry)
        } else {
            warning = Some(format!(
                "more than one target found for cross-reference {}: {}",
                crate::utils::py_repr_str(target),
                matches
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            // ... and the FIRST match (aliased or not) is used (`:981`).
            let (name, entry) = &matches[0];
            (name.clone(), *entry)
        }
    } else {
        let (name, entry) = matches.remove(0);
        (name, entry)
    };

    if entry.objtype == "module" {
        (module_xref_target(data, name), warning)
    } else {
        (
            Some(PyXrefTarget {
                docname: &entry.docname,
                node_id: &entry.node_id,
                reftitle: name,
                is_module: false,
            }),
            warning,
        )
    }
}

/// `_make_module_refnode`'s target (`:1039-1054`): the module entry with
/// the `{name}[: {synopsis}][ (deprecated)][ ({platform})]` reftitle —
/// deprecated BEFORE platform (probe: a module with all three shows
/// `both: Some synopsis. (deprecated) (Unix, Windows)`).
///
/// Sphinx reads `self.modules[name]` — a module *object* entry is only
/// ever written alongside its module entry (and cleared with it), so the
/// lookup cannot miss; `None` is the defensive stand-in for Sphinx's
/// would-be KeyError.
fn module_xref_target(data: &PyDomainData, name: String) -> Option<PyXrefTarget<'_>> {
    let &index = data.modules_index.get(&name)?;
    let module = &data.modules[index].1;
    let mut reftitle = name;
    if !module.synopsis.is_empty() {
        reftitle.push_str(": ");
        reftitle.push_str(&module.synopsis);
    }
    if module.deprecated {
        reftitle.push_str(" (deprecated)");
    }
    if !module.platform.is_empty() {
        reftitle.push_str(" (");
        reftitle.push_str(&module.platform);
        reftitle.push(')');
    }
    Some(PyXrefTarget {
        docname: &module.docname,
        node_id: &module.node_id,
        reftitle,
        is_module: true,
    })
}

/// `PythonDomain.resolve_any_xref` (`__init__.py:996-1037`): always
/// `find_obj(..., type=None, searchmode=1)`; when there are several
/// matches, aliased entries are skipped; a module match yields
/// `('py:mod', module_refnode)`, everything else
/// `('py:' + role_for_objtype(objtype), refnode)` — in `find_obj`'s match
/// order, which is what the generic any-resolver's first-wins rule and its
/// ambiguity candidate list run on.
pub fn resolve_any_xref<'a>(
    data: &'a PyDomainData,
    modname: Option<&str>,
    classname: Option<&str>,
    target: &str,
) -> Vec<(String, PyXrefTarget<'a>)> {
    let matches = find_obj(data, modname, classname, target, None, 1);
    let multiple = matches.len() > 1;
    let mut results = Vec::new();
    for (name, entry) in matches {
        if multiple && entry.aliased {
            // "Skip duplicated matches" (`:1013-1016`).
            continue;
        }
        if entry.objtype == "module" {
            if let Some(target) = module_xref_target(data, name) {
                results.push(("py:mod".to_string(), target));
            }
        } else if let Some(role) = role_for_objtype(&entry.objtype) {
            results.push((
                format!("py:{role}"),
                PyXrefTarget {
                    docname: &entry.docname,
                    node_id: &entry.node_id,
                    reftitle: name,
                    is_module: false,
                },
            ));
        }
    }
    results
}

// ---------------------------------------------------------------------------
// py-modindex ([PY §4]: `PythonModuleIndex.generate`, `__init__.py:620-717`)
// ---------------------------------------------------------------------------

/// One py-modindex row — Sphinx's 7-field `IndexEntry` NamedTuple
/// (`sphinx/domains/_index.py:17-52`). `subtype`: 0 = top-level module,
/// 1 = group head (a parent with listed submodules — possibly a dummy with
/// every other field empty), 2 = submodule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModindexEntry {
    pub name: String,
    pub subtype: u8,
    pub docname: String,
    pub anchor: String,
    pub extra: String,
    pub qualifier: String,
    pub descr: String,
}

/// One first-letter group of the module index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModindexGroup {
    pub letter: String,
    pub entries: Vec<ModindexEntry>,
}

/// `PythonModuleIndex.generate()`'s `(sorted_content, collapse)`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PyModindex {
    pub groups: Vec<ModindexGroup>,
    pub collapse: bool,
}

/// `PythonModuleIndex.generate` (`__init__.py:628-717`), verbatim:
/// `modindex_common_prefix` sorted longest-first (stable, so equal lengths
/// keep config order); modules sorted by `lower()` (stable over
/// registration order); the FIRST matching prefix is stripped (and
/// restored when it swallowed the whole name, clearing `stripped`);
/// letter buckets key on the first character of the *stripped* name,
/// lowercased; a submodule (`package != modname` on the stripped name)
/// gets subtype 2, promoting the bucket's previous entry to a group head
/// when it IS the parent, or inserting an all-empty dummy parent when no
/// `prev_modname.startswith(package)` entry preceded it; display names
/// keep the stripped prefix (`stripped + modname`); `collapse` iff
/// submodules outnumber top-levels; groups come out letter-sorted.
pub fn generate_modindex(data: &PyDomainData, common_prefix: &[String]) -> PyModindex {
    let mut ignores: Vec<&str> = common_prefix.iter().map(String::as_str).collect();
    ignores.sort_by_key(|prefix| std::cmp::Reverse(prefix.len()));

    let mut modules: Vec<(&str, &PyModuleEntry)> = data
        .modules
        .iter()
        .map(|(name, entry)| (name.as_str(), entry))
        .collect();
    modules.sort_by_key(|(name, _)| name.to_lowercase());

    let mut content: BTreeMap<String, Vec<ModindexEntry>> = BTreeMap::new();
    let mut prev_modname = String::new();
    let mut num_top_levels = 0usize;
    for (full_name, module) in &modules {
        let mut modname = *full_name;
        let mut stripped = "";
        for ignore in &ignores {
            if let Some(rest) = modname.strip_prefix(ignore) {
                modname = rest;
                stripped = ignore;
                break;
            }
        }
        // "we stripped the whole module name?"
        if modname.is_empty() {
            (modname, stripped) = (stripped, "");
        }

        // `modname[0].lower()` — Python would IndexError on a name that is
        // still empty (an empty module name cannot register here; the guard
        // is the defensive stand-in).
        let Some(first) = modname.chars().next() else {
            continue;
        };
        let entries = content
            .entry(first.to_lowercase().collect::<String>())
            .or_default();

        let package = modname.split('.').next().unwrap_or(modname);
        let subtype = if package != modname {
            // it's a submodule
            if prev_modname == package {
                // first submodule - make parent a group head
                if let Some(last) = entries.last_mut() {
                    last.subtype = 1;
                }
            } else if !prev_modname.starts_with(package) {
                // submodule without parent in list, add dummy entry
                entries.push(ModindexEntry {
                    name: format!("{stripped}{package}"),
                    subtype: 1,
                    docname: String::new(),
                    anchor: String::new(),
                    extra: String::new(),
                    qualifier: String::new(),
                    descr: String::new(),
                });
            }
            2
        } else {
            num_top_levels += 1;
            0
        };

        entries.push(ModindexEntry {
            name: format!("{stripped}{modname}"),
            subtype,
            docname: module.docname.clone(),
            anchor: module.node_id.clone(),
            extra: module.platform.clone(),
            qualifier: if module.deprecated {
                "Deprecated".to_string()
            } else {
                String::new()
            },
            descr: module.synopsis.clone(),
        });
        prev_modname = modname.to_string();
    }

    // "only collapse if number of toplevel modules is larger than number
    // of submodules".
    let collapse = modules.len() - num_top_levels < num_top_levels;

    PyModindex {
        // `sorted(content.items())`: BTreeMap iteration is byte order,
        // which equals Python's codepoint order for UTF-8 strings.
        groups: content
            .into_iter()
            .map(|(letter, entries)| ModindexGroup { letter, entries })
            .collect(),
        collapse,
    }
}

/// The `py_modindex` slice of the environment snapshot, mirroring
/// [`crate::env::genindex::snapshot`]'s serde shape.
pub fn modindex_snapshot(modindex: &PyModindex) -> serde_json::Value {
    serde_json::to_value(modindex).unwrap_or(serde_json::Value::Null)
}

/// The names `inspect.isclass(getattr(builtins, name, None))` accepts under
/// the pinned oracle toolchain (CPython 3.12, the interpreter every fixture
/// oracle is generated with): every built-in class, exceptions included —
/// plus `__loader__`, which getattr happily hands back
/// (`_frozen_importlib.BuiltinImporter` *is* a class). Sorted for
/// `binary_search`.
const BUILTIN_CLASSES: &[&str] = &[
    "ArithmeticError",
    "AssertionError",
    "AttributeError",
    "BaseException",
    "BaseExceptionGroup",
    "BlockingIOError",
    "BrokenPipeError",
    "BufferError",
    "BytesWarning",
    "ChildProcessError",
    "ConnectionAbortedError",
    "ConnectionError",
    "ConnectionRefusedError",
    "ConnectionResetError",
    "DeprecationWarning",
    "EOFError",
    "EncodingWarning",
    "EnvironmentError",
    "Exception",
    "ExceptionGroup",
    "FileExistsError",
    "FileNotFoundError",
    "FloatingPointError",
    "FutureWarning",
    "GeneratorExit",
    "IOError",
    "ImportError",
    "ImportWarning",
    "IndentationError",
    "IndexError",
    "InterruptedError",
    "IsADirectoryError",
    "KeyError",
    "KeyboardInterrupt",
    "LookupError",
    "MemoryError",
    "ModuleNotFoundError",
    "NameError",
    "NotADirectoryError",
    "NotImplementedError",
    "OSError",
    "OverflowError",
    "PendingDeprecationWarning",
    "PermissionError",
    "ProcessLookupError",
    "RecursionError",
    "ReferenceError",
    "ResourceWarning",
    "RuntimeError",
    "RuntimeWarning",
    "StopAsyncIteration",
    "StopIteration",
    "SyntaxError",
    "SyntaxWarning",
    "SystemError",
    "SystemExit",
    "TabError",
    "TimeoutError",
    "TypeError",
    "UnboundLocalError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "UnicodeError",
    "UnicodeTranslateError",
    "UnicodeWarning",
    "UserWarning",
    "ValueError",
    "Warning",
    "ZeroDivisionError",
    "__loader__",
    "bool",
    "bytearray",
    "bytes",
    "classmethod",
    "complex",
    "dict",
    "enumerate",
    "filter",
    "float",
    "frozenset",
    "int",
    "list",
    "map",
    "memoryview",
    "object",
    "property",
    "range",
    "reversed",
    "set",
    "slice",
    "staticmethod",
    "str",
    "super",
    "tuple",
    "type",
    "zip",
];

/// `_TYPING_ALL = frozenset(typing.__all__)` (`__init__.py:55`) under
/// CPython 3.12. Sorted for `binary_search`.
const TYPING_ALL: &[&str] = &[
    "AbstractSet",
    "Annotated",
    "Any",
    "AnyStr",
    "AsyncContextManager",
    "AsyncGenerator",
    "AsyncIterable",
    "AsyncIterator",
    "Awaitable",
    "BinaryIO",
    "ByteString",
    "Callable",
    "ChainMap",
    "ClassVar",
    "Collection",
    "Concatenate",
    "Container",
    "ContextManager",
    "Coroutine",
    "Counter",
    "DefaultDict",
    "Deque",
    "Dict",
    "Final",
    "ForwardRef",
    "FrozenSet",
    "Generator",
    "Generic",
    "Hashable",
    "IO",
    "ItemsView",
    "Iterable",
    "Iterator",
    "KeysView",
    "List",
    "Literal",
    "LiteralString",
    "Mapping",
    "MappingView",
    "Match",
    "MutableMapping",
    "MutableSequence",
    "MutableSet",
    "NamedTuple",
    "Never",
    "NewType",
    "NoReturn",
    "NotRequired",
    "Optional",
    "OrderedDict",
    "ParamSpec",
    "ParamSpecArgs",
    "ParamSpecKwargs",
    "Pattern",
    "Protocol",
    "Required",
    "Reversible",
    "Self",
    "Sequence",
    "Set",
    "Sized",
    "SupportsAbs",
    "SupportsBytes",
    "SupportsComplex",
    "SupportsFloat",
    "SupportsIndex",
    "SupportsInt",
    "SupportsRound",
    "TYPE_CHECKING",
    "Text",
    "TextIO",
    "Tuple",
    "Type",
    "TypeAlias",
    "TypeAliasType",
    "TypeGuard",
    "TypeVar",
    "TypeVarTuple",
    "TypedDict",
    "Union",
    "Unpack",
    "ValuesView",
    "assert_never",
    "assert_type",
    "cast",
    "clear_overloads",
    "dataclass_transform",
    "final",
    "get_args",
    "get_origin",
    "get_overloads",
    "get_type_hints",
    "is_typeddict",
    "no_type_check",
    "no_type_check_decorator",
    "overload",
    "override",
    "reveal_type",
    "runtime_checkable",
];

/// `builtin_resolver` (`__init__.py:1077-1098`), the py domain's
/// missing-reference listener at priority 900 — AFTER intersphinx's
/// default-priority (500) handler, so a builtin name that a loaded
/// inventory carries resolves externally instead of being silenced (probe:
/// `:py:class:`int`` with `int` in a mapped inventory renders the external
/// reference; `:py:class:`bool``, absent from it, is silenced).
///
/// `true` means "do not emit nitpicky warnings for built-in types": the
/// pending_xref is replaced by its content node with no reference wrapper
/// and no warning — for `class`/`obj` targeting `None`, and for
/// `class`/`obj`/`exc` targeting a `builtins` class or a `typing` name
/// (with one leading `typing.` removed).
pub fn builtin_resolver(reftype: &str, target: &str) -> bool {
    match reftype {
        "class" | "obj" if target == "None" => true,
        "class" | "obj" | "exc" => {
            BUILTIN_CLASSES.binary_search(&target).is_ok()
                || TYPING_ALL
                    .binary_search(&target.strip_prefix("typing.").unwrap_or(target))
                    .is_ok()
        }
        _ => false,
    }
}

/// Replay one document's py registrations from the parse layer's records —
/// the `note_module` + `note_object` calls `PyModule.run` and
/// `PyObject.add_target_and_index` made while the directives ran, which
/// our parse layer records instead (the module scope they read lives in
/// the parser's ref_context, and a `:no-typesetting:` object registers
/// itself and then vanishes from the tree).
///
/// Duplicate warnings join `warnings` keyed by the record's `seq` — its
/// place in the document's diagnostics stream, the merge key
/// [`crate::env::std_domain::replay_registrations`] uses for the glossary
/// and description replays too, because in Sphinx all three warning
/// streams are parse-time (see the module comment).
///
/// `note_module` runs before `note_object` for the whole record stream
/// where Sphinx alternates per directive; the two registries are disjoint
/// maps and `note_module` never warns, so the difference is unobservable.
pub(crate) fn collect_registrations(
    env: &mut BuildEnvironment,
    doc: &DocumentSource<'_>,
    warnings: &mut Vec<(u32, BuildWarning)>,
) {
    for record in &doc.registry.py_modules {
        env.py.note_module(
            &record.name,
            PyModuleEntry {
                docname: doc.docname.to_string(),
                node_id: record.node_id.clone(),
                synopsis: record.synopsis.clone(),
                platform: record.platform.clone(),
                deprecated: record.deprecated,
            },
        );
    }
    for record in &doc.registry.py_objects {
        let Some(other) = env.py.note_object(
            &record.fullname,
            PyObjectEntry {
                docname: doc.docname.to_string(),
                node_id: record.node_id.clone(),
                objtype: record.objtype.clone(),
                aliased: record.aliased,
            },
        ) else {
            continue;
        };
        warnings.push((
            record.seq,
            // [PY §5]: plain `logger.warning` with no type/subtype — no
            // `[category]` suffix, and no objtype in the text (unlike the
            // std domain's `duplicate {objtype} description`).
            BuildWarning::new(
                source_path_of(doc, record.source),
                Some(record.lineno as usize),
                format!(
                    "duplicate object description of {}, other instance in {}, \
                     use :no-index: for one of them",
                    record.fullname, other
                ),
                WarningType::DuplicateLabel,
            )
            .with_category(None),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::std_domain;
    use crate::rst::{parse_rst_full, ParseOptions};
    use std::path::PathBuf;

    fn entry(docname: &str, node_id: &str, objtype: &str, aliased: bool) -> PyObjectEntry {
        PyObjectEntry {
            docname: docname.to_string(),
            node_id: node_id.to_string(),
            objtype: objtype.to_string(),
            aliased,
        }
    }

    fn module_entry(docname: &str, node_id: &str) -> PyModuleEntry {
        PyModuleEntry {
            docname: docname.to_string(),
            node_id: node_id.to_string(),
            synopsis: String::new(),
            platform: String::new(),
            deprecated: false,
        }
    }

    /// `(fullname, docname, aliased)` of every object, in iteration order.
    fn object_rows(data: &PyDomainData) -> Vec<(&str, &str, bool)> {
        data.objects
            .iter()
            .map(|(name, e)| (name.as_str(), e.docname.as_str(), e.aliased))
            .collect()
    }

    fn assert_indices_consistent(data: &PyDomainData) {
        assert_eq!(data.objects_index.len(), data.objects.len());
        for (name, &index) in &data.objects_index {
            assert_eq!(&data.objects[index].0, name, "objects_index[{name}]");
        }
        assert_eq!(data.modules_index.len(), data.modules.len());
        for (name, &index) in &data.modules_index {
            assert_eq!(&data.modules[index].0, name, "modules_index[{name}]");
        }
    }

    // ---- find_obj ([PY §3.2]) ------------------------------------------

    /// The registration order used across the find_obj tests: entries are
    /// noted in the order given, never alphabetized.
    fn data_of(entries: &[(&str, &str)]) -> PyDomainData {
        let mut data = PyDomainData::default();
        for (name, objtype) in entries {
            data.note_object(name, entry("index", name, objtype, false));
        }
        data
    }

    fn names(matches: &[(String, &PyObjectEntry)]) -> Vec<String> {
        matches.iter().map(|(name, _)| name.clone()).collect()
    }

    /// Exact mode tries `name` → `classname.name` → `modname.name` →
    /// `modname.classname.name`, first hit wins, objtype never checked.
    #[test]
    fn exact_mode_walks_the_candidate_chain_in_spec_order() {
        let data = data_of(&[
            ("m.C.x", "method"),
            ("m.x", "function"),
            ("C.x", "method"),
            ("x", "function"),
        ]);
        let find = |modname: Option<&str>, classname: Option<&str>| {
            names(&find_obj(&data, modname, classname, "x", Some("func"), 0))
        };
        assert_eq!(find(Some("m"), Some("C")), vec!["x"], "bare name first");
        let partial = data_of(&[("m.C.x", "method"), ("m.x", "function"), ("C.x", "method")]);
        assert_eq!(
            names(&find_obj(
                &partial,
                Some("m"),
                Some("C"),
                "x",
                Some("func"),
                0
            )),
            vec!["C.x"],
            "then classname.name"
        );
        let partial = data_of(&[("m.C.x", "method"), ("m.x", "function")]);
        assert_eq!(
            names(&find_obj(
                &partial,
                Some("m"),
                Some("C"),
                "x",
                Some("func"),
                0
            )),
            vec!["m.x"],
            "then modname.name"
        );
        let partial = data_of(&[("m.C.x", "method")]);
        assert_eq!(
            names(&find_obj(
                &partial,
                Some("m"),
                Some("C"),
                "x",
                Some("func"),
                0
            )),
            vec!["m.C.x"],
            "then modname.classname.name"
        );
        assert!(
            find_obj(&partial, None, None, "x", Some("func"), 0).is_empty(),
            "no context, no prefix candidates"
        );
    }

    /// Exact mode never checks the objtype: a `func` role happily returns a
    /// class entry.
    #[test]
    fn exact_mode_ignores_the_object_type() {
        let data = data_of(&[("thing", "class")]);
        assert_eq!(
            names(&find_obj(&data, None, None, "thing", Some("func"), 0)),
            vec!["thing"]
        );
    }

    /// `type == 'mod'`: "only exact matches allowed for modules" — the
    /// prefix chain is cut off entirely (probe: `:py:mod:`sub`` under
    /// `.. py:currentmodule:: pkg` does NOT find `pkg.sub`). But the
    /// bare-name hit itself is still type-unchecked.
    #[test]
    fn mod_takes_only_the_bare_name_match() {
        let data = data_of(&[("pkg.sub", "module")]);
        assert!(find_obj(&data, Some("pkg"), None, "sub", Some("mod"), 0).is_empty());
        let shadowed = data_of(&[("sub", "function")]);
        assert_eq!(
            names(&find_obj(
                &shadowed,
                Some("pkg"),
                None,
                "sub",
                Some("mod"),
                0
            )),
            vec!["sub"],
            "the bare-name arm runs before the mod cutoff and skips no types"
        );
    }

    /// The `()` strip is find_obj's FIRST statement, so it applies in both
    /// modes and to every candidate shape.
    #[test]
    fn trailing_parens_are_stripped_before_any_lookup() {
        let data = data_of(&[("m.f", "function")]);
        assert_eq!(
            names(&find_obj(&data, Some("m"), None, "f()", Some("obj"), 0)),
            vec!["m.f"],
            "exact mode"
        );
        assert_eq!(
            names(&find_obj(&data, None, None, "f()", Some("obj"), 1)),
            vec!["m.f"],
            "refspecific mode (the fuzzy pass sees the stripped name)"
        );
        assert!(
            find_obj(&data, Some("m"), None, "()", Some("obj"), 0).is_empty(),
            "a name that is nothing but parens strips to empty and matches nothing"
        );
    }

    /// searchmode 1 walks `modname.classname.name` → `modname.name` →
    /// `name`, each gated on the role's objtypes.
    #[test]
    fn refspecific_mode_prefers_the_most_qualified_gated_candidate() {
        let data = data_of(&[
            ("meth", "function"),
            ("m.meth", "function"),
            ("m.C.meth", "method"),
        ]);
        assert_eq!(
            names(&find_obj(
                &data,
                Some("m"),
                Some("C"),
                "meth",
                Some("meth"),
                1
            )),
            vec!["m.C.meth"],
            "most qualified first"
        );
        assert_eq!(
            names(&find_obj(
                &data,
                Some("m"),
                Some("C"),
                "meth",
                Some("func"),
                1
            )),
            vec!["m.meth"],
            "the objtype gate skips m.C.meth for :func: and lands on m.meth"
        );
        assert_eq!(
            names(&find_obj(&data, None, None, "meth", Some("func"), 1)),
            vec!["meth"],
            "no context leaves the bare-name candidate"
        );
    }

    /// The fuzzy suffix scan runs ONLY when every exact candidate failed,
    /// and iterates in registration order.
    #[test]
    fn the_fuzzy_pass_is_gated_and_registration_ordered() {
        let data = data_of(&[
            ("zeta.same", "function"),
            ("alpha.same", "function"),
            ("beta.same", "class"),
        ]);
        assert_eq!(
            names(&find_obj(&data, None, None, "same", Some("func"), 1)),
            vec!["zeta.same", "alpha.same"],
            "registration order, objtype-filtered (beta.same is a class)"
        );
        let with_exact = data_of(&[("zeta.same", "function"), ("same", "function")]);
        assert_eq!(
            names(&find_obj(&with_exact, None, None, "same", Some("func"), 1)),
            vec!["same"],
            "an exact bare-name hit suppresses the fuzzy pass"
        );
        assert!(
            find_obj(&data, None, None, "ame", Some("func"), 1).is_empty(),
            "the scan matches '.name', never a bare substring"
        );
    }

    /// `objtypes_for_role` returns `None` for `deco`/`const`, which kills
    /// the whole refspecific search — no candidates, no fuzzy (probe:
    /// `:py:deco:`.mydeco`` dangles while `:py:deco:`pkg.mydeco``
    /// resolves through exact mode).
    #[test]
    fn roles_without_objtypes_match_nothing_in_refspecific_mode() {
        let data = data_of(&[("pkg.mydeco", "function")]);
        assert!(find_obj(&data, None, None, "mydeco", Some("deco"), 1).is_empty());
        assert_eq!(
            names(&find_obj(&data, None, None, "pkg.mydeco", Some("deco"), 0)),
            vec!["pkg.mydeco"]
        );
    }

    /// `type=None` (a future `:any:`) searches every objtype.
    #[test]
    fn a_none_type_searches_all_object_types() {
        let data = data_of(&[("m.thing", "attribute")]);
        assert_eq!(
            names(&find_obj(&data, None, None, "thing", None, 1)),
            vec!["m.thing"]
        );
    }

    // ---- resolve_xref ([PY §3.3]) --------------------------------------

    /// The type-fallback chains: class→data→attr, attr→meth, meth→_prop.
    #[test]
    fn resolve_xref_walks_the_type_fallback_chains() {
        let alias = data_of(&[("Alias", "data")]);
        let (found, warning) = resolve_xref(&alias, None, None, "class", "Alias", 0);
        assert_eq!(warning, None);
        assert_eq!(
            found,
            Some(PyXrefTarget {
                docname: "index",
                node_id: "Alias",
                reftitle: "Alias".to_string(),
                is_module: false,
            }),
            "a type alias documented as data resolves through :class:"
        );

        let attr_alias = data_of(&[("A.x", "attribute")]);
        let (found, _) = resolve_xref(&attr_alias, None, Some("A"), "class", "x", 0);
        assert!(found.is_some(), "class falls back to attr after data");

        let prop = data_of(&[("K.oldm", "method"), ("K.prop", "property")]);
        let (found, _) = resolve_xref(&prop, None, Some("K"), "attr", "oldm", 0);
        assert_eq!(found.unwrap().node_id, "K.oldm", "attr falls back to meth");
        let (found, _) = resolve_xref(&prop, None, Some("K"), "meth", "prop", 0);
        assert_eq!(
            found.unwrap().node_id,
            "K.prop",
            "meth falls back to property via the secret _prop role"
        );
    }

    /// Ambiguity: the warning carries the candidates comma-joined in match
    /// order and the FIRST match wins — the order-distinguishing case
    /// (zeta.same registered before alpha.same).
    #[test]
    fn ambiguity_warns_with_candidates_in_registration_order_and_takes_the_first() {
        let data = data_of(&[("zeta.same", "function"), ("alpha.same", "function")]);
        let (found, warning) = resolve_xref(&data, None, None, "func", "same", 1);
        assert_eq!(
            warning.as_deref(),
            Some("more than one target found for cross-reference 'same': zeta.same, alpha.same")
        );
        assert_eq!(found.unwrap().node_id, "zeta.same");
    }

    /// Exactly one non-aliased match is preferred silently; all-aliased (or
    /// several real) candidates warn.
    #[test]
    fn a_single_non_aliased_match_wins_silently() {
        let mut data = PyDomainData::default();
        data.note_object("alpha.f", entry("index", "alpha.f", "function", false));
        data.note_object("beta.f", entry("index", "alpha.f", "function", true));
        let (found, warning) = resolve_xref(&data, None, None, "func", "f", 1);
        assert_eq!(warning, None);
        assert_eq!(found.unwrap().reftitle, "alpha.f");
    }

    /// Module targets build the `_make_module_refnode` reftitle:
    /// `{name}[: {synopsis}][ (deprecated)][ ({platform})]` — deprecated
    /// BEFORE platform (probe `module_both`:
    /// `both: Some synopsis. (deprecated) (Unix, Windows)`).
    #[test]
    fn a_module_target_carries_the_full_reftitle() {
        let mut data = PyDomainData::default();
        data.note_object("both", entry("index", "module-both", "module", false));
        data.note_module(
            "both",
            PyModuleEntry {
                docname: "index".to_string(),
                node_id: "module-both".to_string(),
                synopsis: "Some synopsis.".to_string(),
                platform: "Unix, Windows".to_string(),
                deprecated: true,
            },
        );
        let (found, _) = resolve_xref(&data, None, None, "mod", "both", 0);
        assert_eq!(
            found,
            Some(PyXrefTarget {
                docname: "index",
                node_id: "module-both",
                reftitle: "both: Some synopsis. (deprecated) (Unix, Windows)".to_string(),
                is_module: true,
            })
        );
    }

    // ---- resolve_any_xref ([PY §3.4]) ----------------------------------

    /// `:any:` always searches refspecific with `type=None`: every objtype
    /// participates, and the result role is `py:` + the objtype's first
    /// role — probe `resolve_any_role` (f → py-func, m → py-mod).
    #[test]
    fn resolve_any_finds_functions_and_modules_with_their_roles() {
        let mut data = PyDomainData::default();
        data.note_object("m", entry("index", "module-m", "module", false));
        data.note_module("m", module_entry("index", "module-m"));
        data.note_object("m.f", entry("index", "m.f", "function", false));

        let f = resolve_any_xref(&data, Some("m"), None, "f");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].0, "py:func");
        assert_eq!(f[0].1.reftitle, "m.f");
        assert!(!f[0].1.is_module);

        let m = resolve_any_xref(&data, Some("m"), None, "m");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].0, "py:mod");
        assert_eq!(m[0].1.node_id, "module-m");
        assert!(m[0].1.is_module);

        // find_obj's `()` strip is inherited: `:any:`f()`` resolves.
        let parens = resolve_any_xref(&data, Some("m"), None, "f()");
        assert_eq!(parens.len(), 1);
        assert_eq!(parens[0].1.reftitle, "m.f");
    }

    /// Aliased entries are skipped when there is more than one match —
    /// and kept when they are the ONLY match.
    #[test]
    fn resolve_any_skips_aliased_entries_only_among_multiple_matches() {
        let mut data = PyDomainData::default();
        data.note_object("zeta.same", entry("index", "zeta.same", "function", false));
        data.note_object("beta.same", entry("index", "zeta.same", "function", true));
        data.note_object(
            "alpha.same",
            entry("index", "alpha.same", "function", false),
        );
        let results = resolve_any_xref(&data, None, None, "same");
        let names: Vec<&str> = results.iter().map(|(_, t)| t.reftitle.as_str()).collect();
        assert_eq!(
            names,
            vec!["zeta.same", "alpha.same"],
            "registration order, alias dropped"
        );

        let mut lone = PyDomainData::default();
        lone.note_object("old.name", entry("index", "new_name", "function", true));
        let only = resolve_any_xref(&lone, None, None, "name");
        assert_eq!(only.len(), 1, "a single aliased match is kept");
        assert_eq!(only[0].1.reftitle, "old.name");
    }

    /// A module candidate carries the full `_make_module_refnode` reftitle
    /// (the ambiguity warning renders it verbatim — probe:
    /// ``:py:mod:`syn: The syn module.``).
    #[test]
    fn resolve_any_module_candidates_carry_the_synopsis_reftitle() {
        let mut data = PyDomainData::default();
        data.note_object("syn", entry("index", "module-syn", "module", false));
        data.note_module(
            "syn",
            PyModuleEntry {
                docname: "index".to_string(),
                node_id: "module-syn".to_string(),
                synopsis: "The syn module.".to_string(),
                platform: String::new(),
                deprecated: false,
            },
        );
        let results = resolve_any_xref(&data, None, None, "syn");
        assert_eq!(results[0].1.reftitle, "syn: The syn module.");
    }

    // ---- generate_modindex ([PY §4]) -----------------------------------

    /// One probe dump row: `(name, subtype, docname, anchor, extra,
    /// qualifier, descr)`.
    type ModindexRow<'a> = (&'a str, u8, &'a str, &'a str, &'a str, &'a str, &'a str);

    /// The rows of each letter group, in the probe dumps' tuple shape.
    fn modindex_rows(modindex: &PyModindex) -> Vec<(&str, Vec<ModindexRow<'_>>)> {
        modindex
            .groups
            .iter()
            .map(|group| {
                (
                    group.letter.as_str(),
                    group
                        .entries
                        .iter()
                        .map(|e| {
                            (
                                e.name.as_str(),
                                e.subtype,
                                e.docname.as_str(),
                                e.anchor.as_str(),
                                e.extra.as_str(),
                                e.qualifier.as_str(),
                                e.descr.as_str(),
                            )
                        })
                        .collect(),
                )
            })
            .collect()
    }

    fn modindex_module(
        docname: &str,
        name: &str,
        synopsis: &str,
        platform: &str,
        deprecated: bool,
    ) -> PyModuleEntry {
        PyModuleEntry {
            docname: docname.to_string(),
            node_id: format!("module-{name}"),
            synopsis: synopsis.to_string(),
            platform: platform.to_string(),
            deprecated,
        }
    }

    /// The [PY §4] `modindex_shapes` probe, tuple-exact: lower()-sorted
    /// walk, parent promotion to subtype 1, the dummy `orphan` parent, and
    /// `collapse=False` (5 modules, 2 top-levels: 3 < 2 is false).
    #[test]
    fn modindex_shapes_reproduces_the_probe_tuples() {
        let mut data = PyDomainData::default();
        for (name, synopsis, platform, deprecated) in [
            ("pkg", "", "", false),
            ("pkg.sub", "Sub synopsis.", "", false),
            ("pkg.sub2", "", "Windows", false),
            ("orphan.child", "", "", false),
            ("zzz", "", "", true),
        ] {
            data.note_module(
                name,
                modindex_module("index", name, synopsis, platform, deprecated),
            );
        }
        let modindex = generate_modindex(&data, &[]);
        assert!(!modindex.collapse);
        assert_eq!(
            modindex_rows(&modindex),
            vec![
                (
                    "o",
                    vec![
                        ("orphan", 1, "", "", "", "", ""),
                        (
                            "orphan.child",
                            2,
                            "index",
                            "module-orphan.child",
                            "",
                            "",
                            ""
                        ),
                    ]
                ),
                (
                    "p",
                    vec![
                        ("pkg", 1, "index", "module-pkg", "", "", ""),
                        (
                            "pkg.sub",
                            2,
                            "index",
                            "module-pkg.sub",
                            "",
                            "",
                            "Sub synopsis."
                        ),
                        ("pkg.sub2", 2, "index", "module-pkg.sub2", "Windows", "", ""),
                    ]
                ),
                (
                    "z",
                    vec![("zzz", 0, "index", "module-zzz", "", "Deprecated", "")]
                ),
            ]
        );
    }

    /// The [PY §4] `modindex_common_prefix` probe: prefix-stripped modules
    /// keep their full display name but sort/bucket by the stripped name
    /// and count as top-level — `collapse=True` (3 − 3 = 0 < 3).
    #[test]
    fn modindex_common_prefix_strips_for_bucketing_but_displays_full_names() {
        let mut data = PyDomainData::default();
        for name in ["pkg.aaa", "pkg.bbb", "other"] {
            data.note_module(name, modindex_module("index", name, "", "", false));
        }
        let modindex = generate_modindex(&data, &["pkg.".to_string()]);
        assert!(modindex.collapse);
        assert_eq!(
            modindex_rows(&modindex),
            vec![
                (
                    "a",
                    vec![("pkg.aaa", 0, "index", "module-pkg.aaa", "", "", "")]
                ),
                (
                    "b",
                    vec![("pkg.bbb", 0, "index", "module-pkg.bbb", "", "", "")]
                ),
                ("o", vec![("other", 0, "index", "module-other", "", "", "")]),
            ]
        );
    }

    /// A prefix that swallows a whole module name is restored with
    /// `stripped` cleared, and the longest prefix wins (stable sort by
    /// length, descending) — probe `restore_and_longest`, tuple-exact.
    #[test]
    fn modindex_prefix_stripping_restores_emptied_names_and_prefers_longer() {
        let mut data = PyDomainData::default();
        for name in ["pkg", "pkgx", "pkg.deep.mod"] {
            data.note_module(name, modindex_module("index", name, "", "", false));
        }
        let modindex = generate_modindex(&data, &["pkg".to_string(), "pkg.deep.".to_string()]);
        assert_eq!(
            modindex_rows(&modindex),
            vec![
                (
                    "m",
                    vec![(
                        "pkg.deep.mod",
                        0,
                        "index",
                        "module-pkg.deep.mod",
                        "",
                        "",
                        ""
                    )]
                ),
                ("p", vec![("pkg", 0, "index", "module-pkg", "", "", "")]),
                ("x", vec![("pkgx", 0, "index", "module-pkgx", "", "", "")]),
            ],
            "pkg.deep.mod strips the longer prefix; pkg empties and restores \
             (bucketed under 'p', not dummy-parented); pkgx buckets under \
             its stripped 'x'"
        );
        assert!(modindex.collapse, "3 - 3 = 0 < 3");
    }

    // ---- builtin_resolver ([PY §3.5]) ----------------------------------

    #[test]
    fn builtin_resolver_matches_sphinxs_exact_gates() {
        // reftype {class, obj} + None.
        assert!(builtin_resolver("class", "None"));
        assert!(builtin_resolver("obj", "None"));
        assert!(
            !builtin_resolver("exc", "None"),
            "exc is not in the None gate"
        );
        // reftype {class, obj, exc} + builtins classes (exceptions included).
        assert!(builtin_resolver("class", "int"));
        assert!(builtin_resolver("obj", "bool"));
        assert!(builtin_resolver("exc", "ValueError"));
        assert!(builtin_resolver("class", "__loader__"), "getattr quirk");
        // typing names, bare or with ONE `typing.` prefix removed.
        assert!(builtin_resolver("class", "Sequence"));
        assert!(builtin_resolver("class", "typing.Sequence"));
        assert!(builtin_resolver("obj", "Optional"));
        assert!(
            !builtin_resolver("class", "typing.typing.Sequence"),
            "removeprefix strips one prefix only"
        );
        // Everything else warns.
        assert!(!builtin_resolver("class", "Missing"));
        assert!(!builtin_resolver("func", "int"), "func is never silenced");
        assert!(
            !builtin_resolver("data", "int"),
            "probe: :py:data:`int` warns"
        );
        assert!(
            !builtin_resolver("exc", "len"),
            "a builtin function is not a class"
        );
    }

    // ---- note_object matrix ([PY §5], each cell probe-verified) --------

    #[test]
    fn real_over_real_warns_and_the_last_definition_wins_in_place() {
        let mut py = PyDomainData::default();
        py.note_object("other", entry("a", "other", "function", false));
        assert_eq!(
            py.note_object("dup", entry("a", "dup", "function", false)),
            None
        );
        assert_eq!(
            py.note_object("dup", entry("b", "id0", "function", false)),
            Some("a".to_string()),
            "the second real definition warns naming the first's docname"
        );
        assert_eq!(
            object_rows(&py),
            vec![("other", "a", false), ("dup", "b", false)],
            "the overwrite lands in the original insertion slot"
        );
        assert_eq!(py.objects[py.objects_index["dup"]].1.node_id, "id0");
        assert_indices_consistent(&py);
    }

    #[test]
    fn an_alias_never_replaces_a_real_definition_and_stays_silent() {
        let mut py = PyDomainData::default();
        py.note_object("name", entry("a", "name", "function", false));
        assert_eq!(
            py.note_object("name", entry("b", "alias-id", "function", true)),
            None
        );
        assert_eq!(
            py.objects[py.objects_index["name"]].1,
            entry("a", "name", "function", false),
            "the real entry is untouched"
        );
    }

    #[test]
    fn a_real_definition_silently_overrides_an_alias_in_place() {
        let mut py = PyDomainData::default();
        py.note_object("first", entry("a", "first", "function", false));
        py.note_object("name", entry("a", "alias-id", "function", true));
        py.note_object("last", entry("a", "last", "function", false));
        assert_eq!(
            py.note_object("name", entry("b", "name", "function", false)),
            None,
            "\"The original definition found. Override it!\" — no warning"
        );
        assert_eq!(
            object_rows(&py),
            vec![
                ("first", "a", false),
                ("name", "b", false),
                ("last", "a", false)
            ],
            "the override keeps the alias's insertion slot"
        );
    }

    /// The fourth cell, probe-verified against sphinx 9.1.0: two
    /// `:canonical: shared.alias` registrations warn (`duplicate object
    /// description of shared.alias, other instance in index, use
    /// :no-index: for one of them`) and the later alias wins, keeping the
    /// original slot — `note_object` falls through to the warn+overwrite
    /// `else` whenever the aliased flags are equal.
    #[test]
    fn an_alias_over_an_alias_warns_and_overwrites_in_place() {
        let mut py = PyDomainData::default();
        py.note_object("new_a", entry("index", "new_a", "function", false));
        py.note_object("shared.alias", entry("index", "new_a", "function", true));
        py.note_object("new_b", entry("index", "new_b", "function", false));
        assert_eq!(
            py.note_object("shared.alias", entry("index", "new_b", "function", true)),
            Some("index".to_string())
        );
        assert_eq!(
            object_rows(&py),
            vec![
                ("new_a", "index", false),
                ("shared.alias", "index", true),
                ("new_b", "index", false),
            ]
        );
        assert_eq!(
            py.objects[py.objects_index["shared.alias"]].1.node_id,
            "new_b"
        );
    }

    // ---- ordering, clear_doc, merge, note_module -----------------------

    /// The registration-order contract T10's fuzzy pass builds on:
    /// iteration yields entries in the order they were first registered,
    /// never alphabetized.
    #[test]
    fn iteration_preserves_registration_order_not_lexicographic_order() {
        let mut py = PyDomainData::default();
        py.note_object("zeta.same", entry("a", "zeta.same", "function", false));
        py.note_object("alpha.same", entry("a", "alpha.same", "function", false));
        assert_eq!(
            py.objects
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            vec!["zeta.same", "alpha.same"]
        );
        assert_eq!(py.objects_index["zeta.same"], 0);
        assert_eq!(py.objects_index["alpha.same"], 1);
    }

    #[test]
    fn clear_doc_preserves_the_relative_order_of_survivors() {
        let mut py = PyDomainData::default();
        py.note_object("one", entry("a", "one", "function", false));
        py.note_object("two", entry("b", "two", "function", false));
        py.note_object("three", entry("a", "three", "class", false));
        py.note_object("four", entry("b", "four", "function", false));
        py.note_module("amod", module_entry("a", "module-amod"));
        py.note_module("bmod", module_entry("b", "module-bmod"));

        py.clear_doc("a");

        assert_eq!(
            object_rows(&py),
            vec![("two", "b", false), ("four", "b", false)]
        );
        assert_eq!(
            py.modules
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            vec!["bmod"]
        );
        assert_indices_consistent(&py);

        py.clear_doc("b");
        assert!(py.objects.is_empty() && py.modules.is_empty());
        assert!(py.objects_index.is_empty() && py.modules_index.is_empty());
    }

    #[test]
    fn merge_folds_only_the_named_docnames_in_registration_order() {
        let mut ours = PyDomainData::default();
        ours.note_object("kept", entry("a", "kept", "function", false));
        ours.note_object("both", entry("a", "both", "function", false));

        let mut theirs = PyDomainData::default();
        theirs.note_object("zeta", entry("b", "zeta", "function", false));
        theirs.note_object("both", entry("b", "id0", "function", false));
        theirs.note_object("skipped", entry("c", "skipped", "function", false));
        theirs.note_module("bmod", module_entry("b", "module-bmod"));
        theirs.note_module("cmod", module_entry("c", "module-cmod"));

        ours.merge(&theirs, &BTreeSet::from(["b".to_string()]));

        assert_eq!(
            object_rows(&ours),
            vec![
                ("kept", "a", false),
                // Dict assignment: the existing key keeps its slot, the
                // value is theirs. No duplicate warning — sphinx's
                // merge_domaindata performs none.
                ("both", "b", false),
                ("zeta", "b", false),
            ]
        );
        assert_eq!(
            ours.modules
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            vec!["bmod"]
        );
        assert_indices_consistent(&ours);
    }

    #[test]
    fn note_module_never_warns_and_the_last_entry_wins_in_place() {
        let mut py = PyDomainData::default();
        py.note_module("mod", module_entry("a", "module-mod"));
        py.note_module("other", module_entry("a", "module-other"));
        py.note_module(
            "mod",
            PyModuleEntry {
                docname: "b".to_string(),
                node_id: "module-0".to_string(),
                synopsis: "S".to_string(),
                platform: "P".to_string(),
                deprecated: true,
            },
        );
        assert_eq!(
            py.modules
                .iter()
                .map(|(n, e)| (n.as_str(), e.docname.as_str()))
                .collect::<Vec<_>>(),
            vec![("mod", "b"), ("other", "a")]
        );
        assert!(py.modules[py.modules_index["mod"]].1.deprecated);
    }

    // ---- the replay through std_domain::replay_registrations -----------

    fn parse(source: &str, docname: &str) -> crate::rst::ParseOutput {
        parse_rst_full(
            source,
            &ParseOptions {
                source_path: format!("<{docname}>"),
                sphinx: true,
                docname: docname.to_string(),
                found_docs: None,
                exclude_patterns: Vec::new(),
                py: Default::default(),
                srcdir: None,
                ..Default::default()
            },
        )
    }

    /// Fold sources into a fresh environment through the real per-document
    /// orchestration ([`std_domain::replay_registrations`], which replays
    /// the py records, then [`std_domain::process_doc`]) and return it with
    /// the warnings.
    fn read(sources: &[(&str, &str)]) -> (BuildEnvironment, Vec<BuildWarning>) {
        let mut env = BuildEnvironment::default();
        let mut warnings = Vec::new();
        let doc2path = |docname: &str| PathBuf::from(format!("/src/{docname}.rst"));
        for (docname, source) in sources {
            let parsed = parse(source, docname);
            let path = PathBuf::from(format!("/src/{docname}.rst"));
            let doc = DocumentSource {
                docname,
                doctree: &parsed.doctree,
                registry: &parsed.registry,
                path: &path,
            };
            warnings.extend(
                std_domain::replay_registrations(&mut env, &doc)
                    .into_iter()
                    .map(|(_, warning)| warning),
            );
            std_domain::process_doc(&mut env, &doc, &doc2path, &mut warnings);
        }
        (env, warnings)
    }

    /// [PY §5] `duplicate_functions` probe: the second definition's id
    /// falls back to `id0`, the warning names the document's own docname
    /// with the `:no-index:` hint and no category suffix, and the objects
    /// table keeps the LAST definition in the FIRST definition's slot.
    #[test]
    fn a_py_object_defined_twice_in_one_document_warns_with_the_sphinx_bytes() {
        let (env, warnings) = read(&[(
            "index",
            ".. py:function:: dup()\n\n.. py:function:: dup()\n",
        )]);
        assert_eq!(
            warnings.iter().map(|w| w.render()).collect::<Vec<_>>(),
            vec![
                "<index>:3: WARNING: duplicate object description of dup, \
                 other instance in index, use :no-index: for one of them"
            ]
        );
        assert_eq!(
            object_rows(&env.py),
            vec![("dup", "index", false)],
            "last definition wins"
        );
        assert_eq!(env.py.objects[0].1.node_id, "id0");
    }

    /// [PY §5] `duplicate_modules` probe: the module duplicate warns via
    /// its `note_object` half (line = the directive's own), while
    /// `note_module` silently records the second entry — both tables end
    /// on `module-0`.
    #[test]
    fn a_module_defined_twice_warns_once_and_both_tables_keep_the_second() {
        let (env, warnings) =
            read(&[("index", ".. py:module:: dupmod\n\n.. py:module:: dupmod\n")]);
        assert_eq!(
            warnings.iter().map(|w| w.render()).collect::<Vec<_>>(),
            vec![
                "<index>:3: WARNING: duplicate object description of dupmod, \
                 other instance in index, use :no-index: for one of them"
            ]
        );
        assert_eq!(
            env.py.objects[env.py.objects_index["dupmod"]].1,
            entry("index", "module-0", "module", false)
        );
        assert_eq!(
            env.py.modules[env.py.modules_index["dupmod"]].1,
            module_entry("index", "module-0")
        );
    }

    /// Cross-document duplicate: the warning fires from the second
    /// document, naming the first — byte-checked against a sphinx 9.1.0
    /// dummy build of this pair.
    #[test]
    fn a_py_duplicate_across_documents_names_the_other_docname() {
        let (env, warnings) = read(&[
            ("a", ".. py:function:: dup()\n"),
            ("b", "B\n=\n\n.. py:function:: dup()\n"),
        ]);
        assert_eq!(
            warnings.iter().map(|w| w.render()).collect::<Vec<_>>(),
            vec![
                "<b>:4: WARNING: duplicate object description of dup, \
                 other instance in a, use :no-index: for one of them"
            ]
        );
        assert_eq!(object_rows(&env.py), vec![("dup", "b", false)]);
    }

    /// §6 `canonical_function` probe: `:canonical:` registers a second
    /// entry under the canonical name with `aliased=True` and the same
    /// node id.
    #[test]
    fn canonical_registers_an_aliased_entry_with_the_same_node_id() {
        let (env, warnings) = read(&[(
            "index",
            ".. py:function:: new_name()\n   :canonical: old.name\n",
        )]);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            env.py.objects,
            vec![
                (
                    "new_name".to_string(),
                    entry("index", "new_name", "function", false)
                ),
                (
                    "old.name".to_string(),
                    entry("index", "new_name", "function", true)
                ),
            ]
        );
    }

    /// Cross-domain interleaving, probe-verified against sphinx 9.1.0 on
    /// this exact document built twice: an envvar duplicate (line 8), a py
    /// duplicate (line 15) and a term duplicate (line 18) warn in
    /// DOCUMENT order — all three registrations are parse-time in Sphinx,
    /// so no domain's stream comes out grouped.
    #[test]
    fn py_duplicate_warnings_interleave_with_std_s_in_document_order() {
        let document = "Probe\n=====\n\n\
                        .. envvar:: STDDUP\n\n\
                        .. py:function:: pydup()\n\n\
                        .. envvar:: STDDUP\n\n\
                        .. glossary::\n\n   \
                        gterm\n      First.\n\n\
                        .. py:function:: pydup()\n\n\
                        .. glossary::\n\n   \
                        gterm\n      Second.\n";
        let (_, warnings) = read(&[("index", document)]);
        assert_eq!(
            warnings
                .iter()
                .map(|warning| (warning.line, warning.message.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (
                    Some(8),
                    "duplicate envvar description of STDDUP, other instance in index"
                ),
                (
                    Some(15),
                    "duplicate object description of pydup, other instance in index, \
                     use :no-index: for one of them"
                ),
                (
                    Some(18),
                    "duplicate term description of gterm, other instance in index"
                ),
            ],
            "{warnings:?}"
        );
    }

    /// The std domain must not see any of this: a py-only document adds
    /// nothing to `env.std`, and a std-only document adds nothing to
    /// `env.py` — the guard for "no std behavior change" alongside the
    /// wiring this task added to `process_doc`.
    #[test]
    fn py_and_std_registrations_stay_in_their_own_registries() {
        let (env, warnings) = read(&[("index", ".. py:function:: func()\n\n.. envvar:: HOME\n")]);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(object_rows(&env.py), vec![("func", "index", false)]);
        assert_eq!(
            env.std.objects.keys().collect::<Vec<_>>(),
            vec![&("envvar".to_string(), "HOME".to_string())]
        );
        assert!(env
            .std
            .objects
            .keys()
            .all(|(objtype, _)| objtype != "function"));
    }
}
