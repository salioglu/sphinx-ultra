use anyhow::Result;
use log::{debug, info};
use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::cache::BuildCache;
use crate::config::BuildConfig;
use crate::doctree::Doctree;
use crate::document::Document;
use crate::env::toctree as env_toctree;
use crate::env::BuildEnvironment;
use crate::error::{BuildErrorReport, BuildWarning, ErrorType};
use crate::extensions::{ExtensionLoader, SphinxApp};
use crate::matching;
use crate::parser::Parser;
use crate::utils;

/// Subdirectory of the cache dir holding one bincode doctree per document.
/// It lives inside the `.config-fingerprint`-governed cache directory, so a
/// configuration change wipes these along with everything else.
const DOCTREE_SUBDIR: &str = "doctrees";

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
/// Sphinx does (`docname_join`): a leading `/` means source-root-relative,
/// anything else is relative to the referencing document's directory.
fn resolve_docname(target: &str, referencing_doc: &str) -> String {
    env_toctree::docname_join(referencing_doc, target)
}

/// One document's read-phase output.
///
/// This is the brief's `ReadResult { document, doctree, registry }` with the
/// registry riding `document.registry` instead of sitting beside it: a
/// cache hit skips parsing, so the only honest source for that document's
/// registry is the one persisted with the cached `Document` (see
/// [`Document::registry`]). Splitting it out would mean either a second
/// copy or an empty stand-in on every cache hit.
struct ReadResult {
    /// Root-relative docname (`docname_of_path`).
    docname: String,
    document: Document,
    doctree: Doctree,
    /// Read completion time in microseconds since the epoch — what Sphinx
    /// stores in `env.all_docs[docname]` (`builders/__init__.py:665`).
    read_time_us: u64,
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
    /// Persisted build state (toctree graph, section/figure numbering, std
    /// domain data, ...). Loaded from the cache dir's `env.bin` if present
    /// and current; otherwise a fresh, empty environment. The merge phase
    /// fills it and the resolve phase saves it back.
    ///
    /// It does not yet steer the build: every discovered document is read
    /// on every build, so the loaded environment is fully rebuilt rather
    /// than consulted for what is outdated. Reading it back is what a later
    /// wave-4 task's incremental-rebuild logic needs.
    env: BuildEnvironment,
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

        // Reuse whatever environment survived the fingerprint-wipe check
        // above (BuildCache::new already discarded it if the config
        // changed); a first build or an incompatible/corrupt env.bin both
        // fall back to a fresh, empty environment.
        let env = BuildEnvironment::load(cache.cache_dir()).unwrap_or_default();

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
            env,
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

    /// Run the build: read → merge → resolve → write, then validation.
    ///
    /// The four phases mirror Sphinx's own split (`builders/__init__.py`):
    ///
    /// - **read** ([`Self::read_phase`], parallel): parse every source file
    ///   into a `Document` + doctree, persisting the doctree per document.
    /// - **merge** ([`Self::merge_phase`], sequential, docname-ordered):
    ///   fold each document's read output into the [`BuildEnvironment`] —
    ///   Sphinx's `merge_info_from` plus the collectors it dispatches.
    /// - **resolve** ([`Self::resolve_phase`]): whole-project state that
    ///   needs every document read first, then persist the environment.
    /// - **write** ([`Self::write_phase`]): emit the output files.
    ///
    /// `&mut self` only so the environment can be moved out and back; the
    /// phase methods themselves take `&self` and the environment by
    /// reference.
    pub async fn build(&mut self) -> Result<BuildStats> {
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

        let read_results = self.read_phase(&source_files, &dependency_graph)?;

        let mut env = std::mem::take(&mut self.env);
        self.merge_phase(&mut env, &read_results);
        let resolved = self.resolve_phase(&mut env);
        self.env = env;
        resolved?;

        // Keep documents in discovery order (the merge phase iterates a
        // docname-sorted view of its own): the write and validation phases
        // below produce warnings in this order, which is user-visible.
        let processed_docs: Vec<Document> = read_results
            .into_iter()
            .map(|result| result.document)
            .collect();

        self.write_phase(&processed_docs);

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

    /// Read phase: parse every source file in parallel.
    ///
    /// One file failing must not abort the build: failures become
    /// `BuildErrorReport`s (and a non-zero exit) while the rest continue.
    /// Results keep the discovery order of `files`.
    fn read_phase(
        &self,
        files: &[PathBuf],
        _dependency_graph: &HashMap<PathBuf, Vec<PathBuf>>,
    ) -> Result<Vec<ReadResult>> {
        info!(
            "Processing {} files with {} parallel jobs",
            files.len(),
            self.parallel_jobs
        );

        // Sphinx's `env.found_docs`: known before any file is parsed, and
        // needed *during* the parse so `toctree` entries resolve.
        let found_docs = Arc::new(
            files
                .iter()
                .map(|path| self.docname_of_path(path))
                .collect::<BTreeSet<String>>(),
        );

        // Configure rayon thread pool
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(self.parallel_jobs)
            .build()?;

        let results: Vec<(PathBuf, Result<ReadResult>)> = pool.install(|| {
            files
                .par_iter()
                .map(|file_path| {
                    (
                        file_path.clone(),
                        self.read_one_file(file_path, &found_docs),
                    )
                })
                .collect()
        });

        let mut read_results = Vec::with_capacity(results.len());
        for (file_path, result) in results {
            match result {
                Ok(read) => read_results.push(read),
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

        Ok(read_results)
    }

    fn read_one_file(
        &self,
        file_path: &Path,
        found_docs: &Arc<BTreeSet<String>>,
    ) -> Result<ReadResult> {
        let relative_path = file_path.strip_prefix(&self.source_dir)?;
        debug!("Processing file: {}", relative_path.display());
        let docname = self.docname_of_path(file_path);

        // Check cache if incremental build is enabled. A cache hit skips the
        // parse but must still produce a usable doctree, and the write phase
        // still writes the page — skipping the write is how cached pages
        // went missing from the output tree.
        if self.incremental {
            if let Ok(file_mtime) = utils::get_file_mtime(file_path) {
                let hit = self.cache.get_document_with(file_path, |cached| {
                    if cached.source_mtime < file_mtime || cached.html.is_empty() {
                        return None;
                    }
                    // A cached document whose doctree file is gone or
                    // unreadable is not usable: the environment layer needs
                    // that doctree, and inventing an empty one would quietly
                    // drop the document's toc, titles and toctrees. Re-parse
                    // instead — this counts as a cache miss.
                    self.load_doctree(&docname)
                });
                if let Some((document, doctree)) = hit {
                    debug!("Using cached version of {}", relative_path.display());
                    return Ok(ReadResult {
                        docname,
                        document,
                        doctree,
                        // A cache hit is hash-validated against the file on
                        // disk, so what it holds is as current as a re-read:
                        // this document was (re)established now.
                        read_time_us: now_micros(),
                    });
                }
            }
        }

        // Read and parse the file
        let content = std::fs::read_to_string(file_path)?;
        let parsed =
            self.parser
                .parse_full(file_path, &content, &docname, Some(Arc::clone(found_docs)))?;
        let mut document = parsed.document;

        // Simple document rendering (placeholder). Done here, in the read
        // phase, because the rendered HTML is part of what the incremental
        // cache stores; the write phase only puts it on disk.
        let rendered_html = format!(
            "<html><body>{}</body></html>",
            html_escape::encode_text(&document.content.to_string())
        );
        document.html = rendered_html;

        // Every successful parse persists its doctree, whether this is a
        // first build or a re-parse after a rejected cache entry.
        self.store_doctree(&docname, &parsed.doctree)?;

        // Cache the document
        if self.incremental {
            self.cache.store_document(file_path, &document)?;
        }

        Ok(ReadResult {
            docname,
            document,
            doctree: parsed.doctree,
            read_time_us: now_micros(),
        })
    }

    /// Merge phase: fold the read phase's per-document output into the
    /// environment, in docname order.
    ///
    /// Sequential and deterministic, mirroring Sphinx's `merge_info_from`
    /// (`environment/__init__.py:421`) and the collectors it dispatches to:
    /// `all_docs`, the title collector, and the toctree collector.
    fn merge_phase(&self, env: &mut BuildEnvironment, results: &[ReadResult]) {
        // Documents that vanished since the saved environment was written
        // (Sphinx's `removed` set) must not leave stale state behind.
        let present: HashSet<&str> = results.iter().map(|r| r.docname.as_str()).collect();
        let stale: Vec<String> = env
            .all_docs
            .keys()
            .filter(|docname| !present.contains(docname.as_str()))
            .cloned()
            .collect();
        for docname in stale {
            env.clear_doc(&docname);
        }

        let mut ordered: Vec<&ReadResult> = results.iter().collect();
        ordered.sort_by(|a, b| a.docname.cmp(&b.docname));

        for result in ordered {
            let docname = result.docname.as_str();
            // Every document is re-read on every build today, so each one's
            // environment state is rebuilt from scratch — without this, the
            // `extend`-shaped fields (toctree_includes) would accumulate
            // duplicates across incremental builds.
            env.clear_doc(docname);

            env.all_docs
                .insert(docname.to_string(), result.read_time_us);

            let title = env_toctree::document_title(&result.doctree);
            // Sphinx's longtitle differs from the title only for documents
            // carrying an explicit `title` attribute, which nothing produces
            // yet (`collectors/title.py:27`).
            env.longtitles.insert(docname.to_string(), title.clone());
            env.titles.insert(docname.to_string(), title);

            let (toc, num_entries) = env_toctree::build_toc(&result.doctree, docname);
            // Each toctree node copied into the toc is noted, in the order
            // it was copied (which is the order Sphinx notes them in).
            for toctree in env_toctree::toctree_copies(&toc) {
                env_toctree::note_toctree(env, docname, toctree);
            }
            env.tocs.insert(docname.to_string(), toc);
            env.toc_num_entries.insert(docname.to_string(), num_entries);
        }
    }

    /// Resolve phase: whole-project state that only exists once every
    /// document has been read, then persist the environment.
    ///
    /// Sphinx computes document relations, section/figure numbering and
    /// domain cross-references here; those land in later wave-4 tasks. What
    /// this phase does today is save the environment the merge phase built,
    /// which is Sphinx's own end-of-read-phase step
    /// (`builders/__init__.py:420`).
    fn resolve_phase(&self, env: &mut BuildEnvironment) -> Result<()> {
        info!("Resolving build environment");
        env.save(self.cache.cache_dir())
    }

    /// Write phase: emit every document's rendered output.
    ///
    /// Sequential, and after resolution, so that a page can be written with
    /// whole-project knowledge (numbering, relations) once those exist. The
    /// placeholder renderer only uses the `Document`, so moving the write
    /// here leaves the bytes unchanged.
    ///
    /// One page failing to write must not abort the build (the same rule the
    /// read phase follows): it becomes a `BuildErrorReport`, and a non-zero
    /// exit, while the remaining pages are still written.
    fn write_phase(&self, documents: &[Document]) {
        for document in documents {
            if let Err(e) = self.write_one(document) {
                self.errors.lock().unwrap().push(BuildErrorReport::new(
                    document.source_path.clone(),
                    None,
                    format!("{e:#}"),
                    ErrorType::Other,
                ));
            }
        }
    }

    fn write_one(&self, document: &Document) -> Result<()> {
        let output_path = self.get_output_path(&document.source_path)?;
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&output_path, &document.html)?;
        Ok(())
    }

    /// `<cache_dir>/doctrees/<blake3(docname)>.doctree`.
    fn doctree_path(&self, docname: &str) -> PathBuf {
        let hash = blake3::hash(docname.as_bytes());
        self.cache
            .cache_dir()
            .join(DOCTREE_SUBDIR)
            .join(format!("{}.doctree", hash.to_hex()))
    }

    fn store_doctree(&self, docname: &str, doctree: &Doctree) -> Result<()> {
        let path = self.doctree_path(docname);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, crate::doctree::to_bincode(doctree))?;
        Ok(())
    }

    /// The persisted doctree for `docname`, or `None` if it is missing or
    /// cannot be decoded (a truncated write, or a blob from another version
    /// of this crate).
    fn load_doctree(&self, docname: &str) -> Option<Doctree> {
        let path = self.doctree_path(docname);
        let bytes = std::fs::read(&path).ok()?;
        match crate::doctree::from_bincode(&bytes) {
            Ok(doctree) => Some(doctree),
            Err(e) => {
                debug!(
                    "Ignoring unreadable doctree {}: {e:#} (re-reading {docname})",
                    path.display()
                );
                None
            }
        }
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
        self.docname_of_path(&doc.source_path)
    }

    /// Root-relative docname (no extension, forward slashes) for a source
    /// path. `docname_of` delegates here; this variant exists for callers
    /// (the parse step) that only have a path, not yet a [`Document`].
    fn docname_of_path(&self, path: &Path) -> String {
        let relative = path.strip_prefix(&self.source_dir).unwrap_or(path);
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

    /// The environment this build produced (empty before [`Self::build`]).
    pub fn env(&self) -> &BuildEnvironment {
        &self.env
    }

    /// [`BuildEnvironment::snapshot`] of this build's environment — the
    /// shape the `env_differential` oracle compares against.
    pub fn snapshot_env(&self) -> serde_json::Value {
        self.env.snapshot()
    }
}

/// Wall-clock microseconds since the Unix epoch, the unit Sphinx stores in
/// `env.all_docs` (`time.time_ns() // 1_000`). A pre-epoch clock yields 0
/// rather than wrapping.
fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_project(source_dir: &Path) {
        std::fs::create_dir_all(source_dir).unwrap();
        std::fs::write(
            source_dir.join("index.rst"),
            "Index\n=====\n\n.. toctree::\n\n   a\n",
        )
        .unwrap();
        std::fs::write(source_dir.join("a.rst"), "A\n=\n\nBody.\n").unwrap();
    }

    fn build_incrementally(source_dir: &Path, output_dir: &Path) -> (BuildStats, SphinxBuilder) {
        let mut builder = SphinxBuilder::new(
            BuildConfig::default(),
            source_dir.to_path_buf(),
            output_dir.to_path_buf(),
        )
        .unwrap();
        builder.enable_incremental();
        let stats = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(builder.build())
            .unwrap();
        (stats, builder)
    }

    #[test]
    fn every_read_document_persists_its_doctree() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (_stats, builder) = build_incrementally(&source_dir, &output_dir);

        for docname in ["index", "a"] {
            let path = builder.doctree_path(docname);
            assert!(
                path.is_file(),
                "{docname}: no doctree at {}",
                path.display()
            );
            let doctree = builder.load_doctree(docname).expect("doctree decodes");
            assert_eq!(doctree.root.kind, crate::doctree::kinds::DOCUMENT);
        }
        assert!(
            builder.cache.cache_dir().join("env.bin").is_file(),
            "the resolve phase must persist the environment"
        );
    }

    #[test]
    fn cache_hit_whose_doctree_is_missing_is_treated_as_a_miss() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (first, builder) = build_incrementally(&source_dir, &output_dir);
        assert_eq!(first.cache_hits, 0, "cold build cannot hit the cache");

        // Warm: both documents come from the cache.
        let (warm, _) = build_incrementally(&source_dir, &output_dir);
        assert_eq!(warm.cache_hits, 2);

        // Delete one document's doctree. Its cache entry is still valid, but
        // unusable — the build must re-read the file rather than pretend.
        std::fs::remove_file(builder.doctree_path("a")).unwrap();
        let (degraded, rebuilt) = build_incrementally(&source_dir, &output_dir);
        assert_eq!(
            degraded.cache_hits, 1,
            "a document whose doctree is gone is a cache miss, not a hit"
        );
        assert!(
            rebuilt.doctree_path("a").is_file(),
            "the re-read must persist the doctree it just produced"
        );
        assert_eq!(degraded.errors, 0);

        // And the environment is complete either way.
        let env = rebuilt.env();
        assert_eq!(env.all_docs.len(), 2);
        assert!(env.tocs.contains_key("a"));
    }

    #[test]
    fn corrupt_doctree_file_is_treated_as_a_miss() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (_cold, builder) = build_incrementally(&source_dir, &output_dir);
        std::fs::write(builder.doctree_path("index"), b"not a doctree").unwrap();

        let (stats, rebuilt) = build_incrementally(&source_dir, &output_dir);
        assert_eq!(stats.cache_hits, 1);
        assert_eq!(stats.errors, 0);
        assert!(rebuilt.load_doctree("index").is_some());
    }

    #[test]
    fn cache_hit_still_writes_output_and_fills_the_environment() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (_cold, _) = build_incrementally(&source_dir, &output_dir);
        std::fs::remove_file(output_dir.join("index.html")).unwrap();
        std::fs::remove_file(output_dir.join("a.html")).unwrap();

        let (warm, builder) = build_incrementally(&source_dir, &output_dir);

        assert_eq!(warm.cache_hits, 2);
        assert!(output_dir.join("index.html").is_file());
        assert!(output_dir.join("a.html").is_file());
        assert_eq!(
            builder.env().toctree_includes.get("index"),
            Some(&vec!["a".to_string()]),
            "a fully cached build still rebuilds the environment from the \
             persisted doctrees"
        );
    }
}
