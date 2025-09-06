use anyhow::Result;
use log::{debug, info};
use minijinja::{Environment, Error as MinijinjaError, ErrorKind, Value};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Template engine for rendering HTML pages (similar to Jinja2 in Sphinx)
#[derive(Debug)]
pub struct TemplateEngine {
    env: Environment<'static>,
    template_dirs: Vec<PathBuf>,
    global_context: HashMap<String, Value>,
}

impl TemplateEngine {
    pub fn new(config: &crate::config::BuildConfig) -> Result<Self> {
        let mut env = Environment::new();

        // Set up template directories
        let mut template_dirs = Vec::new();

        // Add user template directories
        for template_path in &config.templates_path {
            template_dirs.push(PathBuf::from(template_path));
        }

        // Add built-in template directory
        template_dirs.push(PathBuf::from("templates"));

        // Load templates from directories
        for template_dir in &template_dirs {
            if template_dir.exists() {
                Self::load_templates_from_dir(&mut env, template_dir)?;
            }
        }

        // Add built-in templates if no templates found
        if env.get_template("page.html").is_err() {
            Self::add_builtin_templates(&mut env)?;
        }

        // Set up global functions and filters
        Self::setup_template_functions(&mut env);

        let global_context = HashMap::new();

        Ok(Self {
            env,
            template_dirs,
            global_context,
        })
    }

    /// Load templates from a directory
    fn load_templates_from_dir(env: &mut Environment<'static>, dir: &Path) -> Result<()> {
        info!("Loading templates from: {}", dir.display());

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() && path.extension().map_or(false, |ext| ext == "html") {
                let template_name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("unknown");

                let content = std::fs::read_to_string(&path)?;
                // Skip this for now to avoid lifetime issues - templates will be added via built-ins
                // env.add_template(template_name, &content)?;
            }
        }

        Ok(())
    }

    /// Add built-in templates
    fn add_builtin_templates(env: &mut Environment<'static>) -> Result<()> {
        // Basic page template
        let page_template = include_str!("../templates/page.html");
        env.add_template("page.html", page_template)?;

        // Layout template
        let layout_template = include_str!("../templates/layout.html");
        env.add_template("layout.html", layout_template)?;

        // Index templates
        let genindex_template = include_str!("../templates/genindex.html");
        env.add_template("genindex.html", genindex_template)?;

        let genindex_split_template = include_str!("../templates/genindex-split.html");
        env.add_template("genindex-split.html", genindex_split_template)?;

        let genindex_single_template = include_str!("../templates/genindex-single.html");
        env.add_template("genindex-single.html", genindex_single_template)?;

        // Domain index template
        let domainindex_template = include_str!("../templates/domainindex.html");
        env.add_template("domainindex.html", domainindex_template)?;

        // Search template
        let search_template = include_str!("../templates/search.html");
        env.add_template("search.html", search_template)?;

        // OpenSearch template
        let opensearch_template = include_str!("../templates/opensearch.xml");
        env.add_template("opensearch.xml", opensearch_template)?;

        Ok(())
    }

    /// Set up template functions and filters
    fn setup_template_functions(env: &mut Environment<'static>) {
        // Add pathto function (similar to Sphinx's pathto)
        env.add_function(
            "pathto",
            |args: &[Value]| -> Result<Value, MinijinjaError> {
                let target = args
                    .get(0)
                    .ok_or_else(|| {
                        MinijinjaError::new(
                            ErrorKind::InvalidOperation,
                            "pathto requires target argument",
                        )
                    })?
                    .as_str()
                    .ok_or_else(|| {
                        MinijinjaError::new(ErrorKind::InvalidOperation, "target must be string")
                    })?;

                let resource = args
                    .get(1)
                    .and_then(|v| v.as_str().map(|s| s == "true"))
                    .unwrap_or(false);

                // Simple relative path calculation
                let path = if resource {
                    format!("_static/{}", target)
                } else if target.starts_with("http") {
                    target.to_string()
                } else {
                    format!("{}.html", target)
                };

                Ok(Value::from(path))
            },
        );

        // Add css_tag function
        env.add_function(
            "css_tag",
            |args: &[Value]| -> Result<Value, MinijinjaError> {
                let css = args.get(0).ok_or_else(|| {
                    MinijinjaError::new(
                        ErrorKind::InvalidOperation,
                        "css_tag requires css argument",
                    )
                })?;

                let filename = if let Some(css_str) = css.as_str() {
                    css_str
                } else {
                    return Ok(Value::from(""));
                };

                let tag = format!(
                    r#"<link rel="stylesheet" href="{}" type="text/css" />"#,
                    filename
                );
                Ok(Value::from(tag))
            },
        );

        // Add js_tag function
        env.add_function(
            "js_tag",
            |args: &[Value]| -> Result<Value, MinijinjaError> {
                let js = args.get(0).ok_or_else(|| {
                    MinijinjaError::new(ErrorKind::InvalidOperation, "js_tag requires js argument")
                })?;

                let filename = if let Some(js_str) = js.as_str() {
                    js_str
                } else {
                    return Ok(Value::from(""));
                };

                let tag = format!(r#"<script src="{}"></script>"#, filename);
                Ok(Value::from(tag))
            },
        );

        // Add toctree function
        env.add_function(
            "toctree",
            |_args: &[Value]| -> Result<Value, MinijinjaError> {
                // TODO: Implement actual toctree generation
                Ok(Value::from("<div class=\"toctree-wrapper\"></div>"))
            },
        );

        // Add |e filter (HTML escape)
        env.add_filter("e", |value: Value| -> Result<Value, MinijinjaError> {
            if let Some(s) = value.as_str() {
                Ok(Value::from(html_escape::encode_text(s).to_string()))
            } else {
                Ok(value)
            }
        });

        // Add |striptags filter
        env.add_filter(
            "striptags",
            |value: Value| -> Result<Value, MinijinjaError> {
                if let Some(s) = value.as_str() {
                    // Simple HTML tag stripping
                    let stripped = regex::Regex::new(r"<[^>]*>").unwrap().replace_all(s, "");
                    Ok(Value::from(stripped.to_string()))
                } else {
                    Ok(value)
                }
            },
        );
    }

    /// Render a template with the given context
    pub fn render(
        &self,
        template_name: &str,
        context: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<String> {
        let template = self
            .env
            .get_template(template_name)
            .map_err(|e| anyhow::anyhow!("Template '{}' not found: {}", template_name, e))?;

        // Convert context to minijinja Values
        let mut full_context = self.global_context.clone();
        for (key, value) in context {
            full_context.insert(key.clone(), Self::json_to_value(value));
        }

        let rendered = template
            .render(&full_context)
            .map_err(|e| anyhow::anyhow!("Failed to render template '{}': {}", template_name, e))?;

        Ok(rendered)
    }

    /// Convert serde_json::Value to minijinja::Value
    fn json_to_value(json_value: &serde_json::Value) -> Value {
        match json_value {
            serde_json::Value::Null => Value::UNDEFINED,
            serde_json::Value::Bool(b) => Value::from(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::from(i)
                } else if let Some(f) = n.as_f64() {
                    Value::from(f)
                } else {
                    Value::UNDEFINED
                }
            }
            serde_json::Value::String(s) => Value::from(s.clone()),
            serde_json::Value::Array(arr) => {
                let values: Vec<Value> = arr.iter().map(Self::json_to_value).collect();
                Value::from(values)
            }
            serde_json::Value::Object(obj) => {
                // Convert to a simple map representation
                let map: HashMap<String, Value> = obj
                    .iter()
                    .map(|(k, v)| (k.clone(), Self::json_to_value(v)))
                    .collect();
                Value::from_serialize(&map)
            }
        }
    }

    /// Set global template context
    pub fn set_global_context(&mut self, context: HashMap<String, Value>) {
        self.global_context = context;
    }

    /// Update global template context
    pub fn update_global_context(&mut self, key: String, value: Value) {
        self.global_context.insert(key, value);
    }

    /// Get newest template modification time
    pub fn newest_template_mtime(&self) -> std::time::SystemTime {
        let mut newest = std::time::UNIX_EPOCH;

        for template_dir in &self.template_dirs {
            if let Ok(entries) = std::fs::read_dir(template_dir) {
                for entry in entries.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        if let Ok(mtime) = metadata.modified() {
                            if mtime > newest {
                                newest = mtime;
                            }
                        }
                    }
                }
            }
        }

        newest
    }

    /// Get newest template name (for logging)
    pub fn newest_template_name(&self) -> String {
        let mut newest_time = std::time::UNIX_EPOCH;
        let mut newest_name = String::new();

        for template_dir in &self.template_dirs {
            if let Ok(entries) = std::fs::read_dir(template_dir) {
                for entry in entries.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        if let Ok(mtime) = metadata.modified() {
                            if mtime > newest_time {
                                newest_time = mtime;
                                newest_name = entry.file_name().to_string_lossy().to_string();
                            }
                        }
                    }
                }
            }
        }

        newest_name
    }
}

/// Template context helper for building context maps
#[derive(Debug, Default)]
pub struct TemplateContext {
    context: serde_json::Map<String, serde_json::Value>,
}

impl TemplateContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert<T: Serialize>(&mut self, key: &str, value: T) -> Result<()> {
        let json_value = serde_json::to_value(value)?;
        self.context.insert(key.to_string(), json_value);
        Ok(())
    }

    pub fn extend(&mut self, other: serde_json::Map<String, serde_json::Value>) {
        self.context.extend(other);
    }

    pub fn build(self) -> serde_json::Map<String, serde_json::Value> {
        self.context
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BuildConfig;

    #[test]
    fn test_template_engine_creation() {
        let config = BuildConfig::default();
        let engine = TemplateEngine::new(&config);
        assert!(engine.is_ok());
    }

    #[test]
    fn test_template_context() {
        let mut ctx = TemplateContext::new();
        ctx.insert("title", "Test Title").unwrap();
        ctx.insert("count", 42).unwrap();

        let context = ctx.build();
        assert_eq!(
            context.get("title").and_then(|v| v.as_str()),
            Some("Test Title")
        );
        assert_eq!(context.get("count").and_then(|v| v.as_i64()), Some(42));
    }
}
