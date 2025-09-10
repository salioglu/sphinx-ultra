use anyhow::Result;
use pyo3::prelude::*;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::BuildConfig;

/// Represents a Sphinx extension
#[derive(Debug, Clone)]
pub struct SphinxExtension {
    pub name: String,
    pub module_path: String,
    pub setup_function: Option<String>,
    pub metadata: ExtensionMetadata,
    pub config: HashMap<String, Value>,
}

/// Extension metadata
#[derive(Debug, Clone)]
pub struct ExtensionMetadata {
    pub version: String,
    pub parallel_read_safe: bool,
    pub parallel_write_safe: bool,
    pub env_version: Option<i32>,
}

/// Sphinx application context for extensions
pub struct SphinxApp {
    pub config: BuildConfig,
    pub extensions: HashMap<String, SphinxExtension>,
    pub env: SphinxEnvironment,
}

/// Sphinx build environment
#[derive(Debug)]
pub struct SphinxEnvironment {
    pub docname_to_path: HashMap<String, PathBuf>,
    pub path_to_docname: HashMap<PathBuf, String>,
    pub dependencies: HashMap<String, Vec<String>>,
    pub included: HashMap<String, Vec<String>>,
    pub toctree_includes: HashMap<String, Vec<String>>,
    pub files_to_rebuild: HashMap<String, Vec<String>>,
    pub glob_toctrees: Vec<String>,
    pub numbered_toctrees: Vec<String>,
    pub metadata: HashMap<String, HashMap<String, String>>,
}

/// Extension loader and manager
pub struct ExtensionLoader {
    loaded_extensions: HashMap<String, SphinxExtension>,
}

impl ExtensionLoader {
    /// Create a new extension loader
    pub fn new() -> Result<Self> {
        Ok(Self {
            loaded_extensions: HashMap::new(),
        })
    }

    /// Load a Sphinx extension by name
    pub fn load_extension(&mut self, extension_name: &str) -> Result<SphinxExtension> {
        if let Some(extension) = self.loaded_extensions.get(extension_name) {
            return Ok(extension.clone());
        }

        let extension = self.import_and_setup_extension(extension_name)?;
        self.loaded_extensions
            .insert(extension_name.to_string(), extension.clone());

        Ok(extension)
    }

    /// Import and set up a Python extension
    fn import_and_setup_extension(&self, extension_name: &str) -> Result<SphinxExtension> {
        // For now, create a stub extension for built-in extensions
        // In a full implementation, this would use PyO3 to import Python modules

        let metadata = ExtensionMetadata {
            version: "1.0.0".to_string(),
            parallel_read_safe: true,
            parallel_write_safe: true,
            env_version: Some(1),
        };

        Ok(SphinxExtension {
            name: extension_name.to_string(),
            module_path: extension_name.to_string(),
            setup_function: Some("setup".to_string()),
            metadata,
            config: HashMap::new(),
        })
    }

    /// Extract metadata from extension module
    fn extract_extension_metadata(&self, _extension_name: &str) -> Result<ExtensionMetadata> {
        // Stub implementation - in a real version this would introspect the Python module
        Ok(ExtensionMetadata {
            version: "1.0.0".to_string(),
            parallel_read_safe: true,
            parallel_write_safe: true,
            env_version: Some(1),
        })
    }

    /// Get all loaded extensions
    pub fn get_loaded_extensions(&self) -> &HashMap<String, SphinxExtension> {
        &self.loaded_extensions
    }
}

impl SphinxApp {
    /// Create a new Sphinx application
    pub fn new(config: BuildConfig) -> Result<Self> {
        let env = SphinxEnvironment::new();

        Ok(Self {
            config,
            extensions: HashMap::new(),
            env,
        })
    }

    /// Add an extension to the application
    pub fn add_extension(&mut self, extension: SphinxExtension) -> Result<()> {
        // Call the extension's setup function if it exists
        if let Some(setup_fn) = &extension.setup_function {
            self.call_extension_setup(&extension, setup_fn)?;
        }

        self.extensions.insert(extension.name.clone(), extension);
        Ok(())
    }

    /// Call an extension's setup function
    fn call_extension_setup(&self, extension: &SphinxExtension, _setup_fn: &str) -> Result<()> {
        // Stub implementation - in a real version this would call the Python setup function
        println!("Setting up extension: {}", extension.name);
        Ok(())
    }

    /// Create a configuration dictionary for Python (stub)
    fn create_config_dict(&self) -> Result<HashMap<String, String>> {
        // Stub implementation
        let mut config_dict = HashMap::new();
        config_dict.insert("project".to_string(), self.config.project.clone());
        Ok(config_dict)
    }

    /// Get extension by name
    pub fn get_extension(&self, name: &str) -> Option<&SphinxExtension> {
        self.extensions.get(name)
    }

    /// Check if extension is loaded
    pub fn has_extension(&self, name: &str) -> bool {
        self.extensions.contains_key(name)
    }
}

impl SphinxEnvironment {
    /// Create a new Sphinx environment
    pub fn new() -> Self {
        Self {
            docname_to_path: HashMap::new(),
            path_to_docname: HashMap::new(),
            dependencies: HashMap::new(),
            included: HashMap::new(),
            toctree_includes: HashMap::new(),
            files_to_rebuild: HashMap::new(),
            glob_toctrees: Vec::new(),
            numbered_toctrees: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    /// Add a document to the environment
    pub fn add_document(&mut self, docname: String, path: PathBuf) {
        self.path_to_docname.insert(path.clone(), docname.clone());
        self.docname_to_path.insert(docname, path);
    }

    /// Get document path by name
    pub fn get_doc_path(&self, docname: &str) -> Option<&PathBuf> {
        self.docname_to_path.get(docname)
    }

    /// Get document name by path
    pub fn get_docname(&self, path: &PathBuf) -> Option<&String> {
        self.path_to_docname.get(path)
    }

    /// Add dependency between documents
    pub fn add_dependency(&mut self, docname: String, dependency: String) {
        self.dependencies
            .entry(docname)
            .or_insert_with(Vec::new)
            .push(dependency);
    }

    /// Get dependencies for a document
    pub fn get_dependencies(&self, docname: &str) -> Option<&Vec<String>> {
        self.dependencies.get(docname)
    }

    /// Add metadata for a document
    pub fn add_metadata(&mut self, docname: String, key: String, value: String) {
        self.metadata
            .entry(docname)
            .or_insert_with(HashMap::new)
            .insert(key, value);
    }

    /// Get metadata for a document
    pub fn get_metadata(&self, docname: &str) -> Option<&HashMap<String, String>> {
        self.metadata.get(docname)
    }
}

/// Built-in Sphinx extensions that we need to handle specially
pub struct BuiltinExtensions;

impl BuiltinExtensions {
    /// Get list of built-in Sphinx extensions
    pub fn get_builtin_extensions() -> Vec<&'static str> {
        vec![
            "sphinx.ext.autodoc",
            "sphinx.ext.autosummary",
            "sphinx.ext.doctest",
            "sphinx.ext.intersphinx",
            "sphinx.ext.todo",
            "sphinx.ext.coverage",
            "sphinx.ext.imgmath",
            "sphinx.ext.mathjax",
            "sphinx.ext.ifconfig",
            "sphinx.ext.viewcode",
            "sphinx.ext.githubpages",
            "sphinx.ext.napoleon",
            "sphinx.ext.extlinks",
            "sphinx.ext.linkcode",
            "sphinx.ext.graphviz",
            "sphinx.ext.inheritance_diagram",
        ]
    }

    /// Check if an extension is built-in
    pub fn is_builtin_extension(name: &str) -> bool {
        Self::get_builtin_extensions().contains(&name)
    }

    /// Get default configuration for built-in extensions
    pub fn get_default_config(extension_name: &str) -> HashMap<String, Value> {
        let mut config = HashMap::new();

        match extension_name {
            "sphinx.ext.autodoc" => {
                config.insert(
                    "autodoc_default_options".to_string(),
                    serde_json::json!({
                        "members": true,
                        "undoc-members": true,
                        "show-inheritance": true
                    }),
                );
                config.insert(
                    "autodoc_member_order".to_string(),
                    Value::String("alphabetical".to_string()),
                );
                config.insert(
                    "autodoc_typehints".to_string(),
                    Value::String("description".to_string()),
                );
            }
            "sphinx.ext.autosummary" => {
                config.insert("autosummary_generate".to_string(), Value::Bool(true));
                config.insert(
                    "autosummary_imported_members".to_string(),
                    Value::Bool(false),
                );
            }
            "sphinx.ext.intersphinx" => {
                config.insert(
                    "intersphinx_mapping".to_string(),
                    serde_json::json!({
                        "python": ["https://docs.python.org/3", null]
                    }),
                );
                config.insert(
                    "intersphinx_timeout".to_string(),
                    Value::Number(serde_json::Number::from(5)),
                );
            }
            "sphinx.ext.todo" => {
                config.insert("todo_include_todos".to_string(), Value::Bool(true));
                config.insert("todo_emit_warnings".to_string(), Value::Bool(false));
            }
            "sphinx.ext.napoleon" => {
                config.insert("napoleon_google_docstring".to_string(), Value::Bool(true));
                config.insert("napoleon_numpy_docstring".to_string(), Value::Bool(true));
                config.insert(
                    "napoleon_include_init_with_doc".to_string(),
                    Value::Bool(false),
                );
                config.insert(
                    "napoleon_include_private_with_doc".to_string(),
                    Value::Bool(false),
                );
                config.insert(
                    "napoleon_include_special_with_doc".to_string(),
                    Value::Bool(true),
                );
                config.insert(
                    "napoleon_use_admonition_for_examples".to_string(),
                    Value::Bool(false),
                );
                config.insert(
                    "napoleon_use_admonition_for_notes".to_string(),
                    Value::Bool(false),
                );
                config.insert(
                    "napoleon_use_admonition_for_references".to_string(),
                    Value::Bool(false),
                );
                config.insert("napoleon_use_ivar".to_string(), Value::Bool(false));
                config.insert("napoleon_use_param".to_string(), Value::Bool(true));
                config.insert("napoleon_use_rtype".to_string(), Value::Bool(true));
                config.insert("napoleon_use_keyword".to_string(), Value::Bool(true));
                config.insert("napoleon_custom_sections".to_string(), Value::Array(vec![]));
            }
            "sphinx.ext.viewcode" => {
                config.insert("viewcode_import".to_string(), Value::Bool(false));
                config.insert("viewcode_enable_epub".to_string(), Value::Bool(false));
            }
            "sphinx.ext.imgmath" => {
                config.insert(
                    "imgmath_image_format".to_string(),
                    Value::String("png".to_string()),
                );
                config.insert("imgmath_use_preview".to_string(), Value::Bool(false));
                config.insert("imgmath_add_tooltips".to_string(), Value::Bool(true));
                config.insert(
                    "imgmath_font_size".to_string(),
                    Value::Number(serde_json::Number::from(12)),
                );
            }
            "sphinx.ext.mathjax" => {
                config.insert("mathjax_path".to_string(), 
                    Value::String("https://cdnjs.cloudflare.com/ajax/libs/mathjax/2.7.7/MathJax.js?config=TeX-AMS-MML_HTMLorMML".to_string()));
                config.insert(
                    "mathjax_config".to_string(),
                    serde_json::json!({
                        "tex2jax": {
                            "inlineMath": [["$", "$"], ["\\(", "\\)"]],
                            "displayMath": [["$$", "$$"], ["\\[", "\\]"]],
                            "processEscapes": true,
                            "processEnvironments": true
                        }
                    }),
                );
            }
            _ => {}
        }

        config
    }
}
