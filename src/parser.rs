use anyhow::Result;
use log::debug;
use pulldown_cmark::{Event, Parser as MarkdownParser, Tag};
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;

use crate::config::BuildConfig;
use crate::directives::DirectiveRegistry;
use crate::document::{
    CrossReference, Document, DocumentContent, MarkdownContent, MarkdownNode, RstContent,
    RstDirective, RstNode, TocEntry,
};
// use crate::roles::RoleRegistry; // TODO: Implement roles module
use crate::utils;

pub struct Parser {
    rst_directive_regex: Regex,
    cross_ref_regex: Regex,
    #[allow(dead_code)]
    directive_registry: DirectiveRegistry,
    // #[allow(dead_code)]
    // role_registry: RoleRegistry, // TODO: Implement roles module
}

impl Parser {
    pub fn new(_config: &BuildConfig) -> Result<Self> {
        // Directive names follow the docutils pattern: alphanumeric plus
        // internal ._+:- (covers `code-block` and domain forms like `py:function`).
        let rst_directive_regex = Regex::new(r"^\s*\.\.\s+([a-zA-Z][a-zA-Z0-9._+:-]*)::\s*(.*?)$")?;
        let cross_ref_regex = Regex::new(r":(\w+):`([^`]+)`")?;
        let directive_registry = DirectiveRegistry::new();
        // let role_registry = RoleRegistry::new(); // TODO: Implement roles module

        Ok(Self {
            rst_directive_regex,
            cross_ref_regex,
            directive_registry,
            // role_registry, // TODO: Implement roles module
        })
    }

    pub fn parse(&self, file_path: &Path, content: &str) -> Result<Document> {
        let output_path = self.get_output_path(file_path)?;
        let mut document = Document::new(file_path.to_path_buf(), output_path);

        // Set source modification time
        document.source_mtime = utils::get_file_mtime(file_path)?;

        // Determine file type and parse accordingly
        let extension = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");

        match extension {
            "rst" => {
                document.content = self.parse_rst(content)?;
            }
            "md" => {
                document.content = self.parse_markdown(content)?;
            }
            _ => {
                document.content = DocumentContent::PlainText(content.to_string());
            }
        }

        // Extract title from content
        document.title = self.extract_title(&document.content);

        // Extract table of contents
        document.toc = self.extract_toc(&document.content);

        // Extract cross-references
        document.cross_refs = self.extract_cross_refs(content);

        debug!(
            "Parsed document: {} ({} chars)",
            file_path.display(),
            content.len()
        );

        Ok(document)
    }

    fn parse_rst(&self, content: &str) -> Result<DocumentContent> {
        let mut nodes = Vec::new();
        let mut directives = Vec::new();
        // docutils assigns section levels by order of first use of each
        // adornment style, not by a fixed character table.
        let mut adornment_order: Vec<char> = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        let mut i = 0;
        while i < lines.len() {
            let line = lines[i];
            let trimmed = line.trim();

            if trimmed.is_empty() {
                i += 1;
                continue;
            }

            // Check for RST directive
            if let Some(captures) = self.rst_directive_regex.captures(line) {
                let directive_name = captures.get(1).unwrap().as_str();
                let directive_args = captures.get(2).unwrap().as_str();

                let (directive, consumed_lines) =
                    self.parse_rst_directive(&lines[i..], directive_name, directive_args, i + 1)?;

                directives.push(directive.clone());
                nodes.push(RstNode::Directive {
                    name: directive.name,
                    args: directive.args,
                    options: directive.options,
                    content: directive.content,
                    line: i + 1,
                });

                i += consumed_lines;
                continue;
            }

            // Check for title (underlined with =, -, ~, etc.)
            if i + 1 < lines.len() {
                let next_line = lines[i + 1];
                if !next_line.trim().is_empty()
                    && next_line.chars().all(|c| "=-~^\"'*+#<>".contains(c))
                    && next_line.len() >= trimmed.len()
                {
                    let adornment = next_line.chars().next().unwrap();
                    let level = match adornment_order.iter().position(|&c| c == adornment) {
                        Some(pos) => pos + 1,
                        None => {
                            adornment_order.push(adornment);
                            adornment_order.len()
                        }
                    };
                    nodes.push(RstNode::Title {
                        text: trimmed.to_string(),
                        level,
                        line: i + 1,
                    });

                    i += 2;
                    continue;
                }
            }

            // Check for code block (indented text after ::)
            if line.ends_with("::") {
                let (code_content, consumed_lines) = self.parse_code_block(&lines[i + 1..]);
                nodes.push(RstNode::CodeBlock {
                    language: None,
                    content: code_content,
                    line: i + 1,
                });
                i += consumed_lines + 1;
                continue;
            }

            // Default to paragraph
            let (paragraph_content, consumed_lines) = self.parse_paragraph(&lines[i..]);
            nodes.push(RstNode::Paragraph {
                content: paragraph_content,
                line: i + 1,
            });
            i += consumed_lines;
        }

        Ok(DocumentContent::RestructuredText(RstContent {
            raw: content.to_string(),
            ast: nodes,
            directives,
        }))
    }

    fn parse_markdown(&self, content: &str) -> Result<DocumentContent> {
        let mut nodes = Vec::new();
        let parser = MarkdownParser::new(content);
        let current_line = 1;

        for event in parser {
            match event {
                Event::Start(Tag::Heading { .. }) => {
                    // We'll handle this in the text event
                }
                Event::End(_) => {
                    // Handle end tags generically
                }
                Event::Start(Tag::Paragraph) => {
                    // Start of paragraph
                }
                Event::Start(Tag::CodeBlock(_)) => {
                    // Start of code block
                }
                Event::Text(text) => {
                    // Handle text content based on context
                    nodes.push(MarkdownNode::Paragraph {
                        content: text.to_string(),
                        line: current_line,
                    });
                }
                Event::Code(_code) => {
                    // Inline code
                }
                _ => {
                    // Handle other events as needed
                }
            }
        }

        Ok(DocumentContent::Markdown(MarkdownContent {
            raw: content.to_string(),
            ast: nodes,
            front_matter: None, // TODO: Parse YAML front matter
        }))
    }

    fn parse_rst_directive(
        &self,
        lines: &[&str],
        name: &str,
        args: &str,
        start_line: usize,
    ) -> Result<(RstDirective, usize)> {
        let mut options = HashMap::new();
        let mut content = String::new();
        let mut consumed_lines = 1;
        let mut i = 1;

        // Parse options (lines starting with :option:)
        while i < lines.len() {
            let line = lines[i];
            if line.trim().is_empty() {
                i += 1;
                consumed_lines += 1;
                continue;
            }

            if let Some(stripped) = line
                .strip_prefix("   :")
                .or_else(|| line.strip_prefix("\t:"))
            {
                // This is an option
                if let Some(colon_pos) = stripped.find(':') {
                    let option_name = &stripped[..colon_pos];
                    let option_value = stripped[colon_pos + 1..].trim();
                    options.insert(option_name.to_string(), option_value.to_string());
                }
                i += 1;
                consumed_lines += 1;
            } else if line.starts_with("   ") || line.starts_with("\t") {
                // This is content
                break;
            } else {
                // End of directive
                break;
            }
        }

        // Parse content (indented lines)
        while i < lines.len() {
            let line = lines[i];
            if line.starts_with("   ") || line.starts_with('\t') {
                // Dedent one indentation unit without byte-slicing (a tab is one
                // byte; `&line[3..]` panicked on short tab-indented lines).
                let dedented = line
                    .strip_prefix("   ")
                    .or_else(|| line.strip_prefix('\t'))
                    .unwrap_or(line);
                content.push_str(dedented);
                content.push('\n');
                i += 1;
                consumed_lines += 1;
            } else if line.trim().is_empty() {
                content.push('\n');
                i += 1;
                consumed_lines += 1;
            } else {
                break;
            }
        }

        let directive = RstDirective {
            name: name.to_string(),
            args: if args.is_empty() {
                Vec::new()
            } else {
                vec![args.to_string()]
            },
            options,
            content: content.trim_end().to_string(),
            line: start_line,
        };

        Ok((directive, consumed_lines))
    }

    fn parse_code_block(&self, lines: &[&str]) -> (String, usize) {
        let mut content = String::new();
        let mut consumed_lines = 0;

        for line in lines {
            if line.starts_with("   ") || line.starts_with("\t") || line.trim().is_empty() {
                content.push_str(line);
                content.push('\n');
                consumed_lines += 1;
            } else {
                break;
            }
        }

        (content.trim().to_string(), consumed_lines)
    }

    fn parse_paragraph(&self, lines: &[&str]) -> (String, usize) {
        let mut content = String::new();
        let mut consumed_lines = 0;

        for line in lines {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }

            content.push_str(trimmed);
            content.push(' ');
            consumed_lines += 1;
        }

        (content.trim().to_string(), consumed_lines)
    }

    fn extract_title(&self, content: &DocumentContent) -> String {
        match content {
            DocumentContent::RestructuredText(rst) => {
                for node in &rst.ast {
                    if let RstNode::Title { text, level: 1, .. } = node {
                        return text.clone();
                    }
                }
            }
            DocumentContent::Markdown(md) => {
                for node in &md.ast {
                    if let MarkdownNode::Heading { text, level: 1, .. } = node {
                        return text.clone();
                    }
                }
            }
            DocumentContent::PlainText(_) => {}
        }

        "Untitled".to_string()
    }

    fn extract_toc(&self, content: &DocumentContent) -> Vec<TocEntry> {
        let mut toc = Vec::new();

        match content {
            DocumentContent::RestructuredText(rst) => {
                for node in &rst.ast {
                    if let RstNode::Title { text, level, line } = node {
                        let anchor = text.to_lowercase().replace(' ', "-");
                        toc.push(TocEntry::new(text.clone(), *level, anchor, *line));
                    }
                }
            }
            DocumentContent::Markdown(md) => {
                for node in &md.ast {
                    if let MarkdownNode::Heading { text, level, line } = node {
                        let anchor = text.to_lowercase().replace(' ', "-");
                        toc.push(TocEntry::new(text.clone(), *level, anchor, *line));
                    }
                }
            }
            DocumentContent::PlainText(_) => {}
        }

        toc
    }

    fn extract_cross_refs(&self, content: &str) -> Vec<CrossReference> {
        let mut cross_refs = Vec::new();

        for (line_num, line) in content.lines().enumerate() {
            for captures in self.cross_ref_regex.captures_iter(line) {
                let ref_type = captures.get(1).unwrap().as_str();
                let target = captures.get(2).unwrap().as_str();

                cross_refs.push(CrossReference {
                    ref_type: ref_type.to_string(),
                    target: target.to_string(),
                    text: None,
                    line_number: line_num + 1,
                });
            }
        }

        cross_refs
    }

    fn get_output_path(&self, source_path: &Path) -> Result<std::path::PathBuf> {
        let mut output_path = source_path.to_path_buf();
        output_path.set_extension("html");
        Ok(output_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BuildConfig;

    fn parse_rst_ast(content: &str) -> Vec<RstNode> {
        let parser = Parser::new(&BuildConfig::default()).unwrap();
        match parser.parse_rst(content).unwrap() {
            DocumentContent::RestructuredText(rst) => rst.ast,
            _ => unreachable!(),
        }
    }

    #[test]
    fn hyphenated_directive_is_recognized() {
        let ast = parse_rst_ast(".. code-block:: python\n\n   x = 1\n");
        assert!(
            ast.iter()
                .any(|n| matches!(n, RstNode::Directive { name, .. } if name == "code-block")),
            "code-block must parse as a directive, got: {ast:?}"
        );
    }

    #[test]
    fn domain_directive_is_recognized() {
        let ast = parse_rst_ast(".. py:function:: foo(x)\n\n   Does foo.\n");
        assert!(
            ast.iter()
                .any(|n| matches!(n, RstNode::Directive { name, .. } if name == "py:function")),
            "py:function must parse as a directive, got: {ast:?}"
        );
    }

    #[test]
    fn tab_indented_directive_content_does_not_panic() {
        let ast = parse_rst_ast(".. note::\n\n\tshort\n");
        let content = ast.iter().find_map(|n| match n {
            RstNode::Directive { content, .. } => Some(content.clone()),
            _ => None,
        });
        assert!(
            content.expect("directive parsed").contains("short"),
            "tab-indented content must be captured"
        );
    }

    #[test]
    fn equals_underline_gets_level_one() {
        let parser = Parser::new(&BuildConfig::default()).unwrap();
        let content_ast = parser.parse_rst("Title\n=====\n\nBody.\n").unwrap();
        if let DocumentContent::RestructuredText(ref rst) = content_ast {
            assert!(
                rst.ast
                    .iter()
                    .any(|n| matches!(n, RstNode::Title { level: 1, .. })),
                "first adornment style must be level 1, got: {:?}",
                rst.ast
            );
        }
        assert_eq!(parser.extract_title(&content_ast), "Title");
    }

    #[test]
    fn adornment_levels_by_order_of_first_use() {
        let ast = parse_rst_ast("One\n===\n\nTwo\n---\n\nAlso One\n========\n");
        let levels: Vec<usize> = ast
            .iter()
            .filter_map(|n| match n {
                RstNode::Title { level, .. } => Some(*level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, vec![1, 2, 1], "levels follow order of first use");
    }
}
