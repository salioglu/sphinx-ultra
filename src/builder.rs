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
use crate::error::{BuildErrorReport, BuildWarning};
use crate::parser::Parser;
use crate::utils;

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
}

impl SphinxBuilder {
    pub fn new(config: BuildConfig, source_dir: PathBuf, output_dir: PathBuf) -> Result<Self> {
        let cache_dir = output_dir.join(".sphinx-ultra-cache");
        let cache = BuildCache::new(cache_dir)?;

        let parser = Parser::new(&config)?;

        let parallel_jobs = config.parallel_jobs.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });

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
        })
    }

    pub fn set_parallel_jobs(&mut self, jobs: usize) {
        self.parallel_jobs = jobs;
    }

    pub fn enable_incremental(&mut self) {
        self.incremental = true;
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
        // For now, use a simple synchronous approach to avoid async recursion issues
        let mut files = Vec::new();
        self.discover_files_sync(&self.source_dir, &mut files)?;
        Ok(files)
    }

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

        let documents: Result<Vec<_>, _> = pool.install(|| {
            files
                .par_iter()
                .map(|file_path| self.process_single_file(file_path))
                .collect()
        });

        documents
    }

    fn process_single_file(&self, file_path: &Path) -> Result<Document> {
        let relative_path = file_path.strip_prefix(&self.source_dir)?;
        debug!("Processing file: {}", relative_path.display());

        // Check cache if incremental build is enabled
        if self.incremental {
            if let Ok(cached_doc) = self.cache.get_document(file_path) {
                let file_mtime = utils::get_file_mtime(file_path)?;
                if cached_doc.source_mtime >= file_mtime {
                    debug!("Using cached version of {}", relative_path.display());
                    return Ok(cached_doc);
                }
            }
        }

        // Read and parse the file
        let content = std::fs::read_to_string(file_path)?;
        let document = self.parser.parse(file_path, &content)?;

        // Simple document rendering (placeholder)
        let rendered_html = format!(
            "<html><body>{}</body></html>",
            html_escape::encode_text(&document.content.to_string())
        );

        // Write output file
        let output_path = self.get_output_path(file_path)?;
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&output_path, &rendered_html)?;

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

    async fn validate_documents(
        &self,
        processed_docs: &[Document],
        _source_files: &[PathBuf],
    ) -> Result<()> {
        info!("Validating documents and checking for warnings...");

        let mut toctree_references = HashSet::new();
        let mut referenced_files = HashSet::new();
        let mut all_documents = HashSet::new();

        // Collect all documents and their toctree references
        for doc in processed_docs {
            // Get relative path for comparison
            let doc_path_relative = doc
                .source_path
                .strip_prefix(&self.source_dir)
                .unwrap_or(&doc.source_path);
            let doc_path_no_ext = doc_path_relative.with_extension("");
            all_documents.insert(doc_path_no_ext.to_string_lossy().to_string());

            // Check for toctree directives and collect their references
            if let Some(toctree_refs) = self.extract_toctree_references(doc) {
                for toc_ref in toctree_refs {
                    toctree_references.insert((doc.source_path.clone(), toc_ref.clone()));
                    referenced_files.insert(toc_ref);
                }
            }
        }

        // Check for missing toctree references
        for (source_file, reference) in &toctree_references {
            let ref_path = format!("{}/index", reference);
            let alt_ref_path = reference.clone();

            if !all_documents.contains(&ref_path) && !all_documents.contains(&alt_ref_path) {
                let warning = BuildWarning::missing_toctree_ref(
                    source_file.clone(),
                    Some(10), // TODO: Extract actual line number
                    reference,
                );
                self.warnings.lock().unwrap().push(warning);
            }
        }

        // Check for orphaned documents
        for doc in processed_docs {
            let doc_path_relative = doc
                .source_path
                .strip_prefix(&self.source_dir)
                .unwrap_or(&doc.source_path);
            let doc_path_no_ext = doc_path_relative.with_extension("");
            let doc_path_str = doc_path_no_ext.to_string_lossy().to_string();

            // Skip the main index file
            if doc_path_str == "index" {
                continue;
            }

            // Check if this document is referenced in any toctree
            let is_referenced = referenced_files.iter().any(|ref_path| {
                ref_path == &doc_path_str
                    || ref_path == &format!("{}/index", doc_path_str)
                    || doc_path_str.starts_with(&format!("{}/", ref_path))
            });

            if !is_referenced {
                let warning = BuildWarning::orphaned_document(doc.source_path.clone());
                self.warnings.lock().unwrap().push(warning);
            }
        }

        let warning_count = self.warnings.lock().unwrap().len();
        info!("Validation completed. Found {} warnings", warning_count);

        Ok(())
    }

    fn extract_toctree_references(&self, doc: &Document) -> Option<Vec<String>> {
        use crate::document::DocumentContent;

        let mut references = Vec::new();

        if let DocumentContent::RestructuredText(rst_content) = &doc.content {
            for node in &rst_content.ast {
                if let crate::document::RstNode::Directive { name, content, .. } = node {
                    if name == "toctree" {
                        // Extract references from toctree content
                        for line in content.lines() {
                            let trimmed = line.trim();
                            if !trimmed.is_empty()
                                && !trimmed.starts_with(':')
                                && !trimmed.starts_with("..")
                            {
                                references.push(trimmed.to_string());
                            }
                        }
                    }
                }
            }
        }

        if references.is_empty() {
            None
        } else {
            Some(references)
        }
    }

    async fn generate_search_index(&self, _documents: &[Document]) -> Result<()> {
        info!("Generating search index");
        // TODO: Implement search index generation
        Ok(())
    }
}
