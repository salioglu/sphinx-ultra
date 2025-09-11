use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::BuildConfig;

/// Python configuration parser that can execute conf.py files
pub struct PythonConfigParser {
    conf_namespace: HashMap<String, serde_json::Value>,
}

/// Represents a parsed conf.py configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfPyConfig {
    // Project information
    pub project: Option<String>,
    pub version: Option<String>,
    pub release: Option<String>,
    pub copyright: Option<String>,
    pub author: Option<String>,

    // General configuration
    pub extensions: Vec<String>,
    pub templates_path: Vec<String>,
    pub exclude_patterns: Vec<String>,
    pub source_suffix: HashMap<String, String>,
    pub root_doc: Option<String>,
    pub language: Option<String>,
    pub locale_dirs: Vec<String>,
    pub gettext_compact: Option<bool>,

    // HTML output options
    pub html_theme: Option<String>,
    pub html_theme_options: HashMap<String, serde_json::Value>,
    pub html_title: Option<String>,
    pub html_short_title: Option<String>,
    pub html_logo: Option<String>,
    pub html_favicon: Option<String>,
    pub html_css_files: Vec<String>,
    pub html_js_files: Vec<String>,
    pub html_static_path: Vec<String>,
    pub html_extra_path: Vec<String>,
    pub html_use_index: Option<bool>,
    pub html_split_index: Option<bool>,
    pub html_copy_source: Option<bool>,
    pub html_show_sourcelink: Option<bool>,
    pub html_sourcelink_suffix: Option<String>,
    pub html_use_opensearch: Option<String>,
    pub html_file_suffix: Option<String>,
    pub html_link_suffix: Option<String>,
    pub html_show_copyright: Option<bool>,
    pub html_show_sphinx: Option<bool>,
    pub html_context: HashMap<String, serde_json::Value>,
    pub html_output_encoding: Option<String>,
    pub html_compact_lists: Option<bool>,
    pub html_secnumber_suffix: Option<String>,
    pub html_search_language: Option<String>,
    pub html_search_options: HashMap<String, serde_json::Value>,
    pub html_search_scorer: Option<String>,
    pub html_scaled_image_link: Option<bool>,
    pub html_baseurl: Option<String>,
    pub html_codeblock_linenos_style: Option<String>,
    pub html_math_renderer: Option<String>,
    pub html_math_renderer_options: HashMap<String, serde_json::Value>,

    // LaTeX output options
    pub latex_engine: Option<String>,
    pub latex_documents: Vec<(String, String, String, String, String)>,
    pub latex_logo: Option<String>,
    pub latex_appendices: Vec<String>,
    pub latex_domain_indices: Option<bool>,
    pub latex_show_pagerefs: Option<bool>,
    pub latex_show_urls: Option<String>,
    pub latex_use_latex_multicolumn: Option<bool>,
    pub latex_use_xindy: Option<bool>,
    pub latex_toplevel_sectioning: Option<String>,
    pub latex_docclass: HashMap<String, String>,
    pub latex_additional_files: Vec<String>,
    pub latex_elements: HashMap<String, String>,

    // ePub output options
    pub epub_title: Option<String>,
    pub epub_author: Option<String>,
    pub epub_language: Option<String>,
    pub epub_publisher: Option<String>,
    pub epub_copyright: Option<String>,
    pub epub_identifier: Option<String>,
    pub epub_scheme: Option<String>,
    pub epub_uid: Option<String>,
    pub epub_cover: Option<(String, String)>,
    pub epub_css_files: Vec<String>,
    pub epub_pre_files: Vec<(String, String)>,
    pub epub_post_files: Vec<(String, String)>,
    pub epub_exclude_files: Vec<String>,
    pub epub_tocdepth: Option<i32>,
    pub epub_tocdup: Option<bool>,
    pub epub_tocscope: Option<String>,
    pub epub_fix_images: Option<bool>,
    pub epub_max_image_width: Option<i32>,
    pub epub_show_urls: Option<String>,
    pub epub_use_index: Option<bool>,
    pub epub_description: Option<String>,
    pub epub_contributor: Option<String>,
    pub epub_writing_mode: Option<String>,

    // Extension-specific configurations
    pub extension_configs: HashMap<String, HashMap<String, serde_json::Value>>,

    // Build options
    pub needs_sphinx: Option<String>,
    pub needs_extensions: HashMap<String, String>,
    pub manpages_url: Option<String>,
    pub nitpicky: Option<bool>,
    pub nitpick_ignore: Vec<(String, String)>,
    pub nitpick_ignore_regex: Vec<(String, String)>,
    pub numfig: Option<bool>,
    pub numfig_format: HashMap<String, String>,
    pub numfig_secnum_depth: Option<i32>,
    pub math_number_all: Option<bool>,
    pub math_eqref_format: Option<String>,
    pub math_numfig: Option<bool>,
    pub tls_verify: Option<bool>,
    pub tls_cacerts: Option<String>,
    pub user_agent: Option<String>,

    // Internationalization
    pub gettext_uuid: Option<bool>,
    pub gettext_location: Option<bool>,
    pub gettext_auto_build: Option<bool>,
    pub gettext_additional_targets: Vec<String>,

    // Custom configurations (catch-all for extension-specific or custom settings)
    pub custom_configs: HashMap<String, serde_json::Value>,
}

impl PythonConfigParser {
    /// Create a new Python configuration parser
    pub fn new() -> Result<Self> {
        let conf_namespace = HashMap::new();

        Ok(Self { conf_namespace })
    }

    /// Parse a conf.py file and extract configuration
    pub fn parse_conf_py<P: AsRef<Path>>(&mut self, conf_py_path: P) -> Result<ConfPyConfig> {
        let conf_py_path = conf_py_path.as_ref();
        let _conf_dir = conf_py_path
            .parent()
            .ok_or_else(|| anyhow!("Invalid conf.py path"))?;

        // Read the conf.py file
        let conf_py_content = std::fs::read_to_string(conf_py_path)?;

        // For now, implement a simple parser that extracts basic configuration
        // In a full implementation, this would execute the Python code
        self.simple_parse_conf_py(&conf_py_content)?;

        // Extract configuration values
        self.extract_configuration()
    }

    /// Simple parser for basic conf.py configurations (stub implementation)
    fn simple_parse_conf_py(&mut self, content: &str) -> Result<()> {
        // Parse simple assignment statements like: variable = "value"
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Parse simple assignments
            if let Some((key, value)) = self.parse_simple_assignment(line) {
                self.conf_namespace.insert(key, value);
            }
        }

        Ok(())
    }

    /// Parse simple Python assignments
    fn parse_simple_assignment(&self, line: &str) -> Option<(String, serde_json::Value)> {
        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim().to_string();
            let value_str = line[eq_pos + 1..].trim();

            // Parse common value types
            if value_str.starts_with('"') && value_str.ends_with('"') {
                // String value
                let value = value_str[1..value_str.len() - 1].to_string();
                return Some((key, serde_json::Value::String(value)));
            } else if value_str.starts_with('\'') && value_str.ends_with('\'') {
                // String value with single quotes
                let value = value_str[1..value_str.len() - 1].to_string();
                return Some((key, serde_json::Value::String(value)));
            } else if value_str == "True" {
                return Some((key, serde_json::Value::Bool(true)));
            } else if value_str == "False" {
                return Some((key, serde_json::Value::Bool(false)));
            } else if let Ok(num) = value_str.parse::<i64>() {
                return Some((key, serde_json::Value::Number(num.into())));
            } else if value_str.starts_with('[') && value_str.ends_with(']') {
                // Simple list parsing
                let list_content = &value_str[1..value_str.len() - 1];
                let items: Vec<serde_json::Value> = list_content
                    .split(',')
                    .map(|item| {
                        let item = item.trim();
                        if (item.starts_with('"') && item.ends_with('"'))
                            || (item.starts_with('\'') && item.ends_with('\''))
                        {
                            serde_json::Value::String(item[1..item.len() - 1].to_string())
                        } else {
                            serde_json::Value::String(item.to_string())
                        }
                    })
                    .collect();
                return Some((key, serde_json::Value::Array(items)));
            }
        }
        None
    }

    /// Extract configuration values from the parsed Python namespace
    fn extract_configuration(&self) -> Result<ConfPyConfig> {
        let mut config = ConfPyConfig::default();

        // Helper function to extract optional string values
        let extract_string = |key: &str| -> Option<String> {
            self.conf_namespace
                .get(key)
                .and_then(|val| val.as_str().map(|s| s.to_string()))
        };

        // Helper function to extract optional bool values
        let extract_bool = |key: &str| -> Option<bool> {
            self.conf_namespace.get(key).and_then(|val| val.as_bool())
        };

        // Helper function to extract optional int values
        let extract_int = |key: &str| -> Option<i32> {
            self.conf_namespace
                .get(key)
                .and_then(|val| val.as_i64().map(|i| i as i32))
        };

        // Helper function to extract list of strings
        let extract_string_list = |key: &str| -> Vec<String> {
            self.conf_namespace
                .get(key)
                .and_then(|val| val.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        };

        // Helper function to extract dictionary
        let extract_dict = |key: &str| -> HashMap<String, serde_json::Value> {
            self.conf_namespace
                .get(key)
                .and_then(|val| val.as_object())
                .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default()
        };

        // Extract project information
        config.project = extract_string("project");
        config.version = extract_string("version");
        config.release = extract_string("release");
        config.copyright = extract_string("copyright");
        config.author = extract_string("author");

        // Extract general configuration
        config.extensions = extract_string_list("extensions");
        config.templates_path = extract_string_list("templates_path");
        config.exclude_patterns = extract_string_list("exclude_patterns");
        config.root_doc = extract_string("root_doc").or_else(|| extract_string("master_doc"));
        config.language = extract_string("language");
        config.locale_dirs = extract_string_list("locale_dirs");
        config.gettext_compact = extract_bool("gettext_compact");

        // Extract HTML output options
        config.html_theme = extract_string("html_theme");
        config.html_theme_options = extract_dict("html_theme_options");
        config.html_title = extract_string("html_title");
        config.html_short_title = extract_string("html_short_title");
        config.html_logo = extract_string("html_logo");
        config.html_favicon = extract_string("html_favicon");
        config.html_css_files = extract_string_list("html_css_files");
        config.html_js_files = extract_string_list("html_js_files");
        config.html_static_path = extract_string_list("html_static_path");
        config.html_extra_path = extract_string_list("html_extra_path");
        config.html_use_index = extract_bool("html_use_index");
        config.html_split_index = extract_bool("html_split_index");
        config.html_copy_source = extract_bool("html_copy_source");
        config.html_show_sourcelink = extract_bool("html_show_sourcelink");
        config.html_sourcelink_suffix = extract_string("html_sourcelink_suffix");
        config.html_use_opensearch = extract_string("html_use_opensearch");
        config.html_file_suffix = extract_string("html_file_suffix");
        config.html_link_suffix = extract_string("html_link_suffix");
        config.html_show_copyright = extract_bool("html_show_copyright");
        config.html_show_sphinx = extract_bool("html_show_sphinx");
        config.html_context = extract_dict("html_context");
        config.html_output_encoding = extract_string("html_output_encoding");
        config.html_compact_lists = extract_bool("html_compact_lists");
        config.html_secnumber_suffix = extract_string("html_secnumber_suffix");
        config.html_search_language = extract_string("html_search_language");
        config.html_search_options = extract_dict("html_search_options");
        config.html_search_scorer = extract_string("html_search_scorer");
        config.html_scaled_image_link = extract_bool("html_scaled_image_link");
        config.html_baseurl = extract_string("html_baseurl");
        config.html_codeblock_linenos_style = extract_string("html_codeblock_linenos_style");
        config.html_math_renderer = extract_string("html_math_renderer");
        config.html_math_renderer_options = extract_dict("html_math_renderer_options");

        // Extract build options
        config.needs_sphinx = extract_string("needs_sphinx");
        config.nitpicky = extract_bool("nitpicky");
        config.numfig = extract_bool("numfig");
        config.numfig_secnum_depth = extract_int("numfig_secnum_depth");
        config.math_number_all = extract_bool("math_number_all");
        config.math_eqref_format = extract_string("math_eqref_format");
        config.math_numfig = extract_bool("math_numfig");
        config.tls_verify = extract_bool("tls_verify");
        config.tls_cacerts = extract_string("tls_cacerts");
        config.user_agent = extract_string("user_agent");

        // Extract internationalization
        config.gettext_uuid = extract_bool("gettext_uuid");
        config.gettext_location = extract_bool("gettext_location");
        config.gettext_auto_build = extract_bool("gettext_auto_build");
        config.gettext_additional_targets = extract_string_list("gettext_additional_targets");

        // Extract custom configurations
        for (key, value) in &self.conf_namespace {
            if !Self::is_standard_config_key(key) {
                config.custom_configs.insert(key.clone(), value.clone());
            }
        }

        Ok(config)
    }

    /// Check if a configuration key is a standard Sphinx configuration
    fn is_standard_config_key(key: &str) -> bool {
        matches!(
            key,
            "project"
                | "version"
                | "release"
                | "copyright"
                | "author"
                | "extensions"
                | "templates_path"
                | "exclude_patterns"
                | "source_suffix"
                | "root_doc"
                | "master_doc"
                | "language"
                | "locale_dirs"
                | "gettext_compact"
                | "html_theme"
                | "html_theme_options"
                | "html_title"
                | "html_short_title"
                | "html_logo"
                | "html_favicon"
                | "html_css_files"
                | "html_js_files"
                | "html_static_path"
                | "html_extra_path"
                | "html_use_index"
                | "html_split_index"
                | "html_copy_source"
                | "html_show_sourcelink"
                | "html_sourcelink_suffix"
                | "html_use_opensearch"
                | "html_file_suffix"
                | "html_link_suffix"
                | "html_show_copyright"
                | "html_show_sphinx"
                | "html_context"
                | "html_output_encoding"
                | "html_compact_lists"
                | "html_secnumber_suffix"
                | "html_search_language"
                | "html_search_options"
                | "html_search_scorer"
                | "html_scaled_image_link"
                | "html_baseurl"
                | "html_codeblock_linenos_style"
                | "html_math_renderer"
                | "html_math_renderer_options"
                | "needs_sphinx"
                | "nitpicky"
                | "numfig"
                | "numfig_secnum_depth"
                | "math_number_all"
                | "math_eqref_format"
                | "math_numfig"
                | "tls_verify"
                | "tls_cacerts"
                | "user_agent"
                | "gettext_uuid"
                | "gettext_location"
                | "gettext_auto_build"
                | "gettext_additional_targets"
        )
    }
}

impl Default for ConfPyConfig {
    fn default() -> Self {
        Self {
            project: None,
            version: None,
            release: None,
            copyright: None,
            author: None,
            extensions: Vec::new(),
            templates_path: vec!["_templates".to_string()],
            exclude_patterns: Vec::new(),
            source_suffix: HashMap::new(),
            root_doc: Some("index".to_string()),
            language: None,
            locale_dirs: vec!["locales".to_string()],
            gettext_compact: Some(true),
            html_theme: Some("alabaster".to_string()),
            html_theme_options: HashMap::new(),
            html_title: None,
            html_short_title: None,
            html_logo: None,
            html_favicon: None,
            html_css_files: Vec::new(),
            html_js_files: Vec::new(),
            html_static_path: vec!["_static".to_string()],
            html_extra_path: Vec::new(),
            html_use_index: Some(true),
            html_split_index: Some(false),
            html_copy_source: Some(true),
            html_show_sourcelink: Some(true),
            html_sourcelink_suffix: Some(".txt".to_string()),
            html_use_opensearch: None,
            html_file_suffix: Some(".html".to_string()),
            html_link_suffix: Some(".html".to_string()),
            html_show_copyright: Some(true),
            html_show_sphinx: Some(true),
            html_context: HashMap::new(),
            html_output_encoding: Some("utf-8".to_string()),
            html_compact_lists: Some(true),
            html_secnumber_suffix: Some(". ".to_string()),
            html_search_language: None,
            html_search_options: HashMap::new(),
            html_search_scorer: None,
            html_scaled_image_link: Some(true),
            html_baseurl: None,
            html_codeblock_linenos_style: Some("table".to_string()),
            html_math_renderer: Some("mathjax".to_string()),
            html_math_renderer_options: HashMap::new(),
            latex_engine: Some("pdflatex".to_string()),
            latex_documents: Vec::new(),
            latex_logo: None,
            latex_appendices: Vec::new(),
            latex_domain_indices: Some(true),
            latex_show_pagerefs: Some(false),
            latex_show_urls: Some("no".to_string()),
            latex_use_latex_multicolumn: Some(false),
            latex_use_xindy: Some(false),
            latex_toplevel_sectioning: None,
            latex_docclass: HashMap::new(),
            latex_additional_files: Vec::new(),
            latex_elements: HashMap::new(),
            epub_title: None,
            epub_author: None,
            epub_language: None,
            epub_publisher: None,
            epub_copyright: None,
            epub_identifier: None,
            epub_scheme: None,
            epub_uid: None,
            epub_cover: None,
            epub_css_files: Vec::new(),
            epub_pre_files: Vec::new(),
            epub_post_files: Vec::new(),
            epub_exclude_files: Vec::new(),
            epub_tocdepth: Some(3),
            epub_tocdup: Some(true),
            epub_tocscope: Some("default".to_string()),
            epub_fix_images: Some(false),
            epub_max_image_width: Some(0),
            epub_show_urls: Some("inline".to_string()),
            epub_use_index: Some(true),
            epub_description: None,
            epub_contributor: None,
            epub_writing_mode: Some("horizontal".to_string()),
            extension_configs: HashMap::new(),
            needs_sphinx: None,
            needs_extensions: HashMap::new(),
            manpages_url: None,
            nitpicky: Some(false),
            nitpick_ignore: Vec::new(),
            nitpick_ignore_regex: Vec::new(),
            numfig: Some(false),
            numfig_format: HashMap::new(),
            numfig_secnum_depth: Some(1),
            math_number_all: Some(false),
            math_eqref_format: None,
            math_numfig: Some(true),
            tls_verify: Some(true),
            tls_cacerts: None,
            user_agent: None,
            gettext_uuid: Some(false),
            gettext_location: Some(true),
            gettext_auto_build: Some(true),
            gettext_additional_targets: Vec::new(),
            custom_configs: HashMap::new(),
        }
    }
}

impl ConfPyConfig {
    /// Convert conf.py configuration to BuildConfig
    pub fn to_build_config(&self) -> BuildConfig {
        let mut config = BuildConfig::default();

        // Map basic project information
        if let Some(project) = &self.project {
            config.project = project.clone();
        }
        if let Some(version) = &self.version {
            config.version = Some(version.clone());
        }
        if let Some(release) = &self.release {
            config.release = Some(release.clone());
        }
        if let Some(copyright) = &self.copyright {
            config.copyright = Some(copyright.clone());
        }
        if let Some(language) = &self.language {
            config.language = Some(language.clone());
        }
        if let Some(root_doc) = &self.root_doc {
            config.root_doc = Some(root_doc.clone());
        }

        // Map extensions
        config.extensions = self.extensions.clone();

        // Map template paths
        config.template_dirs = self.templates_path.iter().map(PathBuf::from).collect();

        // Map static paths
        config.static_dirs = self.html_static_path.iter().map(PathBuf::from).collect();
        config.html_static_path = self.html_static_path.iter().map(PathBuf::from).collect();

        // Map HTML configuration
        if let Some(html_theme) = &self.html_theme {
            config.output.html_theme = html_theme.clone();
            config.theme.name = html_theme.clone();
        }
        if let Some(html_title) = &self.html_title {
            config.html_title = Some(html_title.clone());
        }
        if let Some(html_short_title) = &self.html_short_title {
            config.html_short_title = Some(html_short_title.clone());
        }
        if let Some(html_logo) = &self.html_logo {
            config.html_logo = Some(html_logo.clone());
        }
        if let Some(html_favicon) = &self.html_favicon {
            config.html_favicon = Some(html_favicon.clone());
        }
        config.html_css_files = self.html_css_files.clone();
        config.html_js_files = self.html_js_files.clone();
        if let Some(html_show_copyright) = self.html_show_copyright {
            config.html_show_copyright = Some(html_show_copyright);
        }
        if let Some(html_show_sphinx) = self.html_show_sphinx {
            config.html_show_sphinx = Some(html_show_sphinx);
        }
        if let Some(html_copy_source) = self.html_copy_source {
            config.html_copy_source = Some(html_copy_source);
        }
        if let Some(html_show_sourcelink) = self.html_show_sourcelink {
            config.html_show_sourcelink = Some(html_show_sourcelink);
        }
        if let Some(html_sourcelink_suffix) = &self.html_sourcelink_suffix {
            config.html_sourcelink_suffix = Some(html_sourcelink_suffix.clone());
        }
        if let Some(html_use_index) = self.html_use_index {
            config.html_use_index = Some(html_use_index);
        }
        if let Some(html_use_opensearch) = &self.html_use_opensearch {
            config.html_use_opensearch = Some(!html_use_opensearch.is_empty());
        }
        if let Some(html_last_updated_fmt) = &self.html_context.get("last_updated") {
            if let Some(fmt_str) = html_last_updated_fmt.as_str() {
                config.html_last_updated_fmt = Some(fmt_str.to_string());
            }
        }

        // Map templates path
        config.templates_path = self.templates_path.iter().map(PathBuf::from).collect();

        config
    }
}
