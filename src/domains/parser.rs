use crate::domains::{CrossReference, ReferenceLocation, ReferenceType};
use lazy_static::lazy_static;
/// Reference Parser for extracting cross-references from RST content
///
/// This module provides functionality to parse RST content and extract
/// cross-references like :doc:, :ref:, :func:, :class:, etc.
use regex::Regex;
use std::collections::HashMap;

lazy_static! {
    /// Regex for matching Sphinx cross-references
    /// Matches patterns like :ref:`target`, :doc:`target`, :func:`module.function`
    static ref CROSS_REF_REGEX: Regex = Regex::new(
        r":([a-zA-Z][a-zA-Z0-9_-]*):(`[^`]+`|[^\s]+)"
    ).unwrap();

    /// Regex for extracting target and display text from backtick format
    /// Matches `target <display>` or just `target`
    static ref TARGET_REGEX: Regex = Regex::new(
        r"`([^<>]+?)(?:\s*<([^<>]+?)>)?`"
    ).unwrap();
}

/// Parser for extracting cross-references from RST content
pub struct ReferenceParser {
    /// Map of role names to reference types
    role_mapping: HashMap<String, ReferenceType>,
}

impl Default for ReferenceParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ReferenceParser {
    /// Create a new reference parser
    pub fn new() -> Self {
        let mut role_mapping = HashMap::new();

        // Standard RST roles
        role_mapping.insert("doc".to_string(), ReferenceType::Document);
        role_mapping.insert("ref".to_string(), ReferenceType::Section);

        // Python domain roles
        role_mapping.insert("func".to_string(), ReferenceType::Function);
        role_mapping.insert("class".to_string(), ReferenceType::Class);
        role_mapping.insert("mod".to_string(), ReferenceType::Module);
        role_mapping.insert("meth".to_string(), ReferenceType::Method);
        role_mapping.insert("attr".to_string(), ReferenceType::Attribute);
        role_mapping.insert("data".to_string(), ReferenceType::Data);
        role_mapping.insert("exc".to_string(), ReferenceType::Exception);

        // Other common roles
        role_mapping.insert(
            "numref".to_string(),
            ReferenceType::Custom("numref".to_string()),
        );
        role_mapping.insert(
            "envvar".to_string(),
            ReferenceType::Custom("envvar".to_string()),
        );
        role_mapping.insert(
            "option".to_string(),
            ReferenceType::Custom("option".to_string()),
        );

        Self { role_mapping }
    }

    /// Register a custom role mapping
    pub fn register_role(&mut self, role: String, ref_type: ReferenceType) {
        self.role_mapping.insert(role, ref_type);
    }

    /// Parse content and extract all cross-references
    pub fn parse_content(
        &self,
        content: &str,
        docname: &str,
        source_path: Option<String>,
    ) -> Vec<CrossReference> {
        let mut references = Vec::new();

        for (line_num, line) in content.lines().enumerate() {
            let line_refs = self.parse_line(line, docname, line_num + 1, source_path.clone());
            references.extend(line_refs);
        }

        references
    }

    /// Parse a single line and extract cross-references
    pub fn parse_line(
        &self,
        line: &str,
        docname: &str,
        line_num: usize,
        source_path: Option<String>,
    ) -> Vec<CrossReference> {
        let mut references = Vec::new();

        for cap in CROSS_REF_REGEX.captures_iter(line) {
            let role = cap.get(1).unwrap().as_str();
            let target_text = cap.get(2).unwrap().as_str();

            if let Some(cross_ref) = self.parse_reference(
                role,
                target_text,
                docname,
                line_num,
                cap.get(0).unwrap().start(),
                source_path.clone(),
            ) {
                references.push(cross_ref);
            }
        }

        references
    }

    /// Parse a single reference
    fn parse_reference(
        &self,
        role: &str,
        target_text: &str,
        docname: &str,
        line_num: usize,
        column: usize,
        source_path: Option<String>,
    ) -> Option<CrossReference> {
        let ref_type = self
            .role_mapping
            .get(role)
            .cloned()
            .unwrap_or_else(|| ReferenceType::Custom(role.to_string()));

        let (target, display_text) = self.extract_target_and_display(target_text);

        // Check if this might be an external reference
        let is_external = self.is_external_reference(&target, &ref_type);

        Some(CrossReference {
            ref_type,
            target,
            display_text,
            source_location: ReferenceLocation {
                docname: docname.to_string(),
                lineno: Some(line_num),
                column: Some(column),
                source_path,
            },
            is_external,
        })
    }

    /// Extract target and display text from target string
    fn extract_target_and_display(&self, target_text: &str) -> (String, Option<String>) {
        // Handle backtick format
        if target_text.starts_with('`') && target_text.ends_with('`') {
            if let Some(cap) = TARGET_REGEX.captures(target_text) {
                let target = cap.get(1).unwrap().as_str().trim().to_string();
                let display_text = cap.get(2).map(|m| m.as_str().trim().to_string());
                return (target, display_text);
            }
        }

        // Simple format without backticks
        (target_text.trim().to_string(), None)
    }

    /// Determine if a reference is external
    fn is_external_reference(&self, target: &str, ref_type: &ReferenceType) -> bool {
        match ref_type {
            ReferenceType::Document => {
                // External if it contains a protocol or starts with http
                target.starts_with("http://")
                    || target.starts_with("https://")
                    || target.starts_with("file://")
            }
            ReferenceType::Function | ReferenceType::Class | ReferenceType::Module => {
                // External if it starts with a known external library
                target.starts_with("builtins.")
                    || target.starts_with("typing.")
                    || target.starts_with("collections.")
                    || target.starts_with("pathlib.")
                    || target.starts_with("os.")
                    || target.starts_with("sys.")
                    || target.starts_with("json.")
                    || target.starts_with("re.")
                    || target.starts_with("datetime.")
                    || target.starts_with("urllib.")
                    || target.starts_with("http.")
            }
            _ => false,
        }
    }

    /// Get statistics about parsed references
    pub fn get_reference_stats(&self, references: &[CrossReference]) -> HashMap<String, usize> {
        let mut stats = HashMap::new();

        for reference in references {
            let key = match &reference.ref_type {
                ReferenceType::Custom(name) => name.clone(),
                _ => format!("{:?}", reference.ref_type),
            };
            *stats.entry(key).or_insert(0) += 1;
        }

        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reference_parser_creation() {
        let parser = ReferenceParser::new();
        assert!(parser.role_mapping.contains_key("doc"));
        assert!(parser.role_mapping.contains_key("func"));
        assert!(parser.role_mapping.contains_key("class"));
    }

    #[test]
    fn test_simple_reference_parsing() {
        let parser = ReferenceParser::new();
        let content = "See :doc:`installation` for details.";

        let refs = parser.parse_content(content, "index", None);
        assert_eq!(refs.len(), 1);

        let ref_obj = &refs[0];
        assert_eq!(ref_obj.ref_type, ReferenceType::Document);
        assert_eq!(ref_obj.target, "installation");
        assert_eq!(ref_obj.display_text, None);
        assert!(!ref_obj.is_external);
    }

    #[test]
    fn test_reference_with_display_text() {
        let parser = ReferenceParser::new();
        let content = "See :doc:`Installation Guide <installation>` for details.";

        let refs = parser.parse_content(content, "index", None);
        assert_eq!(refs.len(), 1);

        let ref_obj = &refs[0];
        assert_eq!(ref_obj.target, "Installation Guide");
        assert_eq!(ref_obj.display_text, Some("installation".to_string()));
    }

    #[test]
    fn test_python_function_reference() {
        let parser = ReferenceParser::new();
        let content = "Use :func:`mymodule.my_function` to process data.";

        let refs = parser.parse_content(content, "api", None);
        assert_eq!(refs.len(), 1);

        let ref_obj = &refs[0];
        assert_eq!(ref_obj.ref_type, ReferenceType::Function);
        assert_eq!(ref_obj.target, "mymodule.my_function");
        assert!(!ref_obj.is_external);
    }

    #[test]
    fn test_external_reference_detection() {
        let parser = ReferenceParser::new();

        // External Python reference
        let content1 = "Use :func:`os.path.join` for paths.";
        let refs1 = parser.parse_content(content1, "test", None);
        assert_eq!(refs1.len(), 1);
        assert!(refs1[0].is_external);

        // External document reference
        let content2 = "See :doc:`https://docs.python.org/3/` for more.";
        let refs2 = parser.parse_content(content2, "test", None);
        assert_eq!(refs2.len(), 1);
        assert!(refs2[0].is_external);
    }

    #[test]
    fn test_multiple_references_in_line() {
        let parser = ReferenceParser::new();
        let content = "Use :func:`func1` and :class:`MyClass` together.";

        let refs = parser.parse_content(content, "test", None);
        assert_eq!(refs.len(), 2);

        assert_eq!(refs[0].ref_type, ReferenceType::Function);
        assert_eq!(refs[0].target, "func1");

        assert_eq!(refs[1].ref_type, ReferenceType::Class);
        assert_eq!(refs[1].target, "MyClass");
    }

    #[test]
    fn test_section_reference() {
        let parser = ReferenceParser::new();
        let content = "See :ref:`installation-section` for setup instructions.";

        let refs = parser.parse_content(content, "guide", None);
        assert_eq!(refs.len(), 1);

        let ref_obj = &refs[0];
        assert_eq!(ref_obj.ref_type, ReferenceType::Section);
        assert_eq!(ref_obj.target, "installation-section");
    }

    #[test]
    fn test_custom_role() {
        let mut parser = ReferenceParser::new();
        parser.register_role(
            "myref".to_string(),
            ReferenceType::Custom("myref".to_string()),
        );

        let content = "See :myref:`custom-target` for details.";
        let refs = parser.parse_content(content, "test", None);
        assert_eq!(refs.len(), 1);

        let ref_obj = &refs[0];
        assert_eq!(ref_obj.ref_type, ReferenceType::Custom("myref".to_string()));
        assert_eq!(ref_obj.target, "custom-target");
    }

    #[test]
    fn test_multiline_content() {
        let parser = ReferenceParser::new();
        let content = r#"This is line 1 with :doc:`doc1`.
This is line 2 with :func:`function1`.
This is line 3 with :ref:`section1`."#;

        let refs = parser.parse_content(content, "test", None);
        assert_eq!(refs.len(), 3);

        // Check line numbers
        assert_eq!(refs[0].source_location.lineno, Some(1));
        assert_eq!(refs[1].source_location.lineno, Some(2));
        assert_eq!(refs[2].source_location.lineno, Some(3));
    }

    #[test]
    fn test_reference_stats() {
        let parser = ReferenceParser::new();
        let content = r#"Use :doc:`doc1` and :doc:`doc2`.
Also :func:`func1` and :class:`class1`."#;

        let refs = parser.parse_content(content, "test", None);
        let stats = parser.get_reference_stats(&refs);

        assert_eq!(stats.get("Document"), Some(&2));
        assert_eq!(stats.get("Function"), Some(&1));
        assert_eq!(stats.get("Class"), Some(&1));
    }
}
