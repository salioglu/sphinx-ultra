//! Built-in directive validators for common Sphinx directives

use super::{DirectiveValidationResult, DirectiveValidator, ParsedDirective};

/// Validator for code-block directive
#[derive(Default)]
pub struct CodeBlockValidator;

impl CodeBlockValidator {
    pub fn new() -> Self {
        Self
    }
}

impl DirectiveValidator for CodeBlockValidator {
    fn name(&self) -> &str {
        "code-block"
    }

    fn validate(&self, directive: &ParsedDirective) -> DirectiveValidationResult {
        // Check if language is specified
        if directive.arguments.is_empty() {
            return DirectiveValidationResult::Warning(
                "No language specified for code-block directive".to_string(),
            );
        }

        // Check for valid language
        let language = &directive.arguments[0];
        if language.is_empty() {
            return DirectiveValidationResult::Error(
                "Empty language specification in code-block directive".to_string(),
            );
        }

        // Check if content is provided
        if directive.content.trim().is_empty() {
            return DirectiveValidationResult::Warning(
                "Code-block directive has no content".to_string(),
            );
        }

        // Validate common options
        for (option, value) in &directive.options {
            match option.as_str() {
                "linenos" => {
                    if !value.is_empty() {
                        return DirectiveValidationResult::Error(
                            "linenos option should not have a value".to_string(),
                        );
                    }
                }
                "lineno-start" => {
                    if value.parse::<u32>().is_err() {
                        return DirectiveValidationResult::Error(
                            "lineno-start must be a positive integer".to_string(),
                        );
                    }
                }
                "emphasize-lines" => {
                    // Could validate line numbers format here
                }
                "caption" | "name" | "dedent" => {
                    // These are valid options
                }
                _ => {
                    return DirectiveValidationResult::Warning(format!(
                        "Unknown option '{}' for code-block directive",
                        option
                    ));
                }
            }
        }

        DirectiveValidationResult::Valid
    }

    fn expected_arguments(&self) -> Vec<String> {
        vec!["language".to_string()]
    }

    fn valid_options(&self) -> Vec<String> {
        vec![
            "linenos".to_string(),
            "lineno-start".to_string(),
            "emphasize-lines".to_string(),
            "caption".to_string(),
            "name".to_string(),
            "dedent".to_string(),
            "force".to_string(),
        ]
    }

    fn requires_content(&self) -> bool {
        false // Can be empty for demonstration purposes
    }

    fn allows_content(&self) -> bool {
        true
    }
}

/// Validator for note directive
#[derive(Default)]
pub struct NoteValidator;

impl NoteValidator {
    pub fn new() -> Self {
        Self
    }
}

impl DirectiveValidator for NoteValidator {
    fn name(&self) -> &str {
        "note"
    }

    fn validate(&self, directive: &ParsedDirective) -> DirectiveValidationResult {
        // Note directive should have content
        if directive.content.trim().is_empty() {
            return DirectiveValidationResult::Error("Note directive requires content".to_string());
        }

        // Note directive typically doesn't take arguments
        if !directive.arguments.is_empty() {
            return DirectiveValidationResult::Warning(
                "Note directive does not expect arguments".to_string(),
            );
        }

        // Validate options
        for option in directive.options.keys() {
            match option.as_str() {
                "class" | "name" => {
                    // Valid options
                }
                _ => {
                    return DirectiveValidationResult::Warning(format!(
                        "Unknown option '{}' for note directive",
                        option
                    ));
                }
            }
        }

        DirectiveValidationResult::Valid
    }

    fn expected_arguments(&self) -> Vec<String> {
        vec![]
    }

    fn valid_options(&self) -> Vec<String> {
        vec!["class".to_string(), "name".to_string()]
    }

    fn requires_content(&self) -> bool {
        true
    }

    fn allows_content(&self) -> bool {
        true
    }
}

/// Validator for warning directive
#[derive(Default)]
pub struct WarningValidator;

impl WarningValidator {
    pub fn new() -> Self {
        Self
    }
}

impl DirectiveValidator for WarningValidator {
    fn name(&self) -> &str {
        "warning"
    }

    fn validate(&self, directive: &ParsedDirective) -> DirectiveValidationResult {
        // Warning directive should have content
        if directive.content.trim().is_empty() {
            return DirectiveValidationResult::Error(
                "Warning directive requires content".to_string(),
            );
        }

        // Warning directive typically doesn't take arguments
        if !directive.arguments.is_empty() {
            return DirectiveValidationResult::Warning(
                "Warning directive does not expect arguments".to_string(),
            );
        }

        // Validate options
        for option in directive.options.keys() {
            match option.as_str() {
                "class" | "name" => {
                    // Valid options
                }
                _ => {
                    return DirectiveValidationResult::Warning(format!(
                        "Unknown option '{}' for warning directive",
                        option
                    ));
                }
            }
        }

        DirectiveValidationResult::Valid
    }

    fn expected_arguments(&self) -> Vec<String> {
        vec![]
    }

    fn valid_options(&self) -> Vec<String> {
        vec!["class".to_string(), "name".to_string()]
    }

    fn requires_content(&self) -> bool {
        true
    }

    fn allows_content(&self) -> bool {
        true
    }
}

/// Validator for image directive
#[derive(Default)]
pub struct ImageValidator;

impl ImageValidator {
    pub fn new() -> Self {
        Self
    }
}

impl DirectiveValidator for ImageValidator {
    fn name(&self) -> &str {
        "image"
    }

    fn validate(&self, directive: &ParsedDirective) -> DirectiveValidationResult {
        // Image directive requires a path argument
        if directive.arguments.is_empty() {
            return DirectiveValidationResult::Error(
                "Image directive requires a path argument".to_string(),
            );
        }

        let image_path = &directive.arguments[0];
        if image_path.is_empty() {
            return DirectiveValidationResult::Error("Image path cannot be empty".to_string());
        }

        // Check for valid image extensions
        let valid_extensions = ["png", "jpg", "jpeg", "gif", "svg", "bmp", "webp"];
        if let Some(extension) = image_path.split('.').next_back() {
            if !valid_extensions.contains(&extension.to_lowercase().as_str()) {
                return DirectiveValidationResult::Warning(format!(
                    "Unusual image extension: {}",
                    extension
                ));
            }
        }

        // Validate options
        for (option, value) in &directive.options {
            match option.as_str() {
                "alt" | "target" | "class" | "name" => {
                    // Valid text options
                }
                "width" | "height" => {
                    // Should be length units
                    if !value.ends_with("px") && !value.ends_with("%") && !value.ends_with("em") {
                        return DirectiveValidationResult::Warning(format!(
                            "{} should include units (px, %, em)",
                            option
                        ));
                    }
                }
                "scale" => {
                    if value.parse::<f32>().is_err() {
                        return DirectiveValidationResult::Error(
                            "Scale must be a number".to_string(),
                        );
                    }
                }
                "align" => {
                    let valid_alignments = ["left", "center", "right", "top", "middle", "bottom"];
                    if !valid_alignments.contains(&value.as_str()) {
                        return DirectiveValidationResult::Error(format!(
                            "Invalid alignment: {}. Valid options: {}",
                            value,
                            valid_alignments.join(", ")
                        ));
                    }
                }
                _ => {
                    return DirectiveValidationResult::Warning(format!(
                        "Unknown option '{}' for image directive",
                        option
                    ));
                }
            }
        }

        DirectiveValidationResult::Valid
    }

    fn expected_arguments(&self) -> Vec<String> {
        vec!["image_uri".to_string()]
    }

    fn valid_options(&self) -> Vec<String> {
        vec![
            "alt".to_string(),
            "height".to_string(),
            "width".to_string(),
            "scale".to_string(),
            "align".to_string(),
            "target".to_string(),
            "class".to_string(),
            "name".to_string(),
        ]
    }

    fn requires_content(&self) -> bool {
        false
    }

    fn allows_content(&self) -> bool {
        false
    }
}

/// Validator for figure directive
#[derive(Default)]
pub struct FigureValidator;

impl FigureValidator {
    pub fn new() -> Self {
        Self
    }
}

impl DirectiveValidator for FigureValidator {
    fn name(&self) -> &str {
        "figure"
    }

    fn validate(&self, directive: &ParsedDirective) -> DirectiveValidationResult {
        // Figure directive requires a path argument
        if directive.arguments.is_empty() {
            return DirectiveValidationResult::Error(
                "Figure directive requires a path argument".to_string(),
            );
        }

        // Reuse image validation logic
        let image_validator = ImageValidator::new();
        let mut temp_directive = directive.clone();
        temp_directive.name = "image".to_string();
        let image_result = image_validator.validate(&temp_directive);

        // Figure can have content (caption)
        match image_result {
            DirectiveValidationResult::Valid => DirectiveValidationResult::Valid,
            other => other,
        }
    }

    fn expected_arguments(&self) -> Vec<String> {
        vec!["image_uri".to_string()]
    }

    fn valid_options(&self) -> Vec<String> {
        vec![
            "alt".to_string(),
            "height".to_string(),
            "width".to_string(),
            "scale".to_string(),
            "align".to_string(),
            "target".to_string(),
            "class".to_string(),
            "name".to_string(),
            "figwidth".to_string(),
            "figclass".to_string(),
        ]
    }

    fn requires_content(&self) -> bool {
        false
    }

    fn allows_content(&self) -> bool {
        true
    }
}

/// Validator for toctree directive
#[derive(Default)]
pub struct TocTreeValidator;

impl TocTreeValidator {
    pub fn new() -> Self {
        Self
    }
}

impl DirectiveValidator for TocTreeValidator {
    fn name(&self) -> &str {
        "toctree"
    }

    fn validate(&self, directive: &ParsedDirective) -> DirectiveValidationResult {
        // Toctree typically has content (list of documents)
        if directive.content.trim().is_empty() {
            return DirectiveValidationResult::Warning("Toctree directive is empty".to_string());
        }

        // Validate options
        for (option, value) in &directive.options {
            match option.as_str() {
                "maxdepth" => {
                    if let Ok(depth) = value.parse::<u32>() {
                        if depth > 10 {
                            return DirectiveValidationResult::Warning(
                                "Very deep toctree depth may cause performance issues".to_string(),
                            );
                        }
                    } else {
                        return DirectiveValidationResult::Error(
                            "maxdepth must be a positive integer".to_string(),
                        );
                    }
                }
                "numbered" | "titlesonly" | "glob" | "reversed" | "hidden" | "includehidden" => {
                    // Flag options
                    if !value.is_empty() {
                        return DirectiveValidationResult::Warning(format!(
                            "{} option should not have a value",
                            option
                        ));
                    }
                }
                "caption" | "name" | "class" => {
                    // Valid text options
                }
                _ => {
                    return DirectiveValidationResult::Warning(format!(
                        "Unknown option '{}' for toctree directive",
                        option
                    ));
                }
            }
        }

        DirectiveValidationResult::Valid
    }

    fn expected_arguments(&self) -> Vec<String> {
        vec![]
    }

    fn valid_options(&self) -> Vec<String> {
        vec![
            "maxdepth".to_string(),
            "numbered".to_string(),
            "titlesonly".to_string(),
            "glob".to_string(),
            "reversed".to_string(),
            "hidden".to_string(),
            "includehidden".to_string(),
            "caption".to_string(),
            "name".to_string(),
            "class".to_string(),
        ]
    }

    fn requires_content(&self) -> bool {
        false
    }

    fn allows_content(&self) -> bool {
        true
    }
}

/// Validator for include directive
#[derive(Default)]
pub struct IncludeValidator;

impl IncludeValidator {
    pub fn new() -> Self {
        Self
    }
}

impl DirectiveValidator for IncludeValidator {
    fn name(&self) -> &str {
        "include"
    }

    fn validate(&self, directive: &ParsedDirective) -> DirectiveValidationResult {
        // Include directive requires a file path
        if directive.arguments.is_empty() {
            return DirectiveValidationResult::Error(
                "Include directive requires a file path".to_string(),
            );
        }

        let file_path = &directive.arguments[0];
        if file_path.is_empty() {
            return DirectiveValidationResult::Error(
                "Include file path cannot be empty".to_string(),
            );
        }

        // Check for common file extensions
        if let Some(extension) = file_path.split('.').next_back() {
            let valid_extensions = ["rst", "txt", "md", "inc"];
            if !valid_extensions.contains(&extension.to_lowercase().as_str()) {
                return DirectiveValidationResult::Warning(format!(
                    "Unusual file extension for include: {}",
                    extension
                ));
            }
        }

        DirectiveValidationResult::Valid
    }

    fn expected_arguments(&self) -> Vec<String> {
        vec!["filename".to_string()]
    }

    fn valid_options(&self) -> Vec<String> {
        vec![
            "start-line".to_string(),
            "end-line".to_string(),
            "start-after".to_string(),
            "end-before".to_string(),
            "literal".to_string(),
            "code".to_string(),
            "number-lines".to_string(),
            "encoding".to_string(),
            "tab-width".to_string(),
        ]
    }

    fn requires_content(&self) -> bool {
        false
    }

    fn allows_content(&self) -> bool {
        false
    }
}

/// Validator for literalinclude directive
#[derive(Default)]
pub struct LiteralIncludeValidator;

impl LiteralIncludeValidator {
    pub fn new() -> Self {
        Self
    }
}

impl DirectiveValidator for LiteralIncludeValidator {
    fn name(&self) -> &str {
        "literalinclude"
    }

    fn validate(&self, directive: &ParsedDirective) -> DirectiveValidationResult {
        // Similar to include but for code files
        if directive.arguments.is_empty() {
            return DirectiveValidationResult::Error(
                "Literalinclude directive requires a file path".to_string(),
            );
        }

        let file_path = &directive.arguments[0];
        if file_path.is_empty() {
            return DirectiveValidationResult::Error(
                "Literalinclude file path cannot be empty".to_string(),
            );
        }

        // Validate line number options
        for (option, value) in &directive.options {
            match option.as_str() {
                "start-line" | "end-line" | "lineno-start" | "tab-width" => {
                    if value.parse::<u32>().is_err() {
                        return DirectiveValidationResult::Error(format!(
                            "{} must be a positive integer",
                            option
                        ));
                    }
                }
                "dedent" => {
                    if !value.is_empty() && value.parse::<u32>().is_err() {
                        return DirectiveValidationResult::Error(
                            "dedent must be a positive integer".to_string(),
                        );
                    }
                }
                "language" | "start-after" | "end-before" | "prepend" | "append" | "caption"
                | "name" | "class" | "encoding" | "pyobject" | "diff" => {
                    // Valid text options
                }
                "linenos" | "force" => {
                    // Flag options
                    if !value.is_empty() {
                        return DirectiveValidationResult::Warning(format!(
                            "{} option should not have a value",
                            option
                        ));
                    }
                }
                _ => {
                    return DirectiveValidationResult::Warning(format!(
                        "Unknown option '{}' for literalinclude directive",
                        option
                    ));
                }
            }
        }

        DirectiveValidationResult::Valid
    }

    fn expected_arguments(&self) -> Vec<String> {
        vec!["filename".to_string()]
    }

    fn valid_options(&self) -> Vec<String> {
        vec![
            "language".to_string(),
            "linenos".to_string(),
            "lineno-start".to_string(),
            "emphasize-lines".to_string(),
            "lines".to_string(),
            "start-line".to_string(),
            "end-line".to_string(),
            "start-after".to_string(),
            "end-before".to_string(),
            "prepend".to_string(),
            "append".to_string(),
            "dedent".to_string(),
            "tab-width".to_string(),
            "encoding".to_string(),
            "pyobject".to_string(),
            "caption".to_string(),
            "name".to_string(),
            "class".to_string(),
            "diff".to_string(),
            "force".to_string(),
        ]
    }

    fn requires_content(&self) -> bool {
        false
    }

    fn allows_content(&self) -> bool {
        false
    }
}

/// Validator for admonition directive
#[derive(Default)]
pub struct AdmonitionValidator;

impl AdmonitionValidator {
    pub fn new() -> Self {
        Self
    }
}

impl DirectiveValidator for AdmonitionValidator {
    fn name(&self) -> &str {
        "admonition"
    }

    fn validate(&self, directive: &ParsedDirective) -> DirectiveValidationResult {
        // Admonition directive requires a title argument
        if directive.arguments.is_empty() {
            return DirectiveValidationResult::Error(
                "Admonition directive requires a title argument".to_string(),
            );
        }

        // Should have content
        if directive.content.trim().is_empty() {
            return DirectiveValidationResult::Warning(
                "Admonition directive has no content".to_string(),
            );
        }

        DirectiveValidationResult::Valid
    }

    fn expected_arguments(&self) -> Vec<String> {
        vec!["title".to_string()]
    }

    fn valid_options(&self) -> Vec<String> {
        vec!["class".to_string(), "name".to_string()]
    }

    fn requires_content(&self) -> bool {
        false
    }

    fn allows_content(&self) -> bool {
        true
    }
}

/// Validator for math directive
#[derive(Default)]
pub struct MathValidator;

impl MathValidator {
    pub fn new() -> Self {
        Self
    }
}

impl DirectiveValidator for MathValidator {
    fn name(&self) -> &str {
        "math"
    }

    fn validate(&self, directive: &ParsedDirective) -> DirectiveValidationResult {
        // Math directive should have content
        if directive.content.trim().is_empty() {
            return DirectiveValidationResult::Error(
                "Math directive requires LaTeX math content".to_string(),
            );
        }

        // Basic LaTeX syntax check
        let content = directive.content.trim();
        let open_braces = content.matches('{').count();
        let close_braces = content.matches('}').count();

        if open_braces != close_braces {
            return DirectiveValidationResult::Warning(
                "Unmatched braces in math content".to_string(),
            );
        }

        DirectiveValidationResult::Valid
    }

    fn expected_arguments(&self) -> Vec<String> {
        vec![]
    }

    fn valid_options(&self) -> Vec<String> {
        vec!["label".to_string(), "name".to_string(), "class".to_string()]
    }

    fn requires_content(&self) -> bool {
        true
    }

    fn allows_content(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directives::validation::SourceLocation;
    use std::collections::HashMap;

    fn create_test_directive(
        name: &str,
        args: Vec<String>,
        options: HashMap<String, String>,
        content: &str,
    ) -> ParsedDirective {
        ParsedDirective {
            name: name.to_string(),
            arguments: args,
            options,
            content: content.to_string(),
            location: SourceLocation {
                file: "test.rst".to_string(),
                line: 1,
                column: 1,
            },
        }
    }

    #[test]
    fn test_code_block_validator() {
        let validator = CodeBlockValidator::new();

        // Valid code block
        let directive = create_test_directive(
            "code-block",
            vec!["python".to_string()],
            HashMap::new(),
            "print('Hello, world!')",
        );
        assert_eq!(
            validator.validate(&directive),
            DirectiveValidationResult::Valid
        );

        // Missing language
        let directive = create_test_directive(
            "code-block",
            vec![],
            HashMap::new(),
            "print('Hello, world!')",
        );
        assert!(matches!(
            validator.validate(&directive),
            DirectiveValidationResult::Warning(_)
        ));
    }

    #[test]
    fn test_note_validator() {
        let validator = NoteValidator::new();

        // Valid note
        let directive = create_test_directive("note", vec![], HashMap::new(), "This is a note");
        assert_eq!(
            validator.validate(&directive),
            DirectiveValidationResult::Valid
        );

        // Missing content
        let directive = create_test_directive("note", vec![], HashMap::new(), "");
        assert!(matches!(
            validator.validate(&directive),
            DirectiveValidationResult::Error(_)
        ));
    }

    #[test]
    fn test_image_validator() {
        let validator = ImageValidator::new();

        // Valid image
        let directive =
            create_test_directive("image", vec!["test.png".to_string()], HashMap::new(), "");
        assert_eq!(
            validator.validate(&directive),
            DirectiveValidationResult::Valid
        );

        // Missing path
        let directive = create_test_directive("image", vec![], HashMap::new(), "");
        assert!(matches!(
            validator.validate(&directive),
            DirectiveValidationResult::Error(_)
        ));
    }

    #[test]
    fn test_math_validator() {
        let validator = MathValidator::new();

        // Valid math
        let directive = create_test_directive("math", vec![], HashMap::new(), "x = \\frac{a}{b}");
        assert_eq!(
            validator.validate(&directive),
            DirectiveValidationResult::Valid
        );

        // Missing content
        let directive = create_test_directive("math", vec![], HashMap::new(), "");
        assert!(matches!(
            validator.validate(&directive),
            DirectiveValidationResult::Error(_)
        ));
    }
}
