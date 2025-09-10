use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Represents a parsed Sphinx role
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    pub target: String,
    pub text: Option<String>,
    pub line_number: usize,
    pub source_file: String,
}

/// Role processor trait
pub trait RoleProcessor {
    fn process(&self, role: &Role) -> Result<String>;
    fn get_name(&self) -> &str;
}

/// Role registry for managing built-in and custom roles
pub struct RoleRegistry {
    processors: HashMap<String, Box<dyn RoleProcessor + Send + Sync>>,
}

impl RoleRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            processors: HashMap::new(),
        };

        // Register built-in roles
        registry.register_builtin_roles();
        registry
    }

    pub fn register(&mut self, processor: Box<dyn RoleProcessor + Send + Sync>) {
        self.processors
            .insert(processor.get_name().to_string(), processor);
    }

    pub fn get(&self, name: &str) -> Option<&Box<dyn RoleProcessor + Send + Sync>> {
        self.processors.get(name)
    }

    pub fn process_role(&self, role: &Role) -> Result<String> {
        if let Some(processor) = self.get(&role.name) {
            processor.process(role)
        } else {
            // Return a warning comment for unknown roles
            Ok(format!("<!-- Unknown role: {} -->", role.name))
        }
    }

    fn register_builtin_roles(&mut self) {
        // Cross-reference roles
        self.register(Box::new(RefRole));
        self.register(Box::new(DocRole));
        self.register(Box::new(DownloadRole));
        self.register(Box::new(NumRefRole));

        // Code roles
        self.register(Box::new(CodeRole));
        self.register(Box::new(FileRole));
        self.register(Box::new(ProgramRole));

        // Math roles
        self.register(Box::new(MathRole));

        // Generic emphasis roles
        self.register(Box::new(EmphasisRole::new("emphasis")));
        self.register(Box::new(EmphasisRole::new("strong")));
        self.register(Box::new(EmphasisRole::new("literal")));
    }
}

/// Parse a role from RST text
pub fn parse_role(text: &str, line_number: usize, source_file: &str) -> Result<Option<Role>> {
    // Match patterns like :role:`target` or :role:`text <target>`
    let role_regex = Regex::new(r":([a-zA-Z][a-zA-Z0-9_:-]*):(`[^`]+`)")?;

    if let Some(captures) = role_regex.captures(text) {
        let name = captures.get(1).unwrap().as_str().to_string();
        let content = captures.get(2).unwrap().as_str();

        // Remove backticks
        let content = content.trim_start_matches('`').trim_end_matches('`');

        // Check if it has custom text: "text <target>"
        let angle_bracket_regex = Regex::new(r"^(.+?)\s*<(.+?)>$")?;

        let (text, target) = if let Some(inner_captures) = angle_bracket_regex.captures(content) {
            let text = inner_captures.get(1).unwrap().as_str().trim().to_string();
            let target = inner_captures.get(2).unwrap().as_str().trim().to_string();
            (Some(text), target)
        } else {
            (None, content.to_string())
        };

        Ok(Some(Role {
            name,
            target,
            text,
            line_number,
            source_file: source_file.to_string(),
        }))
    } else {
        Ok(None)
    }
}

// Cross-reference roles
struct RefRole;

impl RoleProcessor for RefRole {
    fn process(&self, role: &Role) -> Result<String> {
        let display_text = role.text.as_ref().unwrap_or(&role.target);
        Ok(format!(
            "<a class=\"reference internal\" href=\"#{}\">{}</a>",
            role.target, display_text
        ))
    }

    fn get_name(&self) -> &str {
        "ref"
    }
}

struct DocRole;

impl RoleProcessor for DocRole {
    fn process(&self, role: &Role) -> Result<String> {
        let display_text = role.text.as_ref().unwrap_or(&role.target);
        Ok(format!(
            "<a class=\"reference internal\" href=\"{}.html\">{}</a>",
            role.target, display_text
        ))
    }

    fn get_name(&self) -> &str {
        "doc"
    }
}

struct DownloadRole;

impl RoleProcessor for DownloadRole {
    fn process(&self, role: &Role) -> Result<String> {
        let display_text = role.text.as_ref().unwrap_or(&role.target);
        Ok(format!(
            "<a class=\"reference download internal\" href=\"{}\" download>{}</a>",
            role.target, display_text
        ))
    }

    fn get_name(&self) -> &str {
        "download"
    }
}

struct NumRefRole;

impl RoleProcessor for NumRefRole {
    fn process(&self, role: &Role) -> Result<String> {
        let display_text = role.text.as_ref().unwrap_or(&role.target);
        Ok(format!(
            "<a class=\"reference internal\" href=\"#{}\">{}</a>",
            role.target, display_text
        ))
    }

    fn get_name(&self) -> &str {
        "numref"
    }
}

// Code roles
struct CodeRole;

impl RoleProcessor for CodeRole {
    fn process(&self, role: &Role) -> Result<String> {
        let display_text = role.text.as_ref().unwrap_or(&role.target);
        Ok(format!(
            "<code class=\"docutils literal notranslate\">{}</code>",
            html_escape::encode_text(display_text)
        ))
    }

    fn get_name(&self) -> &str {
        "code"
    }
}

struct FileRole;

impl RoleProcessor for FileRole {
    fn process(&self, role: &Role) -> Result<String> {
        let display_text = role.text.as_ref().unwrap_or(&role.target);
        Ok(format!(
            "<code class=\"file docutils literal notranslate\">{}</code>",
            html_escape::encode_text(display_text)
        ))
    }

    fn get_name(&self) -> &str {
        "file"
    }
}

struct ProgramRole;

impl RoleProcessor for ProgramRole {
    fn process(&self, role: &Role) -> Result<String> {
        let display_text = role.text.as_ref().unwrap_or(&role.target);
        Ok(format!(
            "<strong class=\"program\">{}</strong>",
            html_escape::encode_text(display_text)
        ))
    }

    fn get_name(&self) -> &str {
        "program"
    }
}

// Math roles
struct MathRole;

impl RoleProcessor for MathRole {
    fn process(&self, role: &Role) -> Result<String> {
        let display_text = role.text.as_ref().unwrap_or(&role.target);
        Ok(format!(
            "<span class=\"math notranslate nohighlight\">\\({}\\)</span>",
            html_escape::encode_text(display_text)
        ))
    }

    fn get_name(&self) -> &str {
        "math"
    }
}

// Generic emphasis roles
struct EmphasisRole {
    name: String,
}

impl EmphasisRole {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

impl RoleProcessor for EmphasisRole {
    fn process(&self, role: &Role) -> Result<String> {
        let display_text = role.text.as_ref().unwrap_or(&role.target);

        match self.name.as_str() {
            "emphasis" => Ok(format!(
                "<em>{}</em>",
                html_escape::encode_text(display_text)
            )),
            "strong" => Ok(format!(
                "<strong>{}</strong>",
                html_escape::encode_text(display_text)
            )),
            "literal" => Ok(format!(
                "<code class=\"docutils literal notranslate\">{}</code>",
                html_escape::encode_text(display_text)
            )),
            _ => Ok(format!(
                "<span class=\"{}\">{}</span>",
                self.name,
                html_escape::encode_text(display_text)
            )),
        }
    }

    fn get_name(&self) -> &str {
        &self.name
    }
}
