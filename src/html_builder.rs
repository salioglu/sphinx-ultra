use anyhow::{Context, Result};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value as JsonValue};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;

use crate::config::BuildConfig;
use crate::document::Document;
use crate::inventory::InventoryFile;
use crate::template::TemplateEngine;
use crate::utils;

/// The filename for the inventory of objects (matches Sphinx)
pub const INVENTORY_FILENAME: &str = "objects.inv";

/// HTML Builder that mirrors Sphinx's StandaloneHTMLBuilder
#[derive(Debug)]
pub struct HTMLBuilder {
    pub name: String,
    pub format: String,
    pub epilog: String,
    pub out_suffix: String,
    pub link_suffix: String,
    pub searchindex_filename: String,
    pub allow_parallel: bool,
    pub copysource: bool,
    pub use_index: bool,
    pub embedded: bool,
    pub search: bool,
    pub download_support: bool,
    pub supported_image_types: Vec<String>,
    pub supported_remote_images: bool,
    pub supported_data_uri_images: bool,

    // Directories
    pub outdir: PathBuf,
    pub srcdir: PathBuf,
    pub confdir: PathBuf,
    pub static_dir: PathBuf,
    pub sources_dir: PathBuf,
    pub downloads_dir: PathBuf,
    pub images_dir: PathBuf,

    // Internal state
    pub config: BuildConfig,
    pub current_docname: String,
    pub secnumbers: HashMap<String, Vec<u32>>,
    pub imgpath: String,
    pub dlpath: String,

    // Asset management
    pub css_files: Vec<CSSFile>,
    pub js_files: Vec<JSFile>,

    // Template engine
    pub template_engine: TemplateEngine,

    /// Global template context
    pub global_context: Map<String, JsonValue>,

    // Relations between documents
    pub relations: HashMap<String, DocumentRelation>,

    // Domain indices
    pub domain_indices: Vec<DomainIndex>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CSSFile {
    pub filename: String,
    pub priority: i32,
    pub media: Option<String>,
    pub id: Option<String>,
    pub rel: String,
    pub type_: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSFile {
    pub filename: String,
    pub priority: i32,
    pub loading_method: String,
    pub async_: bool,
    pub defer: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentRelation {
    pub parent: Option<String>,
    pub prev: Option<String>,
    pub next: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DomainIndex {
    pub name: String,
    pub localname: String,
    pub shortname: Option<String>,
    pub content: Vec<IndexEntry>,
    pub collapse: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    pub subentries: Vec<IndexEntry>,
    pub uri: String,
    pub display_name: String,
}

impl HTMLBuilder {
    pub fn new(config: BuildConfig, srcdir: PathBuf, outdir: PathBuf) -> Result<Self> {
        let confdir = srcdir.clone();
        let static_dir = outdir.join("_static");
        let sources_dir = outdir.join("_sources");
        let downloads_dir = outdir.join("_downloads");
        let images_dir = outdir.join("_images");

        let template_engine = TemplateEngine::new(&config)?;

        Ok(Self {
            name: "html".to_string(),
            format: "html".to_string(),
            epilog: "The HTML pages are in %(outdir)s.".to_string(),
            out_suffix: ".html".to_string(),
            link_suffix: ".html".to_string(),
            searchindex_filename: "searchindex.js".to_string(),
            allow_parallel: true,
            copysource: true,
            use_index: false,
            embedded: false,
            search: true,
            download_support: true,
            supported_image_types: vec![
                "image/svg+xml".to_string(),
                "image/png".to_string(),
                "image/gif".to_string(),
                "image/jpeg".to_string(),
            ],
            supported_remote_images: true,
            supported_data_uri_images: true,

            outdir,
            srcdir,
            confdir,
            static_dir,
            sources_dir,
            downloads_dir,
            images_dir,

            config,
            current_docname: String::new(),
            secnumbers: HashMap::new(),
            imgpath: String::new(),
            dlpath: String::new(),

            css_files: Vec::new(),
            js_files: Vec::new(),

            template_engine,

            global_context: Map::new(),
            relations: HashMap::new(),
            domain_indices: Vec::new(),
        })
    }

    /// Initialize the builder (mirrors Sphinx's init method)
    pub async fn init(&mut self) -> Result<()> {
        info!("Initializing HTML builder");

        // Create necessary directories
        fs::create_dir_all(&self.outdir).await?;
        fs::create_dir_all(&self.static_dir).await?;
        fs::create_dir_all(&self.sources_dir).await?;
        fs::create_dir_all(&self.downloads_dir).await?;
        fs::create_dir_all(&self.images_dir).await?;

        // Initialize CSS and JS files
        self.init_css_files()?;
        self.init_js_files()?;

        // Set up global template context
        self.init_global_context()?;

        // Configure use_index based on config
        self.use_index = self.config.html_use_index.unwrap_or(true);

        Ok(())
    }

    /// Initialize CSS files (mirrors Sphinx's init_css_files)
    fn init_css_files(&mut self) -> Result<()> {
        self.css_files.clear();

        // Add pygments CSS
        self.add_css_file("pygments.css", 200, None, None)?;

        // Add theme stylesheets
        let styles = self.config.html_style.clone();
        for style in &styles {
            self.add_css_file(style, 200, None, None)?;
        }

        // Add user CSS files
        let css_files = self.config.html_css_files.clone();
        for css_file in &css_files {
            self.add_css_file(css_file, 800, None, None)?;
        }

        Ok(())
    }

    /// Initialize JS files (mirrors Sphinx's init_js_files)
    fn init_js_files(&mut self) -> Result<()> {
        self.js_files.clear();

        // Add core JS files
        self.add_js_file("documentation_options.js", 200, false, false)?;
        self.add_js_file("doctools.js", 200, false, false)?;
        self.add_js_file("sphinx_highlight.js", 200, false, false)?;

        // Add user JS files
        let js_files = self.config.html_js_files.clone();
        for js_file in &js_files {
            self.add_js_file(js_file, 800, false, false)?;
        }

        // Add translations if available
        if self.has_translations() {
            self.add_js_file("translations.js", 500, false, false)?;
        }

        Ok(())
    }

    /// Add a CSS file
    fn add_css_file(
        &mut self,
        filename: &str,
        priority: i32,
        media: Option<&str>,
        id: Option<&str>,
    ) -> Result<()> {
        let filename = if !filename.contains("://") {
            format!("_static/{}", filename)
        } else {
            filename.to_string()
        };

        let css_file = CSSFile {
            filename,
            priority,
            media: media.map(|s| s.to_string()),
            id: id.map(|s| s.to_string()),
            rel: "stylesheet".to_string(),
            type_: "text/css".to_string(),
        };

        if !self.css_files.contains(&css_file) {
            self.css_files.push(css_file);
        }

        Ok(())
    }

    /// Add a JS file
    fn add_js_file(
        &mut self,
        filename: &str,
        priority: i32,
        async_: bool,
        defer: bool,
    ) -> Result<()> {
        let filename = if !filename.is_empty() && !filename.contains("://") {
            format!("_static/{}", filename)
        } else {
            filename.to_string()
        };

        let js_file = JSFile {
            filename,
            priority,
            loading_method: "normal".to_string(),
            async_,
            defer,
        };

        if !self.js_files.contains(&js_file) {
            self.js_files.push(js_file);
        }

        Ok(())
    }

    /// Check if translations are available
    fn has_translations(&self) -> bool {
        // Check for translation files
        let locale_dir = self.confdir.join("locale");
        let lang = self.config.language.as_deref().unwrap_or("en");
        let js_file = locale_dir.join(lang).join("LC_MESSAGES").join("sphinx.js");
        js_file.exists()
    }

    /// Initialize global template context (mirrors Sphinx's prepare_writing)
    fn init_global_context(&mut self) -> Result<()> {
        use serde_json::json;

        let _now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let last_updated = if let Some(fmt) = &self.config.html_last_updated_fmt {
            Some(utils::format_date(fmt, &self.config.language))
        } else {
            None
        };

        self.global_context = json!({
            "embedded": self.embedded,
            "project": self.config.project,
            "release": self.config.release.as_deref().unwrap_or(""),
            "version": self.config.version.as_deref().unwrap_or(""),
            "last_updated": last_updated,
            "copyright": self.config.copyright.as_deref().unwrap_or(""),
            "master_doc": self.config.root_doc.as_deref().unwrap_or("index"),
            "root_doc": self.config.root_doc.as_deref().unwrap_or("index"),
            "use_opensearch": self.config.html_use_opensearch.unwrap_or(false),
            "docstitle": self.config.html_title.as_deref().unwrap_or(&self.config.project),
            "shorttitle": self.config.html_short_title.as_deref().unwrap_or(&self.config.project),
            "show_copyright": self.config.html_show_copyright.unwrap_or(true),
            "show_sphinx": self.config.html_show_sphinx.unwrap_or(true),
            "has_source": self.config.html_copy_source.unwrap_or(true),
            "show_source": self.config.html_show_sourcelink.unwrap_or(true),
            "sourcelink_suffix": self.config.html_sourcelink_suffix.as_deref().unwrap_or(".txt"),
            "file_suffix": &self.out_suffix,
            "link_suffix": &self.link_suffix,
            "script_files": &self.js_files,
            "language": self.config.language.as_deref().unwrap_or("en"),
            "css_files": &self.css_files,
            "sphinx_version": env!("CARGO_PKG_VERSION"),
            "styles": self.config.html_style.clone(),
            "builder": &self.name,
            "parents": Vec::<String>::new(),
            "logo_url": self.config.html_logo.as_deref().unwrap_or(""),
            "favicon_url": self.config.html_favicon.as_deref().unwrap_or(""),
            "html5_doctype": true,
        })
        .as_object()
        .unwrap()
        .clone();

        Ok(())
    }

    /// Write a single document (mirrors Sphinx's write_doc)
    pub async fn write_doc(&mut self, docname: &str, doctree: &Document) -> Result<()> {
        info!("Writing document: {}", docname);

        self.current_docname = docname.to_string();
        self.imgpath = self.get_relative_uri(docname, "_images");
        self.dlpath = self.get_relative_uri(docname, "_downloads");

        // Render the document to HTML
        let body = format!(
            "<div class=\"document\">\n{}\n</div>",
            html_escape::encode_text(&doctree.content.to_string())
        );
        let metatags = format!(
            "<meta name=\"source\" content=\"{}\" />",
            html_escape::encode_double_quoted_attribute(&doctree.source_path.to_string_lossy())
        );

        // Get document context
        let ctx = self.get_doc_context(docname, &body, &metatags).await?;

        // Handle the page
        self.handle_page(docname, ctx, "page.html").await?;

        Ok(())
    }

    /// Get document context for template (mirrors Sphinx's get_doc_context)
    async fn get_doc_context(
        &self,
        docname: &str,
        body: &str,
        metatags: &str,
    ) -> Result<serde_json::Map<String, serde_json::Value>> {
        use serde_json::json;

        let mut ctx = self.global_context.clone();

        // Find relations
        let relation = self.relations.get(docname);
        let (prev, next) = if let Some(rel) = relation {
            (rel.prev.clone(), rel.next.clone())
        } else {
            (None, None)
        };

        // Build parents chain
        let mut parents = Vec::new();
        let mut current = relation.and_then(|r| r.parent.clone());
        while let Some(parent_name) = current {
            if let Some(parent_rel) = self.relations.get(&parent_name) {
                parents.push(json!({
                    "link": self.get_relative_uri(docname, &parent_name),
                    "title": parent_name, // TODO: Get actual title
                }));
                current = parent_rel.parent.clone();
            } else {
                break;
            }
        }
        parents.reverse();

        // Title and metadata
        let title = docname; // TODO: Extract actual title from document
        let source_suffix = ".rst"; // TODO: Detect actual suffix
        let sourcename = if self.config.html_copy_source.unwrap_or(true) {
            format!(
                "{}{}",
                docname,
                self.config
                    .html_sourcelink_suffix
                    .as_deref()
                    .unwrap_or(".txt")
            )
        } else {
            String::new()
        };

        // Local TOC
        let toc = self.generate_local_toc(docname).await?;

        ctx.insert("parents".to_string(), json!(parents));
        if let Some(p) = prev {
            ctx.insert(
                "prev".to_string(),
                json!({
                    "link": self.get_relative_uri(docname, &p),
                    "title": p, // TODO: Get actual title
                }),
            );
        }
        if let Some(n) = next {
            ctx.insert(
                "next".to_string(),
                json!({
                    "link": self.get_relative_uri(docname, &n),
                    "title": n, // TODO: Get actual title
                }),
            );
        }
        ctx.insert("title".to_string(), json!(title));
        ctx.insert("body".to_string(), json!(body));
        ctx.insert("metatags".to_string(), json!(metatags));
        ctx.insert("sourcename".to_string(), json!(sourcename));
        ctx.insert("toc".to_string(), json!(toc));
        ctx.insert("display_toc".to_string(), json!(true));
        ctx.insert("page_source_suffix".to_string(), json!(source_suffix));

        Ok(ctx)
    }

    /// Generate local table of contents
    async fn generate_local_toc(&self, _docname: &str) -> Result<String> {
        // TODO: Implement actual TOC generation
        Ok("<div class=\"toc\"></div>".to_string())
    }

    /// Handle a page (render and write) - mirrors Sphinx's handle_page
    async fn handle_page(
        &self,
        pagename: &str,
        context: serde_json::Map<String, serde_json::Value>,
        template_name: &str,
    ) -> Result<()> {
        debug!(
            "Handling page: {} with template: {}",
            pagename, template_name
        );

        // Render the template
        let output = self.template_engine.render(template_name, &context)?;

        // Write to file
        let output_path = self.get_output_path(pagename);
        utils::ensure_dir(output_path.parent().unwrap()).await?;

        fs::write(&output_path, output)
            .await
            .with_context(|| format!("Failed to write page: {}", output_path.display()))?;

        // Copy source file if needed
        if self.copysource
            && context
                .get("sourcename")
                .and_then(|s| s.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false)
        {
            let sourcename = context["sourcename"].as_str().unwrap();
            let source_path = self.sources_dir.join(sourcename);
            utils::ensure_dir(source_path.parent().unwrap()).await?;

            let doc_path = self.srcdir.join(format!("{}.rst", pagename)); // TODO: Detect actual extension
            if doc_path.exists() {
                fs::copy(&doc_path, &source_path).await?;
            }
        }

        Ok(())
    }

    /// Get output path for a document
    fn get_output_path(&self, docname: &str) -> PathBuf {
        self.outdir.join(format!("{}{}", docname, self.out_suffix))
    }

    /// Get relative URI between two documents
    fn get_relative_uri(&self, from: &str, to: &str) -> String {
        utils::relative_uri(from, to, &self.link_suffix)
    }

    /// Get target URI for a document
    pub fn get_target_uri(&self, docname: &str) -> String {
        format!("{}{}", docname, self.link_suffix)
    }

    /// Generate indices (mirrors Sphinx's gen_indices)
    pub async fn gen_indices(&mut self) -> Result<()> {
        info!("Generating indices");

        // Generate general index if enabled
        if self.use_index {
            self.write_genindex().await?;
        }

        // Generate domain-specific indices
        self.write_domain_indices().await?;

        Ok(())
    }

    /// Write general index
    async fn write_genindex(&self) -> Result<()> {
        info!("Writing general index");

        // TODO: Implement actual index generation
        let genindex_context = serde_json::json!({
            "genindexentries": [],
            "genindexcounts": [],
            "split_index": false,
        });

        self.handle_page(
            "genindex",
            genindex_context.as_object().unwrap().clone(),
            "genindex.html",
        )
        .await?;

        Ok(())
    }

    /// Write domain indices
    async fn write_domain_indices(&self) -> Result<()> {
        for domain_index in &self.domain_indices {
            info!("Writing domain index: {}", domain_index.name);

            let index_context = serde_json::json!({
                "indextitle": domain_index.localname,
                "content": domain_index.content,
                "collapse_index": domain_index.collapse,
            });

            self.handle_page(
                &domain_index.name,
                index_context.as_object().unwrap().clone(),
                "domainindex.html",
            )
            .await?;
        }

        Ok(())
    }

    /// Copy static files (mirrors Sphinx's copy_static_files)
    pub async fn copy_static_files(&self) -> Result<()> {
        info!("Copying static files");

        // Copy theme static files
        self.copy_theme_static_files().await?;

        // Copy user static files
        for static_path in &self.config.html_static_path {
            let source_dir = self.confdir.join(static_path);
            if source_dir.exists() {
                utils::copy_dir_all(&source_dir, &self.static_dir).await?;
            }
        }

        // Create pygments CSS
        self.create_pygments_style_file().await?;

        // Copy translations if available
        if self.has_translations() {
            self.copy_translation_js().await?;
        }

        Ok(())
    }

    /// Copy theme static files
    async fn copy_theme_static_files(&self) -> Result<()> {
        // TODO: Implement theme system
        Ok(())
    }

    /// Create pygments style file
    async fn create_pygments_style_file(&self) -> Result<()> {
        let css_content = "/* Basic syntax highlighting */\n.highlight { background: #f8f8f8; }\n";
        let css_path = self.static_dir.join("pygments.css");
        fs::write(css_path, css_content).await?;
        Ok(())
    }

    /// Copy translation JS file
    async fn copy_translation_js(&self) -> Result<()> {
        let locale_dir = self.confdir.join("locale");
        let lang = self.config.language.as_deref().unwrap_or("en");
        let js_file = locale_dir.join(lang).join("LC_MESSAGES").join("sphinx.js");

        if js_file.exists() {
            let dest = self.static_dir.join("translations.js");
            fs::copy(js_file, dest).await?;
        }

        Ok(())
    }

    /// Copy image files
    pub async fn copy_image_files(&self, images: &HashMap<String, String>) -> Result<()> {
        info!("Copying {} images", images.len());

        for (src, dest) in images {
            let src_path = self.srcdir.join(src);
            let dest_path = self.images_dir.join(dest);

            utils::ensure_dir(dest_path.parent().unwrap()).await?;

            if src_path.exists() {
                fs::copy(&src_path, &dest_path).await.with_context(|| {
                    format!(
                        "Failed to copy image {} to {}",
                        src_path.display(),
                        dest_path.display()
                    )
                })?;
            } else {
                warn!("Image file not found: {}", src_path.display());
            }
        }

        Ok(())
    }

    /// Copy download files
    pub async fn copy_download_files(&self, downloads: &HashMap<String, String>) -> Result<()> {
        info!("Copying {} download files", downloads.len());

        for (src, dest) in downloads {
            let src_path = self.srcdir.join(src);
            let dest_path = self.downloads_dir.join(dest);

            utils::ensure_dir(dest_path.parent().unwrap()).await?;

            if src_path.exists() {
                fs::copy(&src_path, &dest_path).await.with_context(|| {
                    format!(
                        "Failed to copy download {} to {}",
                        src_path.display(),
                        dest_path.display()
                    )
                })?;
            } else {
                warn!("Download file not found: {}", src_path.display());
            }
        }

        Ok(())
    }

    /// Dump object inventory (mirrors Sphinx's dump_inventory)
    pub async fn dump_inventory(&self, env: &crate::environment::BuildEnvironment) -> Result<()> {
        info!("Dumping object inventory");

        let inventory_path = self.outdir.join(INVENTORY_FILENAME);
        InventoryFile::dump(&inventory_path, env, self).await?;

        Ok(())
    }

    /// Dump search index
    pub async fn dump_search_index(
        &self,
        _search_index: &crate::search::SearchIndex,
    ) -> Result<()> {
        if !self.search {
            return Ok(());
        }

        info!("Dumping search index");

        // TODO: Implement search index dumping
        let search_index_path = self.outdir.join(&self.searchindex_filename);
        let search_data = serde_json::json!({
            "docnames": [],
            "filenames": [],
            "titles": [],
            "terms": {},
            "objects": {},
            "objnames": {},
            "objtypes": {},
        });

        fs::write(
            search_index_path,
            serde_json::to_string_pretty(&search_data)?,
        )
        .await?;

        Ok(())
    }

    /// Write build info file
    pub async fn write_build_info(&self) -> Result<()> {
        let build_info = serde_json::json!({
            "config": {
                "extensions": [],
                "templates_path": [],
                "source_suffix": ".rst",
                "master_doc": self.config.root_doc.as_deref().unwrap_or("index"),
                "version": self.config.version.as_deref().unwrap_or(""),
                "release": self.config.release.as_deref().unwrap_or(""),
                "project": self.config.project,
                "copyright": self.config.copyright.as_deref().unwrap_or(""),
                "language": self.config.language.as_deref().unwrap_or("en"),
            },
            "tags": [],
            "version": env!("CARGO_PKG_VERSION"),
        });

        let build_info_path = self.outdir.join(".buildinfo");
        fs::write(build_info_path, serde_json::to_string_pretty(&build_info)?).await?;

        Ok(())
    }

    /// Finish the build process
    pub async fn finish(
        &mut self,
        env: &crate::environment::BuildEnvironment,
        search_index: &crate::search::SearchIndex,
    ) -> Result<()> {
        info!("Finishing HTML build");

        // Generate indices
        self.gen_indices().await?;

        // Copy static files
        self.copy_static_files().await?;

        // Dump inventory and search index
        self.dump_inventory(env).await?;
        self.dump_search_index(search_index).await?;

        // Write build info
        self.write_build_info().await?;

        Ok(())
    }
}

impl PartialEq for CSSFile {
    fn eq(&self, other: &Self) -> bool {
        self.filename == other.filename
    }
}

impl PartialEq for JSFile {
    fn eq(&self, other: &Self) -> bool {
        self.filename == other.filename
    }
}
