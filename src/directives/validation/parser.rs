//! Parser for extracting directives and roles from RST content

use super::{ParsedDirective, ParsedRole, SourceLocation};
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashMap;

lazy_static! {
    /// Regex for matching directive patterns
    static ref DIRECTIVE_REGEX: Regex = Regex::new(
        r"(?m)^\.\. ([a-zA-Z][a-zA-Z0-9_-]*)::(.*?)$"
    ).unwrap();

    /// Regex for matching directive options
    static ref OPTION_REGEX: Regex = Regex::new(
        r"(?m)^\s+:([a-zA-Z][a-zA-Z0-9_-]*): ?(.*?)$"
    ).unwrap();

    /// Regex for matching role patterns
    static ref ROLE_REGEX: Regex = Regex::new(
        r":([a-zA-Z][a-zA-Z0-9_-]*):(`[^`]+`|[^\s]+)"
    ).unwrap();

    /// Regex for parsing role with display text
    static ref ROLE_WITH_TEXT_REGEX: Regex = Regex::new(
        r"`([^<]+)<([^>]+)>`"
    ).unwrap();
}

/// Parser for extracting directives and roles from RST content
pub struct DirectiveRoleParser {
    /// Source file being parsed
    source_file: String,
}

impl DirectiveRoleParser {
    /// Creates a new parser for the given source file
    pub fn new(source_file: String) -> Self {
        Self { source_file }
    }

    /// Extracts all directives from the given content
    pub fn extract_directives(&self, content: &str) -> Vec<ParsedDirective> {
        let mut directives = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        for (line_num, line) in lines.iter().enumerate() {
            if let Some(captures) = DIRECTIVE_REGEX.captures(line) {
                let directive_name = captures.get(1).unwrap().as_str().to_string();
                let args_str = captures.get(2).unwrap().as_str().trim();

                // Parse arguments
                let arguments: Vec<String> = if args_str.is_empty() {
                    Vec::new()
                } else {
                    args_str.split_whitespace().map(|s| s.to_string()).collect()
                };

                // Look for options and content in following lines
                let (options, content, _content_end_line) =
                    self.parse_directive_body(&lines, line_num + 1);

                let directive = ParsedDirective {
                    name: directive_name,
                    arguments,
                    options,
                    content,
                    location: SourceLocation {
                        file: self.source_file.clone(),
                        line: line_num + 1,
                        column: line.find("..").unwrap_or(0) + 1,
                    },
                };

                directives.push(directive);
            }
        }

        directives
    }

    /// Extracts all roles from the given content
    pub fn extract_roles(&self, content: &str) -> Vec<ParsedRole> {
        let mut roles = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        for (line_num, line) in lines.iter().enumerate() {
            for captures in ROLE_REGEX.captures_iter(line) {
                let role_name = captures.get(1).unwrap().as_str().to_string();
                let role_content = captures.get(2).unwrap().as_str();

                // Remove backticks if present
                let role_content = if role_content.starts_with('`') && role_content.ends_with('`') {
                    &role_content[1..role_content.len() - 1]
                } else {
                    role_content
                };

                // Check for display text format: `Display Text <target>`
                let (target, display_text) = if role_content.contains('<')
                    && role_content.contains('>')
                {
                    // Try to parse "Display Text <target>" format (without expecting backticks)
                    if let Some(angle_start) = role_content.rfind('<') {
                        if let Some(angle_end) = role_content.rfind('>') {
                            if angle_start < angle_end {
                                let display = role_content[..angle_start].trim().to_string();
                                let target = role_content[angle_start + 1..angle_end].to_string();
                                (
                                    target,
                                    if display.is_empty() {
                                        None
                                    } else {
                                        Some(display)
                                    },
                                )
                            } else {
                                (role_content.to_string(), None)
                            }
                        } else {
                            (role_content.to_string(), None)
                        }
                    } else {
                        (role_content.to_string(), None)
                    }
                } else {
                    (role_content.to_string(), None)
                };

                let role = ParsedRole {
                    name: role_name,
                    target,
                    display_text,
                    location: SourceLocation {
                        file: self.source_file.clone(),
                        line: line_num + 1,
                        column: line.find(':').unwrap_or(0) + 1,
                    },
                };

                roles.push(role);
            }
        }

        roles
    }

    /// Parses directive body (options and content)
    fn parse_directive_body(
        &self,
        lines: &[&str],
        start_line: usize,
    ) -> (HashMap<String, String>, String, usize) {
        let mut options = HashMap::new();
        let mut content_lines = Vec::new();
        let mut current_line = start_line;
        let mut in_content = false;

        while current_line < lines.len() {
            let line = lines[current_line];

            // Empty line
            if line.trim().is_empty() {
                if in_content {
                    content_lines.push(String::new());
                }
                current_line += 1;
                continue;
            }

            // Check for option
            if let Some(option_captures) = OPTION_REGEX.captures(line) {
                if !in_content {
                    let option_name = option_captures.get(1).unwrap().as_str().to_string();
                    let option_value = option_captures.get(2).unwrap().as_str().to_string();
                    options.insert(option_name, option_value);
                    current_line += 1;
                    continue;
                }
            }

            // Check if line is indented (content)
            if line.starts_with("   ") || line.starts_with('\t') {
                in_content = true;
                // Remove common indentation
                let content_line = if let Some(stripped) = line.strip_prefix("   ") {
                    stripped
                } else if let Some(stripped) = line.strip_prefix('\t') {
                    stripped
                } else {
                    line
                };
                content_lines.push(content_line.to_string());
                current_line += 1;
                continue;
            }

            // Non-indented line after we've seen content means end of directive
            if in_content {
                break;
            }

            // If we haven't seen options or content, this might be the start of content
            if !line.starts_with(':') {
                break;
            }

            current_line += 1;
        }

        let content = content_lines.join("\n");
        (options, content, current_line)
    }

    /// Extracts both directives and roles from content
    pub fn parse_content(&self, content: &str) -> (Vec<ParsedDirective>, Vec<ParsedRole>) {
        let directives = self.extract_directives(content);
        let roles = self.extract_roles(content);
        (directives, roles)
    }

    /// Validates that a line contains a properly formatted directive
    pub fn is_directive_line(line: &str) -> bool {
        DIRECTIVE_REGEX.is_match(line)
    }

    /// Validates that text contains a role
    pub fn contains_role(text: &str) -> bool {
        ROLE_REGEX.is_match(text)
    }

    /// Counts the number of directives in content
    pub fn count_directives(content: &str) -> usize {
        DIRECTIVE_REGEX.find_iter(content).count()
    }

    /// Counts the number of roles in content
    pub fn count_roles(content: &str) -> usize {
        ROLE_REGEX.find_iter(content).count()
    }
}

/// Statistics about parsed content
#[derive(Debug, Default, Clone)]
pub struct ParseStatistics {
    /// Number of directives found
    pub directive_count: usize,
    /// Number of roles found
    pub role_count: usize,
    /// Breakdown by directive type
    pub directives_by_type: HashMap<String, usize>,
    /// Breakdown by role type
    pub roles_by_type: HashMap<String, usize>,
    /// Lines processed
    pub lines_processed: usize,
}

impl ParseStatistics {
    /// Creates new parse statistics
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a directive
    pub fn record_directive(&mut self, directive: &ParsedDirective) {
        self.directive_count += 1;
        *self
            .directives_by_type
            .entry(directive.name.clone())
            .or_insert(0) += 1;
    }

    /// Records a role
    pub fn record_role(&mut self, role: &ParsedRole) {
        self.role_count += 1;
        *self.roles_by_type.entry(role.name.clone()).or_insert(0) += 1;
    }

    /// Records lines processed
    pub fn set_lines_processed(&mut self, lines: usize) {
        self.lines_processed = lines;
    }

    /// Returns total items parsed
    pub fn total_items(&self) -> usize {
        self.directive_count + self.role_count
    }
}

/// Enhanced parser with statistics tracking
pub struct StatisticalDirectiveRoleParser {
    parser: DirectiveRoleParser,
    statistics: ParseStatistics,
}

impl StatisticalDirectiveRoleParser {
    /// Creates a new statistical parser
    pub fn new(source_file: String) -> Self {
        Self {
            parser: DirectiveRoleParser::new(source_file),
            statistics: ParseStatistics::new(),
        }
    }

    /// Parses content and updates statistics
    pub fn parse_with_statistics(
        &mut self,
        content: &str,
    ) -> (Vec<ParsedDirective>, Vec<ParsedRole>) {
        let (directives, roles) = self.parser.parse_content(content);

        // Update statistics
        self.statistics.set_lines_processed(content.lines().count());

        for directive in &directives {
            self.statistics.record_directive(directive);
        }

        for role in &roles {
            self.statistics.record_role(role);
        }

        (directives, roles)
    }

    /// Returns current statistics
    pub fn statistics(&self) -> &ParseStatistics {
        &self.statistics
    }

    /// Resets statistics
    pub fn reset_statistics(&mut self) {
        self.statistics = ParseStatistics::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_directive_parsing() {
        let parser = DirectiveRoleParser::new("test.rst".to_string());

        let content = r#"
.. note:: This is a note

   This is the content of the note.
   It can span multiple lines.

.. code-block:: python
   :linenos:
   :caption: Example code

   def hello():
       print("Hello, world!")
"#;

        let directives = parser.extract_directives(content);
        assert_eq!(directives.len(), 2);

        // Check note directive
        assert_eq!(directives[0].name, "note");
        assert_eq!(directives[0].arguments.len(), 4); // "This", "is", "a", "note"
        assert_eq!(directives[0].arguments[0], "This");
        assert_eq!(directives[0].arguments[1], "is");
        assert_eq!(directives[0].arguments[2], "a");
        assert_eq!(directives[0].arguments[3], "note");
        assert!(directives[0].content.contains("content of the note"));

        // Check code-block directive
        assert_eq!(directives[1].name, "code-block");
        assert_eq!(directives[1].arguments.len(), 1);
        assert_eq!(directives[1].arguments[0], "python");
        assert_eq!(directives[1].options.len(), 2);
        assert!(directives[1].options.contains_key("linenos"));
        assert_eq!(
            directives[1].options.get("caption"),
            Some(&"Example code".to_string())
        );
        assert!(directives[1].content.contains("def hello()"));
    }

    #[test]
    fn test_role_parsing() {
        let parser = DirectiveRoleParser::new("test.rst".to_string());

        let content = r#"
See :doc:`installation` for setup instructions.
Use :ref:`advanced-config` for configuration.
Download the :download:`example.pdf` file.
For math, use :math:`x = \frac{a}{b}`.
See :doc:`Custom Title <installation>` for details.
"#;

        let roles = parser.extract_roles(content);
        assert_eq!(roles.len(), 5);

        // Check doc role
        assert_eq!(roles[0].name, "doc");
        assert_eq!(roles[0].target, "installation");
        assert_eq!(roles[0].display_text, None);

        // Check ref role
        assert_eq!(roles[1].name, "ref");
        assert_eq!(roles[1].target, "advanced-config");

        // Check download role
        assert_eq!(roles[2].name, "download");
        assert_eq!(roles[2].target, "example.pdf");

        // Check math role
        assert_eq!(roles[3].name, "math");
        assert_eq!(roles[3].target, r"x = \frac{a}{b}");

        // Check doc role with display text
        assert_eq!(roles[4].name, "doc");
        assert_eq!(roles[4].target, "installation");
        assert_eq!(roles[4].display_text, Some("Custom Title".to_string()));
    }

    #[test]
    fn test_statistical_parser() {
        let mut parser = StatisticalDirectiveRoleParser::new("test.rst".to_string());

        let content = r#"
.. note:: Test note

   Content here.

See :doc:`test` and :ref:`section`.
"#;

        let (directives, roles) = parser.parse_with_statistics(content);

        assert_eq!(directives.len(), 1);
        assert_eq!(roles.len(), 2);

        let stats = parser.statistics();
        assert_eq!(stats.directive_count, 1);
        assert_eq!(stats.role_count, 2);
        assert_eq!(stats.total_items(), 3);
        assert_eq!(stats.directives_by_type.get("note"), Some(&1));
        assert_eq!(stats.roles_by_type.get("doc"), Some(&1));
        assert_eq!(stats.roles_by_type.get("ref"), Some(&1));
    }

    #[test]
    fn test_utility_functions() {
        assert!(DirectiveRoleParser::is_directive_line(".. note:: Test"));
        assert!(!DirectiveRoleParser::is_directive_line(
            "This is not a directive"
        ));

        assert!(DirectiveRoleParser::contains_role("See :doc:`test` here"));
        assert!(!DirectiveRoleParser::contains_role("No roles here"));

        let content = ".. note:: Test\n.. warning:: Another\nSee :doc:`test` and :ref:`section`.";
        assert_eq!(DirectiveRoleParser::count_directives(content), 2);
        assert_eq!(DirectiveRoleParser::count_roles(content), 2);
    }

    #[test]
    fn test_directive_options_parsing() {
        let parser = DirectiveRoleParser::new("test.rst".to_string());

        let content = r#"
.. figure:: image.png
   :width: 100px
   :alt: Test image
   :align: center

   This is the caption.
"#;

        let directives = parser.extract_directives(content);
        assert_eq!(directives.len(), 1);

        let directive = &directives[0];
        assert_eq!(directive.name, "figure");
        assert_eq!(directive.arguments.len(), 1);
        assert_eq!(directive.arguments[0], "image.png");
        assert_eq!(directive.options.len(), 3);
        assert_eq!(directive.options.get("width"), Some(&"100px".to_string()));
        assert_eq!(
            directive.options.get("alt"),
            Some(&"Test image".to_string())
        );
        assert_eq!(directive.options.get("align"), Some(&"center".to_string()));
        assert_eq!(directive.content.trim(), "This is the caption.");
    }
}
