use anyhow::Result;
use log::{debug, info};
use rayon::prelude::*;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::cache::BuildCache;
use crate::config::BuildConfig;
use crate::doctree::Doctree;
use crate::document::Document;
use crate::env;
use crate::env::dependencies as env_dependencies;
use crate::env::genindex as env_genindex;
use crate::env::metadata as env_metadata;
use crate::env::numbers as env_numbers;
use crate::env::resolve as env_resolve;
use crate::env::std_domain as env_std;
use crate::env::toctree as env_toctree;
use crate::env::toctree::{ConsistencyLevel, ToctreeWarningKind};
use crate::env::BuildEnvironment;
use crate::error::{BuildErrorReport, BuildWarning, ErrorType, WarningType};
use crate::extensions::{ExtensionLoader, SphinxApp};
use crate::intersphinx::{self, HttpConfig, Intersphinx, LoadRequest, UreqFetcher};
use crate::matching;
use crate::parser::Parser;
use crate::utils;

/// Subdirectory of the cache dir holding one bincode doctree per document.
/// It lives inside the `.config-fingerprint`-governed cache directory, so a
/// configuration change wipes these along with everything else.
const DOCTREE_SUBDIR: &str = "doctrees";

/// Magic prefix identifying a persisted doctree file ("sphinx-ultra
/// doctree"). Together with [`DOCTREE_FORMAT_VERSION`] it forms the 8-byte
/// header [`SphinxBuilder::store_doctree`] writes ahead of the bincode blob.
const DOCTREE_MAGIC: &[u8; 4] = b"SUDT";

/// Format version of the per-document doctree files.
///
/// The blob itself is bincode, which has no field-presence framing and no
/// self-description: bytes written by an older build decode *successfully*
/// into a plausible-looking doctree and are then silently mis-read. This
/// word is what turns that into an honest cache miss
/// ([`SphinxBuilder::load_doctree`] returns `None` for a missing or
/// mismatched version, and the document is re-read).
///
/// Bump it whenever previously written blobs would be misread, namely:
/// - the serialized shape changes — a field added to or removed from
///   `Doctree`/`Node`/`Attrs`, a different `AttrValue` variant set, or a
///   different bincode configuration;
/// - the *meaning* of what the parser stores changes while the shape does
///   not. Wave 4's index-entry attribute moving from `AttrValue::Str` to
///   `AttrValue::List` is the worked example: both variants decode, and an
///   old blob then harvests the wrong index entries.
const DOCTREE_FORMAT_VERSION: u32 = 1;

/// Bytes of the [`DOCTREE_MAGIC`] + [`DOCTREE_FORMAT_VERSION`] header.
const DOCTREE_HEADER_LEN: usize = DOCTREE_MAGIC.len() + std::mem::size_of::<u32>();

/// Sphinx's `root_doc` default (`config.py`), used when the configuration
/// leaves it unset.
const DEFAULT_ROOT_DOC: &str = "index";

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
    ///
    /// `None` for a document this build did *not* read: its rendered output
    /// and its doctree came back from the cache, and everything it
    /// contributed to the environment — its read time included — is
    /// whatever the build that did read it left behind.
    read_time_us: Option<u64>,
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
    /// and current; otherwise a fresh, empty environment.
    ///
    /// It steers the build: [`BuildEnvironment::get_outdated_files`] decides
    /// which documents this build reads, the merge phase folds those (and
    /// only those) back in, and the resolve phase saves it again. A document
    /// that is not read keeps every contribution the build that read it
    /// made.
    env: BuildEnvironment,
    /// docname -> the pseudo-XML of that document's *resolved* doctree, as
    /// the resolve phase left it (Sphinx's `get_and_resolve_doctree`
    /// output). Kept for [`Self::snapshot_env`], which is what the
    /// environment-oracle differential diffs; the write phase does not
    /// consume doctrees yet.
    resolved: Mutex<BTreeMap<String, String>>,
    /// The general index this build assembled (`IndexEntries.create_index`),
    /// kept beside [`Self::resolved`] for the same reason: it is build
    /// output derived from the environment plus the builder's own uri
    /// scheme, not environment state.
    genindex: Mutex<Vec<env_genindex::IndexGroup>>,
    /// The cross-project inventories `intersphinx_mapping` names, loaded
    /// once per build. Empty (and inert) unless a mapping is configured.
    intersphinx: Intersphinx,
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
            resolved: Mutex::new(BTreeMap::new()),
            genindex: Mutex::new(Vec::new()),
            intersphinx: Intersphinx::default(),
        })
    }

    /// Read every inventory `intersphinx_mapping` names — Sphinx's
    /// `load_mappings`, which it runs at `builder-inited`, before the read
    /// phase (`ext/intersphinx/__init__.py:80`).
    ///
    /// Local inventory locations are resolved against the source directory
    /// and re-read on every build; remote ones go through [`UreqFetcher`]
    /// and are cached under the (fingerprint-wiped) cache directory, so a
    /// configuration change discards them along with everything else.
    /// Fails where Sphinx raises `ConfigError` from `load_mappings` — an
    /// entry that survived normalisation but violates
    /// `_IntersphinxProject`'s invariants — which aborts the build with the
    /// same config-error exit code an invalid mapping gets at config time.
    fn load_intersphinx_inventories(&mut self) -> Result<()> {
        if self.config.intersphinx_mapping.is_empty() {
            return Ok(());
        }
        let http = HttpConfig {
            tls_verify: self.config.tls_verify,
            tls_cacerts: self.config.tls_cacerts.clone(),
            user_agent: self.config.user_agent.clone(),
            timeout: self.config.intersphinx_timeout,
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_secs() as i64)
            .unwrap_or(0);
        let outcome = intersphinx::load_mappings(
            &LoadRequest {
                mapping: &self.config.intersphinx_mapping,
                srcdir: &self.source_dir,
                cache_dir: Some(self.cache.cache_dir().join(intersphinx::CACHE_DIR_NAME)),
                cache_limit: self.config.intersphinx_cache_limit,
                now,
                http: &http,
            },
            &UreqFetcher,
        )?;
        for message in outcome.infos {
            info!("{message}");
        }
        for message in outcome.warnings {
            // Sphinx logs these without a location and without a type, so
            // they render as a bare `WARNING: ...` — but they still count
            // toward the build's warning total and toward `-W`.
            self.add_warning(BuildWarning::new(
                PathBuf::new(),
                None,
                message,
                WarningType::Other,
            ));
        }
        self.intersphinx = Intersphinx {
            data: outcome.data,
            disabled_reftypes: self
                .config
                .intersphinx_disabled_reftypes
                .iter()
                .cloned()
                .collect(),
            resolve_self: self.config.intersphinx_resolve_self.clone(),
        };
        Ok(())
    }

    pub fn set_parallel_jobs(&mut self, jobs: usize) {
        self.parallel_jobs = jobs;
    }

    pub fn enable_incremental(&mut self) {
        self.incremental = true;
    }

    /// Discard the saved environment before building (sphinx-build `-E`).
    ///
    /// Sphinx's `-E` is `freshenv=True`: the pickled environment is not
    /// loaded at all, and the fresh one it builds instead reports every
    /// document as new. Emptying the cache directory is the same statement
    /// about the *other* half of the persisted state (documents and
    /// doctrees), and dropping the already-loaded environment here is what
    /// keeps the two halves saying the same thing.
    pub fn fresh_env(&mut self) -> Result<()> {
        self.cache.clear()?;
        self.env = BuildEnvironment::default();
        Ok(())
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

    pub async fn clean(&mut self) -> Result<()> {
        if self.output_dir.exists() {
            tokio::fs::remove_dir_all(&self.output_dir).await?;
        }
        // A clean build must not reuse documents cached before the clean
        // (the on-disk cache lived inside the output dir we just removed),
        // nor the environment that was loaded from it.
        self.cache.clear()?;
        self.env = BuildEnvironment::default();
        Ok(())
    }

    /// Run the build: read → merge → resolve → write, then validation.
    ///
    /// The four phases mirror Sphinx's own split (`builders/__init__.py`):
    ///
    /// - **read** ([`Self::read_phase`], parallel): parse every *outdated*
    ///   source file into a `Document` + doctree, persisting the doctree per
    ///   document, and recover the rest from the cache.
    /// - **merge** ([`Self::merge_phase`], sequential, docname-ordered):
    ///   fold each re-read document's output into the [`BuildEnvironment`] —
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

        self.load_intersphinx_inventories()?;

        let mut env = std::mem::take(&mut self.env);
        let to_read = self.plan_read(&env, &source_files);
        let mut read_results = self.read_phase(&source_files, &to_read)?;

        self.merge_phase(&mut env, &mut read_results);
        self.resolve_phase(&mut env, &read_results);
        self.env = env;

        let files_skipped = read_results
            .iter()
            .filter(|result| result.read_time_us.is_none())
            .count();

        // Keep documents in discovery order (the merge phase iterates a
        // docname-sorted view of its own): the write and validation phases
        // below produce warnings in this order, which is user-visible.
        let processed_docs: Vec<Document> = read_results
            .into_iter()
            .map(|result| result.document)
            .collect();

        self.write_phase(&processed_docs);

        // Directive/role validation runs in every build unless disabled
        if self.config.validate_directives {
            self.validate_directives_and_roles(&processed_docs);
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
            files_skipped,
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

    /// Which documents this build has to read
    /// ([`BuildEnvironment::get_outdated_files`]), and the `updating
    /// environment:` line Sphinx prints about it.
    ///
    /// A non-incremental build reads everything: without the document cache
    /// there is nowhere to recover an unread document's rendered output
    /// from, so "not reading it" would mean not writing its page. That is
    /// also what `sphinx-build -a` maps to here — it turns the cache off,
    /// and its write set (every found document) is what this builder writes
    /// in any case (see [`Self::write_phase`]).
    fn plan_read(&self, env: &BuildEnvironment, files: &[PathBuf]) -> BTreeSet<String> {
        // `env.doc2path` for the documents this build discovered, and
        // `env.found_docs` as its key set.
        let sources: BTreeMap<String, PathBuf> = files
            .iter()
            .map(|path| (self.docname_of_path(path), path.clone()))
            .collect();
        let found: BTreeSet<String> = sources.keys().cloned().collect();

        if !self.incremental {
            debug!(
                "Not an incremental build: reading all {} files",
                found.len()
            );
            return found;
        }

        let outdated = env.get_outdated_files(
            &found,
            // A configuration change wipes the cache directory whole
            // (`.config-fingerprint`), so it reaches this point as an empty
            // environment as well; saying it out loud keeps the two
            // statements from drifting apart.
            self.cache.config_changed(),
            &env::FileTimes {
                source_modified_us: &|docname| {
                    sources.get(docname).and_then(|path| modified_us(path))
                },
                doctree_exists: &|docname| self.doctree_path(docname).is_file(),
                dependency_modified_us: &modified_us,
            },
        );

        // Sphinx's `updating environment: %s added, %s changed, %s removed`
        // (`builders/__init__.py:493-497`), minus the `[reason]` prefix: the
        // whole-configuration fingerprint this crate uses cannot tell "new
        // config" from "config changed".
        info!(
            "updating environment: {} added, {} changed, {} removed",
            outdated.added.len(),
            outdated.changed.len(),
            outdated.removed.len()
        );

        let mut to_read = outdated.to_read();

        // Deliberate divergence: the toctrees that pointed at a deleted
        // document are read again.
        //
        // Sphinx does not do this — it resolves toctree entries a second
        // time while *writing* each page (`adapters/toctree.py`), which is
        // where its "toctree contains reference to nonexisting document"
        // warning comes from on a rebuild. This crate resolves entries once,
        // in the parser, so leaving the container unread would make the
        // deletion silent until the next cold build. Re-reading it is how a
        // read-time resolver keeps an incremental build's diagnostics equal
        // to a cold one's; it costs one re-parse per container, and only
        // when a document actually disappears.
        for removed in &outdated.removed {
            for container in env.files_to_rebuild.get(removed).into_iter().flatten() {
                if found.contains(container) {
                    to_read.insert(container.clone());
                }
            }
        }

        to_read
    }

    /// Read phase: parse every outdated source file in parallel, and
    /// recover the rest from the cache.
    ///
    /// One file failing must not abort the build: failures become
    /// `BuildErrorReport`s (and a non-zero exit) while the rest continue.
    /// Results keep the discovery order of `files`.
    fn read_phase(&self, files: &[PathBuf], to_read: &BTreeSet<String>) -> Result<Vec<ReadResult>> {
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
                    let docname = self.docname_of_path(file_path);
                    let outdated = to_read.contains(&docname);
                    (
                        file_path.clone(),
                        self.read_one_file(file_path, docname, &found_docs, outdated),
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

    /// One document's read-phase result.
    ///
    /// `outdated` decides *how*: an outdated document is parsed (its cache
    /// entry, however valid, describes a document the environment has
    /// already been told to forget), an up-to-date one is recovered whole
    /// from the cache — rendered page and persisted doctree — and is not
    /// merged into the environment again. A recovery that fails is not
    /// fatal: the document is parsed instead, which is honest work rather
    /// than a hit, and the cache counts it as the miss it is.
    fn read_one_file(
        &self,
        file_path: &Path,
        docname: String,
        found_docs: &Arc<BTreeSet<String>>,
        outdated: bool,
    ) -> Result<ReadResult> {
        let relative_path = file_path.strip_prefix(&self.source_dir)?;
        debug!("Processing file: {}", relative_path.display());

        // The write phase still writes an unread document's page — skipping
        // the write is how cached pages went missing from the output tree.
        if !outdated && self.incremental {
            if let Ok(file_mtime) = utils::get_file_mtime(file_path) {
                let hit = self.cache.get_document_with(file_path, |cached| {
                    if cached.source_mtime < file_mtime || cached.html.is_empty() {
                        return None;
                    }
                    // A cached document whose doctree file is gone, or was
                    // written in a format this build no longer reads, is not
                    // usable: the resolve phase needs that doctree, and
                    // inventing an empty one would quietly drop the
                    // document's toc, titles and toctrees. Re-parse instead
                    // — this counts as a cache miss.
                    self.load_doctree(&docname)
                });
                if let Some((document, doctree)) = hit {
                    debug!("Using cached version of {}", relative_path.display());
                    return Ok(ReadResult {
                        docname,
                        document,
                        doctree,
                        read_time_us: None,
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
            read_time_us: Some(now_micros()),
        })
    }

    /// Merge phase: fold the read phase's per-document output into the
    /// environment, in docname order.
    ///
    /// Sequential and deterministic, mirroring Sphinx's `merge_info_from`
    /// (`environment/__init__.py:421`) and the collectors it dispatches to:
    /// `all_docs`, the title collector, and the toctree collector.
    ///
    /// Only documents this build actually **read** are merged. Sphinx's
    /// `Builder._read_serial` clears a document immediately before reading
    /// it and touches nothing else; a document that was not outdated keeps
    /// every contribution the build that read it made, which is the whole
    /// point of the environment being persistent.
    fn merge_phase(&self, env: &mut BuildEnvironment, results: &mut [ReadResult]) {
        env.root_doc = self
            .config
            .root_doc
            .clone()
            .unwrap_or_else(|| DEFAULT_ROOT_DOC.to_string());

        // Documents that vanished since the saved environment was written
        // (Sphinx's `removed` set) must not leave stale state behind. The
        // set is taken from what the read phase came back with rather than
        // from `get_outdated_files`: it is the same set plus any document
        // that failed to read, whose recorded state is equally worthless.
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

        let mut ordered: Vec<usize> = (0..results.len()).collect();
        ordered.sort_by(|a, b| results[*a].docname.cmp(&results[*b].docname));

        // Sphinx's `env.doc2path`: the source file a docname was read from.
        // Documents this build did not read fall back to the conventional
        // `<srcdir>/<docname>.rst`, the same shape `doc2path` synthesizes
        // from `source_suffix`.
        //
        // Owned rather than borrowed from `results`, which this loop holds
        // mutably: the index domain *removes* an `index` node whose entries
        // do not validate.
        let paths: HashMap<String, PathBuf> = results
            .iter()
            .map(|result| (result.docname.clone(), result.document.source_path.clone()))
            .collect();
        let doc2path = |docname: &str| -> PathBuf {
            paths
                .get(docname)
                .cloned()
                .unwrap_or_else(|| self.source_dir.join(format!("{docname}.rst")))
        };

        for index in ordered {
            let result = &mut results[index];
            // A document this build did not read contributes nothing: what
            // it contributed last time is still in the environment, and
            // still correct.
            let Some(read_time_us) = result.read_time_us else {
                continue;
            };
            let docname = result.docname.clone();
            let docname = docname.as_str();
            // A re-read replaces this document's state wholesale — without
            // the clear, the `extend`-shaped fields (toctree_includes)
            // would accumulate duplicates across rebuilds.
            env.clear_doc(docname);

            env.all_docs.insert(docname.to_string(), read_time_us);

            let title = env_toctree::document_title(&result.doctree);
            // Sphinx's longtitle differs from the title only for documents
            // carrying an explicit `title` attribute, which nothing produces
            // yet (`collectors/title.py:27`).
            env.longtitles.insert(docname.to_string(), title.clone());
            env.titles.insert(docname.to_string(), title);

            env.metadata.insert(
                docname.to_string(),
                env_metadata::document_metadata(&result.doctree),
            );

            // The files this document pulls in, which is what makes it
            // outdated when one of *them* changes.
            env_dependencies::process_doc(env, docname, &result.doctree, &self.source_dir);

            let (toc, num_entries) = env_toctree::build_toc(&result.doctree, docname);
            // Each toctree node copied into the toc is noted, in the order
            // it was copied (which is the order Sphinx notes them in).
            for toctree in env_toctree::toctree_copies(&toc) {
                env_toctree::note_toctree(env, docname, toctree);
            }
            env.tocs.insert(docname.to_string(), toc);
            env.toc_num_entries.insert(docname.to_string(), num_entries);

            // `TocTree.parse_content` calls `env.note_reread()` for every
            // entry that names a document the project does not have
            // (`directives/other.py`): such a document is re-read on every
            // build, so that the day the missing target appears its toctree
            // takes it up — and stops warning about it. `clear_doc` above
            // dropped the previous read's claim, so a document that no
            // longer has a dangling entry is no longer re-read either.
            if result.document.toctrees.iter().any(|toctree| {
                toctree
                    .warnings
                    .iter()
                    .any(|warning| warning.kind == ToctreeWarningKind::MissingDocument)
            }) {
                env.reread_always.insert(docname.to_string());
            }

            // The document's toctree diagnostics, produced when its entries
            // were resolved. Sphinx logs them during the read phase, which
            // walks documents in this same sorted order.
            self.report_parse_warnings(&result.document);

            // The domains' read-phase hooks, dispatched in the order
            // `_DomainsContainer._process_doc` walks them — `index` before
            // `std` — and after the parse diagnostics above, which Sphinx
            // logs while reading.
            let text = result.document.content.to_string();
            let mut index_warnings = Vec::new();
            env_genindex::process_doc(
                env,
                docname,
                &mut result.doctree,
                &result.document.source_path,
                &text,
                &mut index_warnings,
            );
            for warning in index_warnings {
                self.add_warning(warning);
            }

            let mut std_warnings = Vec::new();
            env_std::process_doc(
                env,
                &env_std::DocumentSource {
                    docname,
                    doctree: &result.doctree,
                    registry: &result.document.registry,
                    text: &text,
                    path: &result.document.source_path,
                },
                &doc2path,
                &mut std_warnings,
            );
            for warning in std_warnings {
                self.add_warning(warning);
            }
        }
    }

    /// Surface one document's parse-time diagnostics: `TocTree.parse_content`'s
    /// warnings and the `logger.warning` calls other directives make
    /// (`RegistryExport::log_warnings`). Both are carried on the parse
    /// records rather than raised as they happen, so that a cache hit — which
    /// skips the parse entirely — still reproduces them.
    ///
    /// Sphinx logs both as the parse reaches them, so they interleave by
    /// source position; the two record streams are each in document order,
    /// and a stable sort by line merges them back into that one order.
    fn report_parse_warnings(&self, document: &Document) {
        let mut ordered: Vec<(u32, BuildWarning)> = Vec::new();
        for toctree in &document.toctrees {
            for warning in &toctree.warnings {
                let warning_type = match warning.kind {
                    ToctreeWarningKind::MissingDocument => WarningType::MissingToctreeRef,
                    ToctreeWarningKind::EmptyGlob | ToctreeWarningKind::PatternError => {
                        WarningType::EmptyToctree
                    }
                    ToctreeWarningKind::DuplicateEntry => WarningType::Other,
                };
                ordered.push((
                    warning.line,
                    BuildWarning::new(
                        document.source_path.clone(),
                        Some(warning.line as usize),
                        warning.message.clone(),
                        warning_type,
                    )
                    .with_category(warning.category.clone()),
                ));
            }
        }
        for warning in &document.registry.log_warnings {
            // Sphinx logs these with no `type`/`subtype`, so they render
            // with no `[category]` suffix.
            ordered.push((
                warning.line,
                BuildWarning::new(
                    document.source_path.clone(),
                    Some(warning.line as usize),
                    warning.message.clone(),
                    WarningType::Other,
                ),
            ));
        }
        ordered.sort_by_key(|(line, _)| *line);
        let mut warnings = self.warnings.lock().unwrap();
        warnings.extend(ordered.into_iter().map(|(_, warning)| warning));
    }

    /// Resolve phase: whole-project state that only exists once every
    /// document has been read, then persist the environment.
    ///
    /// Runs the numbering passes (`TocTreeCollector.get_updated_docs`, which
    /// Sphinx dispatches through `env-get-updated` right after the read
    /// phase) and Sphinx's post-read consistency checks over the finished
    /// toctree graph (`env.check_consistency()`), then saves the environment
    /// — Sphinx's own end-of-read-phase step (`builders/__init__.py:420`).
    ///
    /// Every document is resolved, not only the ones this build read: the
    /// write phase emits every page (see [`Self::write_phase`]), and a page
    /// is written from a doctree resolved against the environment as it
    /// stands *now* — a document that was not re-read can still have gained
    /// a section number, or lost the target of one of its references.
    ///
    /// Failing to save the environment is **not** a build failure: the
    /// cache directory is optional infrastructure, the output this build
    /// produced is valid without it, and the only consequence is that the
    /// next build starts cold. It is reported and the build goes on.
    fn resolve_phase(&self, env: &mut BuildEnvironment, results: &[ReadResult]) {
        info!("Resolving build environment");

        let sources: HashMap<&str, &Path> = results
            .iter()
            .map(|result| {
                (
                    result.docname.as_str(),
                    result.document.source_path.as_path(),
                )
            })
            .collect();

        self.number_phase(env, results);
        for message in env_toctree::check_consistency(env) {
            // Sphinx logs these with `location=docname`, which renders as
            // the document's source path with no line number.
            let source = sources
                .get(message.docname.as_str())
                .map(|path| path.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(&message.docname));
            match message.level {
                ConsistencyLevel::Warning => self.warnings.lock().unwrap().push(
                    BuildWarning::new(source, None, message.message, WarningType::OrphanedDocument)
                        .with_category(message.category),
                ),
                // Sphinx uses `logger.info` for the multiple-parents note,
                // so it must stay out of the warning count (and out of -W).
                ConsistencyLevel::Info => info!("{}: {}", source.display(), message.message),
            }
        }

        self.xref_phase(env, results);
        self.genindex_phase(env, &sources);

        if let Err(e) = env.save(self.cache.cache_dir()) {
            log::warn!(
                "Could not save the build environment to {}: {e:#} — this build's \
                 output is complete, but the next one will start from scratch",
                self.cache.cache_dir().display()
            );
        }
    }

    /// Assemble the general index (`IndexEntries.create_index`).
    ///
    /// Sphinx runs this from the HTML builder's `write_genindex`, so a
    /// `dummy` build never reaches it — but the environment oracle calls it
    /// explicitly, right after the build and before snapshotting warnings
    /// (see `tools/gen_env_fixture.py`), which is why this runs last and
    /// unconditionally rather than from the writer.
    fn genindex_phase(&self, env: &BuildEnvironment, sources: &HashMap<&str, &Path>) {
        // The oracle's dummy builder answers `get_relative_uri('genindex',
        // docname)` with `''` for every document, making each target a bare
        // `#<target_id>` — the same honest answer [`Self::xref_phase`] gives
        // until the HTML writer supplies its own uri scheme.
        let rel_uri = |_docname: &str| Some(String::new());
        let mut messages = Vec::new();
        let groups = env_genindex::create_index(env, &rel_uri, &mut messages);
        for message in messages {
            // `location=docname`: the document's source path, no line.
            let source = sources
                .get(message.docname.as_str())
                .map(|path| path.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(&message.docname));
            self.add_warning(message.into_warning(&source));
        }
        *self.genindex.lock().unwrap() = groups;
    }

    /// Cross-reference resolution (`ReferencesResolver`, run per document as
    /// Sphinx writes it — after numbering, which `:numref:` reads).
    ///
    /// Each document is resolved over a *copy* of its doctree, exactly like
    /// Sphinx's `get_and_resolve_doctree`, and the result is kept as
    /// pseudo-XML for [`Self::snapshot_env`]. Documents are visited in
    /// docname order, which is the order Sphinx's write loop uses and
    /// therefore the order its warnings come out in.
    fn xref_phase(&self, env: &BuildEnvironment, results: &[ReadResult]) {
        let in_memory: HashMap<&str, &Doctree> = results
            .iter()
            .map(|result| (result.docname.as_str(), &result.doctree))
            .collect();
        let load_doctree = |docname: &str| -> Option<Cow<'_, Doctree>> {
            match in_memory.get(docname) {
                Some(doctree) => Some(Cow::Borrowed(*doctree)),
                None => self.load_doctree(docname).map(Cow::Owned),
            }
        };
        // The oracle builds with sphinx's dummy builder, whose
        // `get_target_uri` is `''`; nothing consumes a resolved doctree's
        // URIs yet (the write phase renders from `Document.html`), so this
        // is the one honest answer until the HTML writer supplies its own.
        let relative_uri = |_from: &str, _to: &str| String::new();
        let resolver = env_resolve::Resolver {
            env,
            numfig: self.config.numfig,
            numfig_format: &self.config.numfig_format,
            doctree: &load_doctree,
            relative_uri: &relative_uri,
            intersphinx: &self.intersphinx,
        };
        let nitpick = env_resolve::NitpickConfig {
            nitpicky: self.config.nitpicky,
            ignore: &self.config.nitpick_ignore,
            ignore_regex: &self.config.nitpick_ignore_regex,
        };

        let mut ordered: Vec<&ReadResult> = results.iter().collect();
        ordered.sort_by(|a, b| a.docname.cmp(&b.docname));

        // A second `build()` on the same builder must not keep the previous
        // one's documents (one of them may since have been deleted).
        self.resolved.lock().unwrap().clear();
        let mut unresolvable_domain_refs = 0usize;
        for result in ordered {
            let mut doctree = result.doctree.clone();
            let resolution = env_resolve::resolve_document(
                &resolver,
                &nitpick,
                &result.docname,
                &mut doctree,
                &result.document.content.to_string(),
                &result.document.source_path,
            );
            unresolvable_domain_refs += resolution.unresolvable_domain_refs;
            for warning in resolution.warnings {
                self.add_warning(warning);
            }
            self.resolved
                .lock()
                .unwrap()
                .insert(result.docname.clone(), doctree.root.pformat());
        }

        if unresolvable_domain_refs > 0 {
            info!(
                "{unresolvable_domain_refs} python-domain reference(s) not validated \
                 (no object inventory until M5)"
            );
        }
    }

    /// Section and figure numbering (`TocTreeCollector.get_updated_docs`,
    /// `collectors/toctree.py:194`), run in that order: figure numbers are
    /// scoped by the section numbers the first pass assigns.
    ///
    /// The doctree loader hands the walks whatever this build already has in
    /// memory — every read result carries its doctree, including the ones a
    /// warm cache hit loaded from disk — and falls back to the persisted
    /// doctree for anything else.
    ///
    /// The returned docnames (Sphinx's `rewrite_needed`) are the documents
    /// whose numbering moved, which Sphinx adds to its write set. They are
    /// logged rather than consumed here because this builder's write set is
    /// already every found document (see [`Self::write_phase`]) — a
    /// superset — so there is nothing left for them to widen.
    fn number_phase(&self, env: &mut BuildEnvironment, results: &[ReadResult]) {
        let in_memory: HashMap<&str, &Doctree> = results
            .iter()
            .map(|result| (result.docname.as_str(), &result.doctree))
            .collect();
        let load_doctree = |docname: &str| -> Option<std::borrow::Cow<'_, Doctree>> {
            match in_memory.get(docname) {
                Some(doctree) => Some(std::borrow::Cow::Borrowed(*doctree)),
                None => self.load_doctree(docname).map(std::borrow::Cow::Owned),
            }
        };

        let sections = env_numbers::assign_section_numbers(env, &load_doctree);
        for warning in sections.warnings {
            self.report_numbering_warning(&warning, results);
        }
        let figures = env_numbers::assign_figure_numbers(
            env,
            self.config.numfig,
            self.config.numfig_secnum_depth,
            &load_doctree,
        );
        debug!(
            "Numbering: {} document(s) with changed section numbers, {} with changed figure numbers",
            sections.changed.len(),
            figures.len()
        );
    }

    /// Surface one numbering diagnostic at the location Sphinx logs it —
    /// the source line of the `toctree` node it names, which the parse
    /// record for that document's Nth toctree carries.
    fn report_numbering_warning(
        &self,
        warning: &env_numbers::NumberingWarning,
        results: &[ReadResult],
    ) {
        let document = results
            .iter()
            .find(|result| result.docname == warning.docname)
            .map(|result| &result.document);
        let source = document
            .map(|document| document.source_path.clone())
            .unwrap_or_else(|| PathBuf::from(&warning.docname));
        let line = document
            .and_then(|document| document.toctrees.get(warning.toctree_index))
            .map(|toctree| toctree.line as usize);
        self.warnings.lock().unwrap().push(
            BuildWarning::new(
                source,
                line,
                warning.message.clone(),
                WarningType::MissingToctreeRef,
            )
            .with_category(warning.category.clone()),
        );
    }

    /// Write phase: emit every document's rendered output, in parallel and
    /// after resolution, so that a page can be written with whole-project
    /// knowledge (numbering, relations) once those exist.
    ///
    /// **Every found document is written, not just the ones this build
    /// read.** Sphinx writes the read set plus the toctree containers of
    /// what changed plus the documents whose numbering moved
    /// (`builders/__init__.py:717-736`), because its HTML builder also
    /// compares each output file against its sources and can tell that the
    /// rest are already on disk and current. This builder cannot do that
    /// yet, so it writes the superset — which is also what makes a cache
    /// hit still produce a page, rather than leaving a hole in the output
    /// tree where an unchanged document should be.
    ///
    /// One page failing to write must not abort the build (the same rule the
    /// read phase follows): it becomes a `BuildErrorReport`, and a non-zero
    /// exit, while the remaining pages are still written.
    fn write_phase(&self, documents: &[Document]) {
        documents.par_iter().for_each(|document| {
            if let Err(e) = self.write_one(document) {
                self.errors.lock().unwrap().push(BuildErrorReport::new(
                    document.source_path.clone(),
                    None,
                    format!("{e:#}"),
                    ErrorType::Other,
                ));
            }
        });
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

    /// Persist one document's doctree, behind the
    /// [`DOCTREE_MAGIC`]/[`DOCTREE_FORMAT_VERSION`] header that lets a later
    /// build tell whether the bytes are still readable *by meaning*, not
    /// just by bincode.
    fn store_doctree(&self, docname: &str, doctree: &Doctree) -> Result<()> {
        let path = self.doctree_path(docname);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let blob = crate::doctree::to_bincode(doctree);
        let mut bytes = Vec::with_capacity(DOCTREE_HEADER_LEN + blob.len());
        bytes.extend_from_slice(DOCTREE_MAGIC);
        bytes.extend_from_slice(&DOCTREE_FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&blob);
        std::fs::write(path, bytes)?;
        Ok(())
    }

    /// The persisted doctree for `docname`, or `None` if it is missing,
    /// carries another format version (including none at all — a file
    /// written before the header existed), or cannot be decoded (a
    /// truncated write). Every `None` means the same thing to callers: this
    /// document has to be read again.
    fn load_doctree(&self, docname: &str) -> Option<Doctree> {
        let path = self.doctree_path(docname);
        let bytes = std::fs::read(&path).ok()?;
        let Some(blob) = current_format_doctree(&bytes) else {
            debug!(
                "Ignoring doctree {} written in another format (re-reading {docname})",
                path.display()
            );
            return None;
        };
        match crate::doctree::from_bincode(blob) {
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

    /// Root-relative docname (no extension, forward slashes) for a source
    /// path.
    fn docname_of_path(&self, path: &Path) -> String {
        let relative = path.strip_prefix(&self.source_dir).unwrap_or(path);
        relative
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/")
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
    /// shape the `env_differential` oracle compares against — plus the
    /// `resolved_pformat` of every document this build resolved, which is
    /// build output rather than environment state and so has no home inside
    /// [`BuildEnvironment`].
    pub fn snapshot_env(&self) -> serde_json::Value {
        let mut snapshot = self.env.snapshot();
        let resolved: serde_json::Map<String, serde_json::Value> = self
            .resolved
            .lock()
            .unwrap()
            .iter()
            .map(|(docname, pformat)| (docname.clone(), serde_json::Value::String(pformat.clone())))
            .collect();
        if let Some(object) = snapshot.as_object_mut() {
            object.insert(
                "resolved_pformat".to_string(),
                serde_json::Value::Object(resolved),
            );
            object.insert(
                "genindex".to_string(),
                env_genindex::snapshot(&self.genindex.lock().unwrap()),
            );
        }
        snapshot
    }
}

/// The bincode blob inside a persisted doctree file, or `None` if the file
/// does not start with this build's [`DOCTREE_MAGIC`] +
/// [`DOCTREE_FORMAT_VERSION`] header.
fn current_format_doctree(bytes: &[u8]) -> Option<&[u8]> {
    let (header, blob) = bytes.split_at_checked(DOCTREE_HEADER_LEN)?;
    if &header[..DOCTREE_MAGIC.len()] != DOCTREE_MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(header[DOCTREE_MAGIC.len()..].try_into().ok()?);
    (version == DOCTREE_FORMAT_VERSION).then_some(blob)
}

/// A file's modification time in microseconds since the Unix epoch — the
/// unit `env.all_docs` read times are in, so the two are directly
/// comparable (Sphinx's `_StrPath._last_modified_time`).
///
/// `None` when the file cannot be stat-ed, which is Sphinx's `OSError`
/// path: the caller treats it as "this document is outdated".
fn modified_us(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(
        modified
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_micros() as u64)
            .unwrap_or(0),
    )
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
    fn persisted_doctrees_carry_the_format_version_header() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (_stats, builder) = build_incrementally(&source_dir, &output_dir);

        let bytes = std::fs::read(builder.doctree_path("index")).unwrap();
        assert_eq!(
            &bytes[..DOCTREE_MAGIC.len()],
            DOCTREE_MAGIC,
            "a persisted doctree must be self-identifying"
        );
        let version = u32::from_le_bytes(
            bytes[DOCTREE_MAGIC.len()..DOCTREE_MAGIC.len() + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(version, DOCTREE_FORMAT_VERSION);
        assert_eq!(
            builder.load_doctree("index").unwrap().root.kind,
            crate::doctree::kinds::DOCUMENT,
            "the header must not disturb the round trip"
        );
    }

    #[test]
    fn a_doctree_written_in_the_unversioned_format_is_treated_as_a_miss() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (_cold, builder) = build_incrementally(&source_dir, &output_dir);

        // Exactly what the pre-versioning builder wrote: a bare bincode
        // blob. It still *decodes* — which is the trap: an old blob whose
        // attribute shapes have since changed decodes into a plausible
        // doctree and then mis-harvests. The version word is what makes it
        // a miss instead.
        let doctree = builder.load_doctree("index").expect("doctree decodes");
        std::fs::write(
            builder.doctree_path("index"),
            crate::doctree::to_bincode(&doctree),
        )
        .unwrap();
        assert!(
            builder.load_doctree("index").is_none(),
            "an unversioned blob must not be trusted"
        );

        let (stats, rebuilt) = build_incrementally(&source_dir, &output_dir);
        assert_eq!(
            stats.cache_hits, 1,
            "the document whose doctree is stale must be re-read"
        );
        assert!(rebuilt.load_doctree("index").is_some());
    }

    #[test]
    fn a_doctree_from_a_future_format_version_is_treated_as_a_miss() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (_cold, builder) = build_incrementally(&source_dir, &output_dir);
        let doctree = builder.load_doctree("index").expect("doctree decodes");

        let mut bytes = Vec::from(DOCTREE_MAGIC);
        bytes.extend_from_slice(&(DOCTREE_FORMAT_VERSION + 1).to_le_bytes());
        bytes.extend_from_slice(&crate::doctree::to_bincode(&doctree));
        std::fs::write(builder.doctree_path("index"), bytes).unwrap();

        assert!(builder.load_doctree("index").is_none());
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

    /// The cache directory is optional infrastructure: a build whose
    /// environment cannot be written still produced valid output, and
    /// saying "build failed" over it would be a lie. (A directory where
    /// `env.bin` belongs is the portable way to make exactly that one write
    /// fail while every other cache write succeeds.)
    #[test]
    fn a_build_whose_environment_cannot_be_saved_still_writes_its_output() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (_cold, builder) = build_incrementally(&source_dir, &output_dir);
        let env_file = builder.cache.cache_dir().join("env.bin");
        std::fs::remove_file(&env_file).unwrap();
        std::fs::create_dir(&env_file).unwrap();

        let (stats, rebuilt) = build_incrementally(&source_dir, &output_dir);

        assert_eq!(stats.errors, 0, "an unsaveable environment is not an error");
        assert!(
            output_dir.join("index.html").is_file() && output_dir.join("a.html").is_file(),
            "the pages this build produced are still written"
        );
        assert_eq!(
            rebuilt.env().all_docs.len(),
            2,
            "the in-memory environment is complete; only its persistence failed"
        );
    }

    /// `-E`: the persisted environment is not to be trusted, and neither is
    /// the half of it already sitting in memory.
    #[test]
    fn fresh_env_discards_the_loaded_environment_and_re_reads_everything() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        build_incrementally(&source_dir, &output_dir);

        let mut builder = SphinxBuilder::new(
            BuildConfig::default(),
            source_dir.clone(),
            output_dir.clone(),
        )
        .unwrap();
        builder.enable_incremental();
        assert_eq!(
            builder.env().all_docs.len(),
            2,
            "the builder loads the saved environment"
        );

        builder.fresh_env().unwrap();
        assert!(
            builder.env().all_docs.is_empty(),
            "-E starts from an empty environment, not the loaded one"
        );

        let stats = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(builder.build())
            .unwrap();
        assert_eq!(stats.cache_hits, 0, "every document is new again");
        assert_eq!(stats.files_skipped, 0);
        assert_eq!(builder.env().all_docs.len(), 2);
    }

    /// A build with the document cache off has nowhere to recover an unread
    /// document's rendered page from, so it reads everything — which is
    /// what `sphinx-build -a` maps to here.
    #[test]
    fn a_non_incremental_build_reads_every_document() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        build_incrementally(&source_dir, &output_dir);

        let mut builder = SphinxBuilder::new(
            BuildConfig::default(),
            source_dir.clone(),
            output_dir.clone(),
        )
        .unwrap();
        let stats = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(builder.build())
            .unwrap();

        assert_eq!(stats.cache_hits, 0);
        assert_eq!(stats.files_skipped, 0, "nothing was skipped: all was read");
        assert!(output_dir.join("index.html").is_file() && output_dir.join("a.html").is_file());
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
