use anyhow::{anyhow, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Represents a parsed Sphinx directive
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Directive {
    pub name: String,
    pub arguments: Vec<String>,
    pub options: HashMap<String, String>,
    pub content: Vec<String>,
    pub line_number: usize,
    pub source_file: String,
}

/// Directive processor trait
pub trait DirectiveProcessor {
    fn process(&self, directive: &Directive) -> Result<String>;
    fn get_name(&self) -> &str;
    fn get_option_spec(&self) -> HashMap<String, DirectiveOptionType>;
}

/// Directive option types
#[derive(Debug, Clone)]
pub enum DirectiveOptionType {
    Flag,
    String,
    Integer,
    Float,
    Choice(Vec<String>),
    Unchanged,
    UnchangedRequired,
    Path,
    Percentage,
    LengthOrPercentage,
    Class,
    ClassOption,
    Encoding,
}

/// Built-in directive processors
pub struct DirectiveRegistry {
    processors: HashMap<String, Box<dyn DirectiveProcessor + Send + Sync>>,
}

impl Default for DirectiveRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DirectiveRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            processors: HashMap::new(),
        };

        // Register built-in directives
        registry.register_builtin_directives();
        registry
    }

    pub fn register(&mut self, processor: Box<dyn DirectiveProcessor + Send + Sync>) {
        self.processors
            .insert(processor.get_name().to_string(), processor);
    }

    pub fn get(&self, name: &str) -> Option<&(dyn DirectiveProcessor + Send + Sync)> {
        self.processors.get(name).map(|boxed| boxed.as_ref())
    }

    pub fn process_directive(&self, directive: &Directive) -> Result<String> {
        if let Some(processor) = self.get(&directive.name) {
            processor.process(directive)
        } else {
            // Return a warning comment for unknown directives
            Ok(format!("<!-- Unknown directive: {} -->", directive.name))
        }
    }

    fn register_builtin_directives(&mut self) {
        // Admonition directives
        self.register(Box::new(AdmonitionDirective::new("note")));
        self.register(Box::new(AdmonitionDirective::new("warning")));
        self.register(Box::new(AdmonitionDirective::new("important")));
        self.register(Box::new(AdmonitionDirective::new("tip")));
        self.register(Box::new(AdmonitionDirective::new("caution")));
        self.register(Box::new(AdmonitionDirective::new("danger")));
        self.register(Box::new(AdmonitionDirective::new("error")));
        self.register(Box::new(AdmonitionDirective::new("hint")));
        self.register(Box::new(AdmonitionDirective::new("attention")));
        self.register(Box::new(AdmonitionDirective::new("seealso")));
        self.register(Box::new(GenericAdmonitionDirective));

        // Code directives
        self.register(Box::new(CodeBlockDirective));
        self.register(Box::new(LiteralIncludeDirective));
        self.register(Box::new(HighlightDirective));

        // Structure directives
        self.register(Box::new(ToctreeDirective));
        self.register(Box::new(IndexDirective));
        self.register(Box::new(OnlyDirective));
        self.register(Box::new(IfConfigDirective));

        // Image directives
        self.register(Box::new(ImageDirective));
        self.register(Box::new(FigureDirective));

        // Table directives
        self.register(Box::new(TableDirective));
        self.register(Box::new(CsvTableDirective));
        self.register(Box::new(ListTableDirective));

        // Include directives
        self.register(Box::new(IncludeDirective));
        self.register(Box::new(RawDirective));

        // Math directives
        self.register(Box::new(MathDirective));

        // Domain-specific directives
        self.register(Box::new(AutoDocDirective));
        self.register(Box::new(AutoModuleDirective));
        self.register(Box::new(AutoClassDirective));
        self.register(Box::new(AutoFunctionDirective));

        // Meta directives
        self.register(Box::new(MetaDirective));
        self.register(Box::new(SidebarDirective));
        self.register(Box::new(TopicDirective));
        self.register(Box::new(RubricDirective));
        self.register(Box::new(EpigraphDirective));
        self.register(Box::new(HighlightsDirective));
        self.register(Box::new(PullQuoteDirective));
        self.register(Box::new(CompoundDirective));
        self.register(Box::new(ContainerDirective));

        // Version directives
        self.register(Box::new(VersionAddedDirective));
        self.register(Box::new(VersionChangedDirective));
        self.register(Box::new(DeprecatedDirective));
    }
}

/// Parse a directive from RST text
pub fn parse_directive(
    text: &str,
    line_number: usize,
    source_file: &str,
) -> Result<Option<Directive>> {
    let directive_regex = Regex::new(r"^\.\. ([a-zA-Z][a-zA-Z0-9_-]*)::\s*(.*?)$")?;

    if let Some(captures) = directive_regex.captures(text) {
        let name = captures.get(1).unwrap().as_str().to_string();
        let args_str = captures.get(2).unwrap().as_str();

        // Parse arguments (simple space-separated for now)
        let arguments: Vec<String> = if args_str.is_empty() {
            Vec::new()
        } else {
            args_str.split_whitespace().map(|s| s.to_string()).collect()
        };

        Ok(Some(Directive {
            name,
            arguments,
            options: HashMap::new(),
            content: Vec::new(),
            line_number,
            source_file: source_file.to_string(),
        }))
    } else {
        Ok(None)
    }
}

// Admonition Directive
struct AdmonitionDirective {
    name: String,
}

impl AdmonitionDirective {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

impl DirectiveProcessor for AdmonitionDirective {
    fn process(&self, directive: &Directive) -> Result<String> {
        let class = if self.name == "seealso" {
            "seealso"
        } else {
            &self.name
        };
        let title = if directive.arguments.is_empty() {
            match self.name.as_str() {
                "note" => "Note",
                "warning" => "Warning",
                "important" => "Important",
                "tip" => "Tip",
                "caution" => "Caution",
                "danger" => "Danger",
                "error" => "Error",
                "hint" => "Hint",
                "attention" => "Attention",
                "seealso" => "See also",
                _ => &self.name,
            }
        } else {
            &directive.arguments[0]
        };

        let content = directive.content.join("\n");

        Ok(format!(
            "<div class=\"admonition {}\"><p class=\"admonition-title\">{}</p>{}</div>",
            class, title, content
        ))
    }

    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_option_spec(&self) -> HashMap<String, DirectiveOptionType> {
        let mut options = HashMap::new();
        options.insert("class".to_string(), DirectiveOptionType::ClassOption);
        options.insert("name".to_string(), DirectiveOptionType::String);
        options
    }
}

// Generic Admonition Directive
struct GenericAdmonitionDirective;

impl DirectiveProcessor for GenericAdmonitionDirective {
    fn process(&self, directive: &Directive) -> Result<String> {
        let default_title = "Admonition".to_string();
        let title = directive.arguments.first().unwrap_or(&default_title);
        let content = directive.content.join("\n");

        Ok(format!(
            "<div class=\"admonition admonition-generic\"><p class=\"admonition-title\">{}</p>{}</div>",
            title, content
        ))
    }

    fn get_name(&self) -> &str {
        "admonition"
    }

    fn get_option_spec(&self) -> HashMap<String, DirectiveOptionType> {
        let mut options = HashMap::new();
        options.insert("class".to_string(), DirectiveOptionType::ClassOption);
        options.insert("name".to_string(), DirectiveOptionType::String);
        options
    }
}

// Code Block Directive
struct CodeBlockDirective;

impl DirectiveProcessor for CodeBlockDirective {
    fn process(&self, directive: &Directive) -> Result<String> {
        let default_language = "text".to_string();
        let language = directive.arguments.first().unwrap_or(&default_language);
        let _linenos = directive.options.contains_key("linenos");
        let _emphasize_lines = directive.options.get("emphasize-lines");
        let caption = directive.options.get("caption");
        let _name = directive.options.get("name");

        let content = directive.content.join("\n");

        let mut html = String::new();

        if let Some(caption_text) = caption {
            html.push_str(&format!(
                "<div class=\"code-block-caption\">{}</div>",
                caption_text
            ));
        }

        html.push_str(&format!(
            "<div class=\"highlight-{}\"><pre><code class=\"language-{}\">{}</code></pre></div>",
            language,
            language,
            html_escape::encode_text(&content)
        ));

        Ok(html)
    }

    fn get_name(&self) -> &str {
        "code-block"
    }

    fn get_option_spec(&self) -> HashMap<String, DirectiveOptionType> {
        let mut options = HashMap::new();
        options.insert("linenos".to_string(), DirectiveOptionType::Flag);
        options.insert("lineno-start".to_string(), DirectiveOptionType::Integer);
        options.insert("emphasize-lines".to_string(), DirectiveOptionType::String);
        options.insert("caption".to_string(), DirectiveOptionType::String);
        options.insert("name".to_string(), DirectiveOptionType::String);
        options.insert("dedent".to_string(), DirectiveOptionType::Integer);
        options.insert("force".to_string(), DirectiveOptionType::Flag);
        options
    }
}

// Literal Include Directive
struct LiteralIncludeDirective;

impl DirectiveProcessor for LiteralIncludeDirective {
    fn process(&self, directive: &Directive) -> Result<String> {
        let filename = directive
            .arguments
            .first()
            .ok_or_else(|| anyhow!("literalinclude directive requires a filename"))?;

        let language = directive
            .options
            .get("language")
            .cloned()
            .or_else(|| {
                std::path::Path::new(filename)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| {
                        match ext {
                            "py" => "python",
                            "rs" => "rust",
                            "js" => "javascript",
                            "ts" => "typescript",
                            "cpp" | "cc" | "cxx" => "cpp",
                            "c" => "c",
                            "h" | "hpp" => "cpp",
                            "java" => "java",
                            "go" => "go",
                            "php" => "php",
                            "rb" => "ruby",
                            "sh" | "bash" => "bash",
                            "ps1" => "powershell",
                            "sql" => "sql",
                            "xml" => "xml",
                            "html" => "html",
                            "css" => "css",
                            "json" => "json",
                            "yaml" | "yml" => "yaml",
                            "toml" => "toml",
                            "ini" => "ini",
                            "md" => "markdown",
                            "rst" => "rst",
                            "tex" => "latex",
                            _ => "text",
                        }
                        .to_string()
                    })
            })
            .unwrap_or_else(|| "text".to_string());

        // For now, return a placeholder. In a full implementation,
        // you would read the file and include its contents
        Ok(format!(
            "<div class=\"literal-include\"><div class=\"highlight-{}\"><pre><code class=\"language-{}\"><!-- Content of {} would be included here --></code></pre></div></div>",
            language, language, filename
        ))
    }

    fn get_name(&self) -> &str {
        "literalinclude"
    }

    fn get_option_spec(&self) -> HashMap<String, DirectiveOptionType> {
        let mut options = HashMap::new();
        options.insert("language".to_string(), DirectiveOptionType::String);
        options.insert("linenos".to_string(), DirectiveOptionType::Flag);
        options.insert("lineno-start".to_string(), DirectiveOptionType::Integer);
        options.insert("emphasize-lines".to_string(), DirectiveOptionType::String);
        options.insert("lines".to_string(), DirectiveOptionType::String);
        options.insert("start-line".to_string(), DirectiveOptionType::Integer);
        options.insert("end-line".to_string(), DirectiveOptionType::Integer);
        options.insert("start-after".to_string(), DirectiveOptionType::String);
        options.insert("end-before".to_string(), DirectiveOptionType::String);
        options.insert("prepend".to_string(), DirectiveOptionType::String);
        options.insert("append".to_string(), DirectiveOptionType::String);
        options.insert("dedent".to_string(), DirectiveOptionType::Integer);
        options.insert("tab-width".to_string(), DirectiveOptionType::Integer);
        options.insert("encoding".to_string(), DirectiveOptionType::Encoding);
        options.insert("pyobject".to_string(), DirectiveOptionType::String);
        options.insert("caption".to_string(), DirectiveOptionType::String);
        options.insert("name".to_string(), DirectiveOptionType::String);
        options.insert("class".to_string(), DirectiveOptionType::ClassOption);
        options.insert("diff".to_string(), DirectiveOptionType::String);
        options
    }
}

// Highlight Directive
struct HighlightDirective;

impl DirectiveProcessor for HighlightDirective {
    fn process(&self, directive: &Directive) -> Result<String> {
        let default_language = "text".to_string();
        let language = directive.arguments.first().unwrap_or(&default_language);
        // This directive sets the highlighting language for subsequent code blocks
        Ok(format!("<!-- highlight language set to {} -->", language))
    }

    fn get_name(&self) -> &str {
        "highlight"
    }

    fn get_option_spec(&self) -> HashMap<String, DirectiveOptionType> {
        let mut options = HashMap::new();
        options.insert("linenothreshold".to_string(), DirectiveOptionType::Integer);
        options.insert("force".to_string(), DirectiveOptionType::Flag);
        options
    }
}

// Additional directive implementations would go here...
// For brevity, I'll provide stub implementations for the remaining directives

macro_rules! stub_directive {
    ($name:ident, $directive_name:expr) => {
        struct $name;

        impl DirectiveProcessor for $name {
            fn process(&self, directive: &Directive) -> Result<String> {
                Ok(format!(
                    "<!-- {} directive: {} -->",
                    $directive_name,
                    directive.arguments.join(" ")
                ))
            }

            fn get_name(&self) -> &str {
                $directive_name
            }

            fn get_option_spec(&self) -> HashMap<String, DirectiveOptionType> {
                HashMap::new()
            }
        }
    };
}

stub_directive!(ToctreeDirective, "toctree");
stub_directive!(IndexDirective, "index");
stub_directive!(OnlyDirective, "only");
stub_directive!(IfConfigDirective, "ifconfig");
stub_directive!(ImageDirective, "image");
stub_directive!(FigureDirective, "figure");
stub_directive!(TableDirective, "table");
stub_directive!(CsvTableDirective, "csv-table");
stub_directive!(ListTableDirective, "list-table");
stub_directive!(IncludeDirective, "include");
stub_directive!(RawDirective, "raw");
stub_directive!(MathDirective, "math");
stub_directive!(AutoDocDirective, "autodoc");
stub_directive!(AutoModuleDirective, "automodule");
stub_directive!(AutoClassDirective, "autoclass");
stub_directive!(AutoFunctionDirective, "autofunction");
stub_directive!(MetaDirective, "meta");
stub_directive!(SidebarDirective, "sidebar");
stub_directive!(TopicDirective, "topic");
stub_directive!(RubricDirective, "rubric");
stub_directive!(EpigraphDirective, "epigraph");
stub_directive!(HighlightsDirective, "highlights");
stub_directive!(PullQuoteDirective, "pull-quote");
stub_directive!(CompoundDirective, "compound");
stub_directive!(ContainerDirective, "container");
stub_directive!(VersionAddedDirective, "versionadded");
stub_directive!(VersionChangedDirective, "versionchanged");
stub_directive!(DeprecatedDirective, "deprecated");
