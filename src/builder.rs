use anyhow::Result;
use log::{debug, info};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cache::BuildCache;
use crate::config::BuildConfig;
use crate::document::Document;
use crate::error::{BuildErrorReport, BuildWarning, ErrorType};
use crate::extensions::{ExtensionLoader, SphinxApp};
use crate::matching;
use crate::parser::Parser;
use crate::utils;

/// A single toctree entry with its real source position.
#[derive(Debug, Clone)]
struct ToctreeEntry {
    /// The target as written (title stripped, angle-bracket target extracted).
    target: String,
    /// 1-based line number of the entry in its source file.
    line: usize,
    /// True when the containing toctree has `:glob:` and the target contains
    /// glob metacharacters.
    is_glob: bool,
}

/// Resolve a toctree target against the document that references it, the way
/// Sphinx does: a leading `/` means source-root-relative, anything else is
/// relative to the referencing document's directory. `.`/`..` segments are
/// normalized.
fn resolve_docname(target: &str, referencing_doc: &str) -> String {
    let (base, target) = if let Some(stripped) = target.strip_prefix('/') {
        ("", stripped)
    } else {
        (
            referencing_doc
                .rsplit_once('/')
                .map(|(d, _)| d)
                .unwrap_or(""),
            target,
        )
    };

    let mut segments: Vec<&str> = Vec::new();
    for seg in base.split('/').chain(target.split('/')) {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    segments.join("/")
}

#[derive(Debug, Clone)]
pub struct BuildStats {
    pub files_processed: usize,
    pub files_skipped: usize,
    pub build_time: Duration,
    pub output_size_mb: f64,
    pub cache_hits: usize,
    pub errors: usize,
    pub warnings: usize,
    pub warning_details: Vec<BuildWarning>,
    pub error_details: Vec<BuildErrorReport>,
}

pub struct SphinxBuilder {
    config: BuildConfig,
    source_dir: PathBuf,
    output_dir: PathBuf,
    cache: BuildCache,
    parser: Parser,
    parallel_jobs: usize,
    incremental: bool,
    warnings: Arc<Mutex<Vec<BuildWarning>>>,
    errors: Arc<Mutex<Vec<BuildErrorReport>>>,
    #[allow(dead_code)]
    sphinx_app: Option<SphinxApp>,
    #[allow(dead_code)]
    extension_loader: ExtensionLoader,
}

impl SphinxBuilder {
    pub fn new(config: BuildConfig, source_dir: PathBuf, output_dir: PathBuf) -> Result<Self> {
        // -d/doctree_dir relocates the cache (sphinx-build's doctree dir).
        let cache_dir = config
            .doctree_dir
            .clone()
            .unwrap_or_else(|| output_dir.join(".sphinx-ultra-cache"));
        // Any config change invalidates cached documents (they were rendered
        // under the old configuration).
        let config_fingerprint = blake3::hash(serde_json::to_string(&config)?.as_bytes())
            .to_hex()
            .to_string();
        let cache = BuildCache::new(
            cache_dir,
            config.max_cache_size_mb,
            config.cache_expiration_hours,
            &config_fingerprint,
        )?;

        // Canonicalize source_dir so it matches the canonicalized absolute paths
        // returned by matching::get_matching_files; without this, relative
        // --source paths (including the default ".") fail strip_prefix later.
        let source_dir = source_dir.canonicalize().unwrap_or(source_dir);

        let parser = Parser::new(&config)?;

        let parallel_jobs = config.parallel_jobs.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });

        // Initialize Sphinx app with extensions
        let mut sphinx_app = SphinxApp::new(config.clone())?;
        let mut extension_loader = ExtensionLoader::new()?;

        // Load configured extensions
        for extension_name in &config.extensions {
            match extension_loader.load_extension(extension_name) {
                Ok(extension) => {
                    if let Err(e) = sphinx_app.add_extension(extension) {
                        log::warn!("Failed to add extension '{}': {}", extension_name, e);
                    }
                }
                Err(e) => {
                    log::warn!("Failed to load extension '{}': {}", extension_name, e);
                }
            }
        }

        Ok(Self {
            config,
            source_dir,
            output_dir,
            cache,
            parser,
            parallel_jobs,
            incremental: false,
            warnings: Arc::new(Mutex::new(Vec::new())),
            errors: Arc::new(Mutex::new(Vec::new())),
            sphinx_app: Some(sphinx_app),
            extension_loader,
        })
    }

    pub fn set_parallel_jobs(&mut self, jobs: usize) {
        self.parallel_jobs = jobs;
    }

    pub fn enable_incremental(&mut self) {
        self.incremental = true;
    }

    /// Discard the saved environment before building (sphinx-build `-E`).
    pub fn fresh_env(&self) -> Result<()> {
        self.cache.clear()
    }

    /// Add a warning to the collection
    #[allow(dead_code)]
    pub fn add_warning(&self, warning: BuildWarning) {
        self.warnings.lock().unwrap().push(warning);
    }

    /// Add an error to the collection
    #[allow(dead_code)]
    pub fn add_error(&self, error: BuildErrorReport) {
        self.errors.lock().unwrap().push(error);
    }

    /// Check if warnings should be treated as errors
    #[allow(dead_code)]
    pub fn should_fail_on_warning(&self) -> bool {
        self.config.fail_on_warning
    }

    pub async fn clean(&self) -> Result<()> {
        if self.output_dir.exists() {
            tokio::fs::remove_dir_all(&self.output_dir).await?;
        }
        // A clean build must not reuse documents cached before the clean
        // (the on-disk cache lived inside the output dir we just removed).
        self.cache.clear()?;
        Ok(())
    }

    pub async fn build(&self) -> Result<BuildStats> {
        let start_time = Instant::now();
        info!("Starting build process...");

        // Ensure output directory exists
        tokio::fs::create_dir_all(&self.output_dir).await?;

        // Discover all source files
        let source_files = self.discover_source_files().await?;
        info!("Discovered {} source files", source_files.len());

        // Build dependency graph
        let dependency_graph = self.build_dependency_graph(&source_files).await?;
        debug!(
            "Built dependency graph with {} nodes",
            dependency_graph.len()
        );

        // Process files in dependency order
        let processed_docs = self
            .process_files_parallel(&source_files, &dependency_graph)
            .await?;

        // Validate documents and collect warnings/errors
        self.validate_documents(&processed_docs, &source_files)
            .await?;

        // Directive/role validation runs in every build unless disabled
        if self.config.validate_directives {
            self.validate_directives_and_roles(&processed_docs);
        }

        // Cross-reference validation is opt-in (-n/nitpicky): its heuristics
        // still false-positive on refs we cannot resolve yet (intersphinx,
        // python objects before the M5 sidecar).
        if self.config.nitpicky {
            self.validate_cross_references(&processed_docs)?;
        }

        // Generate cross-references and indices
        self.generate_indices(&processed_docs).await?;

        // Copy static assets
        self.copy_static_assets().await?;

        // Generate sitemap and search index
        self.generate_search_index(&processed_docs).await?;

        let build_time = start_time.elapsed();
        let output_size = utils::calculate_directory_size(&self.output_dir).await?;

        let warnings = self.warnings.lock().unwrap();
        let errors = self.errors.lock().unwrap();

        let stats = BuildStats {
            files_processed: processed_docs.len(),
            files_skipped: 0, // TODO: Track skipped files
            build_time,
            output_size_mb: output_size as f64 / 1024.0 / 1024.0,
            cache_hits: self.cache.hit_count(),
            errors: errors.len(),
            warnings: warnings.len(),
            warning_details: warnings.clone(),
            error_details: errors.clone(),
        };

        info!("Build completed in {:?}", build_time);
        Ok(stats)
    }

    async fn discover_source_files(&self) -> Result<Vec<PathBuf>> {
        // Use pattern-based file discovery like Sphinx
        let include_patterns = &self.config.include_patterns;
        let exclude_patterns = &self.config.exclude_patterns;

        // Add built-in exclude patterns for common build artifacts and hidden files
        let mut all_exclude_patterns = exclude_patterns.clone();
        all_exclude_patterns.extend_from_slice(&[
            "_build/**".to_string(),
            "__pycache__/**".to_string(),
            ".git/**".to_string(),
            ".svn/**".to_string(),
            ".hg/**".to_string(),
            ".*/**".to_string(), // Skip all hidden directories
            "Thumbs.db".to_string(),
            ".DS_Store".to_string(),
        ]);

        match matching::get_matching_files(
            &self.source_dir,
            include_patterns,
            &all_exclude_patterns,
        ) {
            // Sphinx's Project.discover keeps only files with a configured
            // source suffix, regardless of include_patterns
            Ok(files) => Ok(files
                .into_iter()
                .filter(|path| self.is_source_file(path))
                .collect()),
            Err(e) => {
                log::warn!(
                    "Pattern matching failed, falling back to simple discovery: {}",
                    e
                );
                // Fallback to old method if pattern matching fails
                let mut files = Vec::new();
                self.discover_files_sync(&self.source_dir, &mut files)?;
                Ok(files)
            }
        }
    }

    /// Fallback file discovery for when pattern matching fails
    fn discover_files_sync(&self, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                // Skip hidden directories and build artifacts
                if let Some(name) = path.file_name() {
                    if name.to_string_lossy().starts_with('.')
                        || name == "_build"
                        || name == "__pycache__"
                    {
                        continue;
                    }
                }

                self.discover_files_sync(&path, files)?;
            } else if self.is_source_file(&path) {
                files.push(path);
            }
        }
        Ok(())
    }

    /// Fallback method to check if a file is a source file (used as backup)
    fn is_source_file(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension() {
            matches!(ext.to_string_lossy().as_ref(), "rst" | "md" | "txt")
        } else {
            false
        }
    }

    async fn build_dependency_graph(
        &self,
        files: &[PathBuf],
    ) -> Result<HashMap<PathBuf, Vec<PathBuf>>> {
        let mut graph = HashMap::new();

        // For now, simple implementation - process files in alphabetical order
        // TODO: Parse files to find actual dependencies (includes, references, etc.)
        for file in files {
            graph.insert(file.clone(), Vec::new());
        }

        Ok(graph)
    }

    async fn process_files_parallel(
        &self,
        files: &[PathBuf],
        _dependency_graph: &HashMap<PathBuf, Vec<PathBuf>>,
    ) -> Result<Vec<Document>> {
        info!(
            "Processing {} files with {} parallel jobs",
            files.len(),
            self.parallel_jobs
        );

        // Configure rayon thread pool
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(self.parallel_jobs)
            .build()?;

        // One file failing must not abort the build: failures become
        // BuildErrorReports (and a non-zero exit) while the rest continue.
        let results: Vec<(PathBuf, Result<Document>)> = pool.install(|| {
            files
                .par_iter()
                .map(|file_path| (file_path.clone(), self.process_single_file(file_path)))
                .collect()
        });

        let mut documents = Vec::with_capacity(results.len());
        for (file_path, result) in results {
            match result {
                Ok(document) => documents.push(document),
                Err(e) => {
                    self.errors.lock().unwrap().push(BuildErrorReport::new(
                        file_path,
                        None,
                        format!("{e:#}"),
                        ErrorType::ParseError,
                    ));
                }
            }
        }

        Ok(documents)
    }

    fn process_single_file(&self, file_path: &Path) -> Result<Document> {
        let relative_path = file_path.strip_prefix(&self.source_dir)?;
        debug!("Processing file: {}", relative_path.display());

        // Check cache if incremental build is enabled. A cache hit still
        // writes the rendered output — skipping the write is how cached pages
        // went missing from the output tree.
        if self.incremental {
            if let Ok(cached_doc) = self.cache.get_document(file_path) {
                let file_mtime = utils::get_file_mtime(file_path)?;
                if cached_doc.source_mtime >= file_mtime && !cached_doc.html.is_empty() {
                    debug!("Using cached version of {}", relative_path.display());
                    let output_path = self.get_output_path(file_path)?;
                    if let Some(parent) = output_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&output_path, &cached_doc.html)?;
                    return Ok(cached_doc);
                }
            }
        }

        // Read and parse the file
        let content = std::fs::read_to_string(file_path)?;
        let mut document = self.parser.parse(file_path, &content)?;

        // Simple document rendering (placeholder)
        let rendered_html = format!(
            "<html><body>{}</body></html>",
            html_escape::encode_text(&document.content.to_string())
        );
        document.html = rendered_html;

        // Write output file
        let output_path = self.get_output_path(file_path)?;
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&output_path, &document.html)?;

        // Cache the document
        if self.incremental {
            self.cache.store_document(file_path, &document)?;
        }

        Ok(document)
    }

    fn get_output_path(&self, source_path: &Path) -> Result<PathBuf> {
        let relative_path = source_path.strip_prefix(&self.source_dir)?;
        let mut output_path = self.output_dir.join(relative_path);

        // Change extension to .html
        output_path.set_extension("html");

        Ok(output_path)
    }

    async fn generate_indices(&self, _documents: &[Document]) -> Result<()> {
        info!("Generating indices and cross-references");
        // TODO: Implement index generation
        Ok(())
    }

    async fn copy_static_assets(&self) -> Result<()> {
        info!("Copying static assets");

        // Create _static directory
        let static_output_dir = self.output_dir.join("_static");
        tokio::fs::create_dir_all(&static_output_dir).await?;

        // Copy built-in static assets - use relative path from binary location
        let exe_dir = std::env::current_exe()?
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Could not determine executable directory"))?
            .to_path_buf();

        // Try multiple possible locations for static assets
        let possible_static_dirs = [
            exe_dir.join("../static"),                      // Release build
            exe_dir.join("../../static"),                   // Debug build
            exe_dir.join("../../../static"),                // Deep build
            Path::new("rust-builder/static").to_path_buf(), // Local development
        ];

        let mut static_assets_copied = false;
        for builtin_static_dir in &possible_static_dirs {
            if builtin_static_dir.exists() {
                debug!("Found static assets at: {:?}", builtin_static_dir);
                for entry in std::fs::read_dir(builtin_static_dir)? {
                    let entry = entry?;
                    let file_path = entry.path();
                    if file_path.is_file() {
                        let file_name = file_path.file_name().unwrap();
                        let dest_path = static_output_dir.join(file_name);
                        tokio::fs::copy(&file_path, &dest_path).await?;
                        debug!("Copied static asset: {:?}", file_name);
                    }
                }
                static_assets_copied = true;
                break;
            }
        }

        if !static_assets_copied {
            debug!("No built-in static assets found, creating basic ones");
            // Create minimal CSS files if not found
            self.create_default_static_assets(&static_output_dir)
                .await?;
        }

        // Copy project-specific static assets
        let static_dirs = [
            self.source_dir.join("_static"),
            self.source_dir.join("_templates"),
        ];

        for static_dir in &static_dirs {
            if static_dir.exists() {
                let dest = self.output_dir.join(static_dir.file_name().unwrap());
                utils::copy_dir_recursive(static_dir, &dest).await?;
                debug!("Copied static directory: {:?}", static_dir);
            }
        }

        Ok(())
    }

    async fn create_default_static_assets(&self, static_dir: &Path) -> Result<()> {
        // Create basic pygments.css
        let pygments_css = include_str!("../static/pygments.css");
        tokio::fs::write(static_dir.join("pygments.css"), pygments_css).await?;

        // Create basic theme.css
        let theme_css = include_str!("../static/theme.css");
        tokio::fs::write(static_dir.join("theme.css"), theme_css).await?;

        // Create basic JavaScript files
        let jquery_js = include_str!("../static/jquery.js");
        tokio::fs::write(static_dir.join("jquery.js"), jquery_js).await?;

        let doctools_js = include_str!("../static/doctools.js");
        tokio::fs::write(static_dir.join("doctools.js"), doctools_js).await?;

        let sphinx_highlight_js = include_str!("../static/sphinx_highlight.js");
        tokio::fs::write(static_dir.join("sphinx_highlight.js"), sphinx_highlight_js).await?;

        debug!("Created default static assets");
        Ok(())
    }

    /// Root-relative docname (no extension, forward slashes) for a document.
    fn docname_of(&self, doc: &Document) -> String {
        let relative = doc
            .source_path
            .strip_prefix(&self.source_dir)
            .unwrap_or(&doc.source_path);
        relative
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/")
    }

    async fn validate_documents(
        &self,
        processed_docs: &[Document],
        _source_files: &[PathBuf],
    ) -> Result<()> {
        info!("Validating documents and checking for warnings...");

        let mut all_documents = HashSet::new();
        let mut toctree_refs: Vec<(PathBuf, String, ToctreeEntry)> = Vec::new();

        for doc in processed_docs {
            let docname = self.docname_of(doc);
            for entry in self.extract_toctree_references(doc) {
                toctree_refs.push((doc.source_path.clone(), docname.clone(), entry));
            }
            all_documents.insert(docname);
        }

        // Resolve every entry the way Sphinx does and warn on the misses.
        let mut referenced: HashSet<String> = HashSet::new();
        for (source_file, referencing_doc, entry) in &toctree_refs {
            let resolved = resolve_docname(&entry.target, referencing_doc);

            if entry.is_glob {
                let matches: Vec<String> = all_documents
                    .iter()
                    .filter(|d| matching::pattern_match(d, &resolved).unwrap_or(false))
                    .cloned()
                    .collect();
                if matches.is_empty() {
                    self.warnings
                        .lock()
                        .unwrap()
                        .push(BuildWarning::toctree_glob_no_match(
                            source_file.clone(),
                            Some(entry.line),
                            &entry.target,
                        ));
                } else {
                    referenced.extend(matches);
                }
            } else if all_documents.contains(&resolved) {
                referenced.insert(resolved);
            } else {
                self.warnings
                    .lock()
                    .unwrap()
                    .push(BuildWarning::missing_toctree_ref(
                        source_file.clone(),
                        Some(entry.line),
                        &resolved,
                    ));
            }
        }

        // Orphan check: exact membership of the resolved reference set.
        for doc in processed_docs {
            let docname = self.docname_of(doc);
            if docname == "index" {
                continue;
            }
            if !referenced.contains(&docname) {
                let warning = BuildWarning::orphaned_document(doc.source_path.clone());
                self.warnings.lock().unwrap().push(warning);
            }
        }

        let warning_count = self.warnings.lock().unwrap().len();
        info!("Validation completed. Found {} warnings", warning_count);

        Ok(())
    }

    /// Run the directive/role validation system over every RST document.
    ///
    /// Findings surface as build *warnings* (so `-W`/`-w` govern promotion);
    /// `Unknown` results stay silent — the built-in validators cover a
    /// fraction of real Sphinx, and reporting the rest would drown every
    /// real project in noise.
    fn validate_directives_and_roles(&self, processed_docs: &[Document]) {
        use crate::directives::validation::{
            DirectiveValidationResult, DirectiveValidationSystem, ParsedDirective, ParsedRole,
            RoleValidationResult, SourceLocation,
        };
        use crate::document::DocumentContent;

        let results: Vec<(Vec<BuildWarning>, usize)> = processed_docs
            .par_iter()
            .filter_map(|doc| {
                if !matches!(&doc.content, DocumentContent::RestructuredText(_)) {
                    return None;
                }

                let mut warnings = Vec::new();
                let mut unknown = 0usize;
                // Statistics make validate_* take &mut self, so each document
                // gets its own (cheap) system instance for the parallel pass.
                let mut system = DirectiveValidationSystem::new();
                // Since wave 3 the feed comes from the parse-time records
                // (M1-scanner-compatible tuples), not a raw re-scan.
                let file = doc.source_path.display().to_string();
                let directives: Vec<ParsedDirective> = doc
                    .directive_records
                    .iter()
                    .map(|r| ParsedDirective {
                        name: r.name.clone(),
                        arguments: r.arguments.clone(),
                        options: r.options.iter().cloned().collect(),
                        content: r.content.clone(),
                        location: SourceLocation {
                            file: file.clone(),
                            line: r.line as usize,
                            column: 0,
                        },
                    })
                    .collect();
                let roles: Vec<ParsedRole> = doc
                    .role_records
                    .iter()
                    .map(|r| ParsedRole {
                        name: r.name.clone(),
                        target: r.target.clone(),
                        display_text: r.display.clone(),
                        location: SourceLocation {
                            file: file.clone(),
                            line: r.line as usize,
                            column: 0,
                        },
                    })
                    .collect();

                for directive in &directives {
                    match system.validate_directive(directive) {
                        DirectiveValidationResult::Valid => {}
                        DirectiveValidationResult::Unknown => unknown += 1,
                        DirectiveValidationResult::Warning(msg)
                        | DirectiveValidationResult::Error(msg) => {
                            warnings.push(BuildWarning::new(
                                doc.source_path.clone(),
                                Some(directive.location.line),
                                msg,
                                crate::error::WarningType::Other,
                            ));
                        }
                    }
                }

                for role in &roles {
                    match system.validate_role(role) {
                        RoleValidationResult::Valid => {}
                        RoleValidationResult::Unknown => unknown += 1,
                        RoleValidationResult::Warning(msg) | RoleValidationResult::Error(msg) => {
                            warnings.push(BuildWarning::new(
                                doc.source_path.clone(),
                                Some(role.location.line),
                                msg,
                                crate::error::WarningType::Other,
                            ));
                        }
                    }
                }

                Some((warnings, unknown))
            })
            .collect();

        let mut unknown_total = 0usize;
        for (warnings, unknown) in results {
            unknown_total += unknown;
            for warning in warnings {
                self.add_warning(warning);
            }
        }
        if unknown_total > 0 {
            debug!(
                "{} directive/role occurrence(s) had no validator and were not checked",
                unknown_total
            );
        }
    }

    /// Nitpicky cross-reference validation (`-n`): resolve every `:doc:` and
    /// `:ref:` against the documents and labels this build actually produced,
    /// via the domain registry. Python-domain references are counted but not
    /// validated (no object inventory until the M5 sidecar) — silently
    /// reporting them broken would false-positive on every third-party ref.
    fn validate_cross_references(&self, processed_docs: &[Document]) -> Result<()> {
        use crate::document::DocumentContent;
        use crate::domains::rst::RstDomain;
        use crate::domains::{DomainRegistry, ReferenceType};

        // docutils label matching is case-insensitive AND whitespace-
        // collapsing (fully_normalize_name); the doctree labels arrive
        // already collapsed, so role targets must collapse too or
        // multi-space :ref: text false-positives (review finding 38/42).
        let normalize_label = |label: &str| {
            label
                .trim()
                .to_lowercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };

        let mut rst_domain = RstDomain::new();
        for doc in processed_docs {
            let docname = self.docname_of(doc);
            let location = crate::domains::ReferenceLocation {
                docname: docname.clone(),
                lineno: None,
                column: None,
                source_path: Some(doc.source_path.display().to_string()),
            };
            rst_domain.register_document(docname.clone(), doc.title.clone(), location.clone())?;

            // Explicit targets from the doctree (docutils-normalized names,
            // which are already lowercase).
            if matches!(&doc.content, DocumentContent::RestructuredText(_)) {
                for label in &doc.labels {
                    rst_domain.register_label(
                        normalize_label(&label.name),
                        "section".to_string(),
                        None,
                        docname.clone(),
                        crate::domains::ReferenceLocation {
                            lineno: Some(label.line),
                            ..location.clone()
                        },
                    )?;
                }
            }

            // Section anchors double as :ref: targets (autosectionlabel-style;
            // better than false-positives on every section reference).
            let mut stack: Vec<&crate::document::TocEntry> = doc.toc.iter().collect();
            while let Some(entry) = stack.pop() {
                stack.extend(entry.children.iter());
                rst_domain.register_section(
                    normalize_label(&entry.anchor),
                    entry.title.clone(),
                    docname.clone(),
                    crate::domains::ReferenceLocation {
                        lineno: Some(entry.line_number),
                        ..location.clone()
                    },
                )?;
            }
        }

        let mut registry = DomainRegistry::new();
        registry.register_domain(Box::new(rst_domain))?;

        let mut python_refs = 0usize;
        for doc in processed_docs {
            if !matches!(&doc.content, DocumentContent::RestructuredText(_)) {
                continue;
            }
            let docname = self.docname_of(doc);
            // Since wave 3: role occurrences come from the parse-time
            // records (all roles; unmapped ones are dropped exactly like
            // the M1 scanner's Custom type).
            for record in &doc.role_records {
                let ref_type = match record.name.as_str() {
                    "doc" => ReferenceType::Document,
                    "ref" => ReferenceType::Section,
                    "func" => ReferenceType::Function,
                    "class" => ReferenceType::Class,
                    "mod" => ReferenceType::Module,
                    "meth" => ReferenceType::Method,
                    "attr" => ReferenceType::Attribute,
                    "data" => ReferenceType::Data,
                    "exc" => ReferenceType::Exception,
                    _ => continue,
                };
                // External-reference heuristics (M1 parity): URL-ish doc
                // targets and known-stdlib python targets are skipped.
                let external = match &ref_type {
                    ReferenceType::Document => {
                        record.target.starts_with("http://")
                            || record.target.starts_with("https://")
                            || record.target.starts_with("file://")
                    }
                    ReferenceType::Function | ReferenceType::Class | ReferenceType::Module => [
                        "builtins.",
                        "typing.",
                        "collections.",
                        "pathlib.",
                        "os.",
                        "sys.",
                        "json.",
                        "re.",
                        "datetime.",
                        "urllib.",
                        "http.",
                    ]
                    .iter()
                    .any(|p| record.target.starts_with(p)),
                    _ => false,
                };
                if external {
                    continue;
                }
                match ref_type {
                    ReferenceType::Document | ReferenceType::Section => {
                        let target = if matches!(ref_type, ReferenceType::Document) {
                            // :doc: targets resolve like toctree entries:
                            // leading `/` is source-root-relative, else
                            // current-doc-relative.
                            resolve_docname(&record.target, &docname)
                        } else {
                            normalize_label(&record.target)
                        };
                        registry.add_cross_reference(crate::domains::CrossReference {
                            ref_type,
                            target,
                            display_text: record.display.clone(),
                            source_location: crate::domains::ReferenceLocation {
                                docname: docname.clone(),
                                lineno: Some(record.line as usize),
                                column: None,
                                source_path: Some(doc.source_path.display().to_string()),
                            },
                            is_external: false,
                        });
                    }
                    _ => python_refs += 1,
                }
            }
        }

        // Validate exactly once; stats/broken helpers re-validate internally.
        for result in registry.validate_all_references() {
            if result.is_valid {
                continue;
            }
            let reference = &result.reference;
            let message = match reference.ref_type {
                ReferenceType::Document => {
                    format!("unknown document: '{}'", reference.target)
                }
                ReferenceType::Section => {
                    format!("undefined label: '{}'", reference.target)
                }
                _ => continue,
            };
            let file = reference
                .source_location
                .source_path
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(&reference.source_location.docname));
            self.add_warning(BuildWarning::new(
                file,
                reference.source_location.lineno,
                message,
                crate::error::WarningType::BrokenCrossReference,
            ));
        }

        if python_refs > 0 {
            info!(
                "{} python-domain reference(s) not validated (no object inventory until M5)",
                python_refs
            );
        }

        Ok(())
    }

    /// Toctree entries from the parse-time records (wave 3: authored
    /// entries with their real per-entry line numbers; no raw re-scan).
    fn extract_toctree_references(&self, doc: &Document) -> Vec<ToctreeEntry> {
        let mut entries = Vec::new();
        for toctree in &doc.toctrees {
            for entry in &toctree.entries {
                // External URLs and the `self` keyword are valid entries
                // that do not reference source documents.
                if entry.target.starts_with("http://")
                    || entry.target.starts_with("https://")
                    || entry.target == "self"
                {
                    continue;
                }
                entries.push(ToctreeEntry {
                    target: entry.target.clone(),
                    line: entry.line as usize,
                    is_glob: toctree.glob && entry.target.contains(['*', '?', '[']),
                });
            }
        }
        entries
    }

    async fn generate_search_index(&self, _documents: &[Document]) -> Result<()> {
        info!("Generating search index");
        // TODO: Implement search index generation
        Ok(())
    }
}
