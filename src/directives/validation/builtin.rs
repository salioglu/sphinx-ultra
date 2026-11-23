//! Built-in directive validators for common Sphinx directives
//!
//! ## Option lists
//!
//! Each validator's option list is spelled ONCE, as a shared `&[&str]`
//! const, for two reasons found the hard way in wave 4.5:
//!
//! * `valid_options()` returns a freshly allocated `Vec<String>` on every
//!   call, and `LiteralIncludeValidator::validate` calls it from inside its
//!   per-option loop;
//! * more importantly, a validator that spells its options twice drifts.
//!   Task 14's env oracle caught the build warning `Unknown option 'lines'`
//!   on a `literalinclude` — against an option the very same validator
//!   advertised as valid — and the task-16 audit found the same shape in
//!   `code-block` (`force`), `figure` (`figwidth`/`figclass`, warned about
//!   under the *image* directive's name) and `image` (`loading`).
//!
//! Every list mirrors the directive's parse-time `option_spec` in
//! `src/rst/block.rs`, which is this crate's probe-verified transcription
//! of the real docutils/sphinx spec. The test
//! `validator_option_lists_match_the_parser_spec` holds the two together in
//! BOTH directions, and `every_validator_accepts_every_option_it_advertises`
//! holds each list against its own `validate`. A fabricated "Unknown
//! option" warning is not cosmetic: it fails `-W` on projects Sphinx builds
//! clean.

use super::{DirectiveValidationResult, DirectiveValidator, ParsedDirective};

/// `CODE_BLOCK_OPTS` (`SP/directives/code.py` CodeBlock.option_spec).
const CODE_BLOCK_OPTIONS: &[&str] = &[
    "force",
    "linenos",
    "dedent",
    "lineno-start",
    "emphasize-lines",
    "caption",
    "class",
    "name",
];

/// `SPHINX_ADMONITION_OPTS` — shared by `note`, `warning` and
/// `admonition`: docutils' table plus Sphinx's `collapsible`.
const ADMONITION_OPTIONS: &[&str] = &["class", "name", "collapsible"];

/// `IMAGE_OPTS` (`DU/parsers/rst/directives/images.py` Image.option_spec).
const IMAGE_OPTIONS: &[&str] = &[
    "alt", "height", "width", "scale", "align", "target", "loading", "class", "name",
];

/// `FIGURE_OPTS`: the image set plus the three figure-only options.
const FIGURE_OPTIONS: &[&str] = &[
    "alt", "height", "width", "scale", "align", "target", "loading", "class", "name", "figwidth",
    "figclass", "figname",
];

/// The options `FigureValidator` handles itself instead of delegating to
/// [`ImageValidator`], which does not know them.
const FIGURE_ONLY_OPTIONS: &[&str] = &["figwidth", "figclass", "figname"];

/// `TOCTREE_OPTS` (`SP/directives/other.py` TocTree.option_spec).
const TOCTREE_OPTIONS: &[&str] = &[
    "maxdepth",
    "name",
    "class",
    "caption",
    "glob",
    "hidden",
    "includehidden",
    "numbered",
    "titlesonly",
    "reversed",
];

/// `INCLUDE_OPTS` (`DU/parsers/rst/directives/misc.py` Include.option_spec).
const INCLUDE_OPTIONS: &[&str] = &[
    "literal",
    "code",
    "encoding",
    "parser",
    "tab-width",
    "start-line",
    "end-line",
    "start-after",
    "end-before",
    "number-lines",
    "class",
    "name",
];

/// `LITERALINCLUDE_OPTS` (`SP/directives/code.py` LiteralInclude).
///
/// NOTE the two names that are NOT here: `start-line` and `end-line`
/// belong to docutils' `include`, and Sphinx's `literalinclude` has
/// neither (probe: `sorted(LiteralInclude.option_spec)` on 9.1.0 lists 21
/// names, none of them those). They were advertised anyway, so the
/// validator accepted an option the parser rejects.
const LITERALINCLUDE_OPTIONS: &[&str] = &[
    "dedent",
    "linenos",
    "lineno-start",
    "lineno-match",
    "tab-width",
    "language",
    "force",
    "encoding",
    "pyobject",
    "lines",
    "start-after",
    "end-before",
    "start-at",
    "end-at",
    "prepend",
    "append",
    "emphasize-lines",
    "caption",
    "class",
    "name",
    "diff",
];

/// `SPHINX_MATH_OPTS` (`SP/directives/patches.py` MathDirective).
const MATH_OPTIONS: &[&str] = &["label", "name", "class", "no-wrap", "nowrap"];

/// The owned form the [`DirectiveValidator::valid_options`] signature asks
/// for. Allocating here keeps the const the single spelling.
fn names(options: &[&'static str]) -> Vec<String> {
    options.iter().map(|name| (*name).to_string()).collect()
}

/// A docutils length: a number with an optional unit (bare numbers default
/// to pixels).
fn is_valid_length(value: &str) -> bool {
    const UNITS: &[&str] = &["em", "ex", "px", "in", "cm", "mm", "pt", "pc", "%"];
    let number = UNITS
        .iter()
        .find_map(|u| value.strip_suffix(u))
        .unwrap_or(value);
    !number.trim().is_empty() && number.trim().parse::<f64>().is_ok()
}

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
        // A bare `.. code-block::` is valid Sphinx: the language falls back to
        // highlight_language. An EMPTY code-block is valid too — Sphinx's
        // `CodeBlock.run` renders an empty literal_block without a word
        // (`directives/code.py`), so "has no content" was a fabricated
        // warning that failed `-W` on markup sphinx-build accepts.

        // Validate common options
        for (option, value) in &directive.options {
            match option.as_str() {
                // Flags: `force` was advertised by `valid_options` but had
                // no arm, so `.. code-block:: python` + `:force:` warned
                // "Unknown option" against an option Sphinx accepts.
                "linenos" | "force" => {
                    if !value.is_empty() {
                        return DirectiveValidationResult::Error(format!(
                            "{option} option should not have a value"
                        ));
                    }
                }
                "emphasize-lines" => {
                    // Could validate line numbers format here
                }
                // Value-carrying options. `lineno-start` is typed `int` in
                // sphinx (`code.py:112`), so `-3` and `0` are accepted there;
                // `dedent` is `optional_int`. The parse-time converter owns
                // every value diagnostic — a second opinion here can only
                // fabricate ("must be a positive integer" for a value
                // sphinx-build takes).
                "caption" | "name" | "dedent" | "class" | "lineno-start" => {}
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
        names(CODE_BLOCK_OPTIONS)
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
        // Note directive should have content (the parser routes directive-line
        // text into content, so a one-line `.. note:: text` passes here)
        if directive.content.trim().is_empty() {
            return DirectiveValidationResult::Error("Note directive requires content".to_string());
        }

        // Validate options
        for option in directive.options.keys() {
            match option.as_str() {
                "class" | "name" | "collapsible" => {
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
        names(ADMONITION_OPTIONS)
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
        // Warning directive should have content (directive-line text counts,
        // same as note)
        if directive.content.trim().is_empty() {
            return DirectiveValidationResult::Error(
                "Warning directive requires content".to_string(),
            );
        }

        // Validate options
        for option in directive.options.keys() {
            match option.as_str() {
                "class" | "name" | "collapsible" => {
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
        names(ADMONITION_OPTIONS)
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
                // `loading` (embed/link/lazy) is part of the docutils
                // image spec and was missing here, so `:loading: lazy`
                // warned "Unknown option" against valid markup.
                "alt" | "target" | "class" | "name" | "loading" => {
                    // Valid text options
                }
                "width" | "height" => {
                    if !is_valid_length(value) {
                        return DirectiveValidationResult::Warning(format!(
                            "{} is not a valid length: '{}'",
                            option, value
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
        names(IMAGE_OPTIONS)
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

        // Reuse image validation logic for the shared options. The
        // figure-only ones must be removed first: `ImageValidator` does
        // not know them, so they fell through to its catch-all and a plain
        // `.. figure:: x.png` + `:figwidth: image` warned "Unknown option
        // 'figwidth' for image directive" -- naming the wrong directive,
        // about an option this validator itself advertises.
        let image_validator = ImageValidator::new();
        let mut temp_directive = directive.clone();
        temp_directive.name = "image".to_string();
        for option in FIGURE_ONLY_OPTIONS {
            temp_directive.options.remove(*option);
        }
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
        names(FIGURE_OPTIONS)
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
                // `maxdepth` is typed `int` (`directives/other.py`,
                // `TocTree.option_spec`): `-1` is the documented "no limit"
                // spelling, and no depth is "too deep" to Sphinx. The
                // parse-time converter owns the value diagnostics; the
                // positive-integer and depth>10 checks that lived here were
                // fabricated warnings (same class as literalinclude's
                // `tab-width`, panel fix round B).
                "maxdepth" => {}
                // `numbered` is NOT a flag: Sphinx types it `int_or_nothing`
                // (`directives/other.py`, `TocTree.option_spec`), so
                // `:numbered: 2` -- the documented spelling for a numbering
                // depth -- is valid input. Warning on it fabricated a
                // diagnostic Sphinx never emits and failed `-W` on projects
                // sphinx 9.1.0 builds clean. Option handling for toctree
                // belongs to the parser's own table (`TOCTREE_OPTS`, which
                // has always had this right); nothing is re-checked here.
                "numbered" => {}
                "titlesonly" | "glob" | "reversed" | "hidden" | "includehidden" => {
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
        names(TOCTREE_OPTIONS)
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

        // No opinion on the target's spelling: docutils' `Include` opens
        // whatever path it is given (`<isonum.txt>` is a standard include,
        // `snippet.py` with `:literal:` is ordinary), and sphinx has no
        // extension check to mirror. The "Unusual file extension" warning
        // that lived here was fabricated (panel fix round B, [30]).

        DirectiveValidationResult::Valid
    }

    fn expected_arguments(&self) -> Vec<String> {
        vec!["filename".to_string()]
    }

    fn valid_options(&self) -> Vec<String> {
        names(INCLUDE_OPTIONS)
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

        // Option loop. NO value-range arms: `lineno-start` and `tab-width`
        // are typed plain `int` in sphinx (`code.py:112`, `:425-427`) — a
        // negative or zero value is accepted there — and `dedent` is
        // `optional_int`, whose own converter rejects a negative one at
        // parse time with sphinx's text. Every value diagnostic belongs to
        // the parse-time converter; the "must be a positive integer" arms
        // that lived here fabricated warnings sphinx-build never emits.
        for (option, value) in &directive.options {
            match option.as_str() {
                "language" | "start-after" | "end-before" | "prepend" | "append" | "caption"
                | "name" | "class" | "encoding" | "pyobject" | "diff" | "lineno-start"
                | "tab-width" | "dedent" => {
                    // Valid value-carrying options
                }
                "linenos" | "force" | "lineno-match" => {
                    // Flag options
                    if !value.is_empty() {
                        return DirectiveValidationResult::Warning(format!(
                            "{} option should not have a value",
                            option
                        ));
                    }
                }
                // Every other name the spec admits (`lines`,
                // `emphasize-lines`, `start-at`, `end-at`, …) carries a free
                // string this validator has no extra constraint for.
                // Consulting the shared const rather than a second literal
                // list is what keeps the two from drifting: they did, and a
                // plain `.. literalinclude:: f.py` + `:lines:` warned
                // "Unknown option 'lines'" against an option the very same
                // validator advertises as valid.
                _ if LITERALINCLUDE_OPTIONS.contains(&option.as_str()) => {}
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
        names(LITERALINCLUDE_OPTIONS)
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
        names(ADMONITION_OPTIONS)
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
        names(MATH_OPTIONS)
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

        // No language is valid Sphinx (falls back to highlight_language)
        let directive = create_test_directive(
            "code-block",
            vec![],
            HashMap::new(),
            "print('Hello, world!')",
        );
        assert_eq!(
            validator.validate(&directive),
            DirectiveValidationResult::Valid
        );

        // Bare numbers and all docutils units are valid lengths
        for width in ["100", "2cm", "50%", "1.5em", "12pt"] {
            let mut options = HashMap::new();
            options.insert("width".to_string(), width.to_string());
            let directive = create_test_directive("image", vec!["x.png".to_string()], options, "");
            assert_eq!(
                ImageValidator::new().validate(&directive),
                DirectiveValidationResult::Valid,
                "width '{width}' must be accepted"
            );
        }
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

    /// Every name `LiteralIncludeValidator::valid_options` advertises must
    /// actually validate. The two lists had drifted: `:lines:`,
    /// `:emphasize-lines:` and `:lineno-match:` — three of the directive's
    /// most common options, all present in the real option spec
    /// (`LITERALINCLUDE_OPTS`, src/rst/block.rs) — fell through to the
    /// catch-all and warned "Unknown option", a warning stream Sphinx has
    /// no counterpart for (found by the env-fixture inc_* projects).
    #[test]
    fn literalinclude_accepts_every_option_it_advertises() {
        let validator = LiteralIncludeValidator::new();

        for option in validator.valid_options() {
            // A value every constrained option accepts: the integer ones
            // parse it, the flags reject a non-empty value, the rest are
            // free strings.
            let value = if matches!(
                option.as_str(),
                "linenos" | "force" | "lineno-match" | "dedent"
            ) {
                String::new()
            } else {
                "1".to_string()
            };
            let mut options = HashMap::new();
            options.insert(option.clone(), value);
            let directive =
                create_test_directive("literalinclude", vec!["f.py".to_string()], options, "");
            assert_eq!(
                validator.validate(&directive),
                DirectiveValidationResult::Valid,
                "option {option:?} is advertised by valid_options but does not validate"
            );
        }

        // The catch-all still catches a name that really is not in the spec.
        let mut options = HashMap::new();
        options.insert("no-such-option".to_string(), String::new());
        let directive =
            create_test_directive("literalinclude", vec!["f.py".to_string()], options, "");
        assert_eq!(
            validator.validate(&directive),
            DirectiveValidationResult::Warning(
                "Unknown option 'no-such-option' for literalinclude directive".to_string()
            )
        );
    }

    /// Every registered validator, with a directive shaped so that
    /// validation actually reaches the option loop (arguments where the
    /// directive needs one, content where it requires one).
    fn every_validator() -> Vec<(Box<dyn DirectiveValidator>, Vec<String>, &'static str)> {
        let arg = |s: &str| vec![s.to_string()];
        vec![
            (
                Box::new(CodeBlockValidator::new()),
                arg("python"),
                "print(1)",
            ),
            (Box::new(NoteValidator::new()), vec![], "body"),
            (Box::new(WarningValidator::new()), vec![], "body"),
            (Box::new(ImageValidator::new()), arg("x.png"), ""),
            (Box::new(FigureValidator::new()), arg("x.png"), "caption"),
            (Box::new(TocTreeValidator::new()), vec![], "a\nb"),
            (Box::new(IncludeValidator::new()), arg("inc.rst"), ""),
            (Box::new(LiteralIncludeValidator::new()), arg("f.py"), ""),
            (Box::new(AdmonitionValidator::new()), arg("Title"), "body"),
            (Box::new(MathValidator::new()), vec![], "x = 1"),
        ]
    }

    /// THE DRIFT AUDIT (wave-4.5 task 16, generalizing task 14's finding).
    ///
    /// For every registered validator: each name it advertises must be
    /// ACCEPTED by its own `validate`. A validator whose `validate` match
    /// and `valid_options` disagree emits `Unknown option 'x'` for an
    /// option it simultaneously calls valid — a warning Sphinx has no
    /// counterpart for, which fails `-W` on a clean project.
    ///
    /// The assertion is about RECOGNITION, not about per-value
    /// constraints: an advertised option must never produce the
    /// `Unknown option '…'` catch-all, whatever value it carries. (A
    /// value-checking arm may still reject a specific value — `:align: 1`
    /// is an "Invalid alignment" error, and that is correct.) The value
    /// set includes a negative and a zero so an integer-typed option is
    /// exercised on the values sphinx's plain `int` converter accepts —
    /// the range where the fabricated "must be a positive integer" arms
    /// used to hide.
    #[test]
    fn every_validator_accepts_every_option_it_advertises() {
        for (validator, arguments, content) in every_validator() {
            for option in validator.valid_options() {
                for value in ["", "1", "left", "-1", "0"] {
                    let mut options = HashMap::new();
                    options.insert(option.clone(), value.to_string());
                    let directive = create_test_directive(
                        validator.name(),
                        arguments.clone(),
                        options,
                        content,
                    );
                    if let DirectiveValidationResult::Warning(message)
                    | DirectiveValidationResult::Error(message) = validator.validate(&directive)
                    {
                        assert!(
                            !message.starts_with(&format!("Unknown option '{option}'")),
                            "{}: option {option:?} is advertised by valid_options \
                             but its validate() calls it unknown (value {value:?})",
                            validator.name()
                        );
                    }
                }
            }
        }
    }

    /// The other half of the audit: each validator's advertised list must
    /// equal the directive's parse-time `option_spec`
    /// (`directive_option_names`, src/rst/block.rs), which is this crate's
    /// probe-verified transcription of the real docutils/sphinx spec.
    ///
    /// Both directions matter. An option in the spec but not the list is a
    /// fabricated `Unknown option` warning waiting to happen (this caught
    /// `code-block`'s `class`, `image`/`figure`'s `loading`, and
    /// `include`'s `parser`/`class`/`name`). An option in the list but not
    /// the spec is a name the validator blesses and the parser then
    /// rejects — which is what `literalinclude`'s `start-line`/`end-line`
    /// were, borrowed from docutils' `include`, where they do exist.
    #[test]
    fn validator_option_lists_match_the_parser_spec() {
        use std::collections::BTreeSet;
        for (validator, _, _) in every_validator() {
            let name = validator.name();
            let spec: BTreeSet<String> = crate::rst::block::directive_option_names(name)
                .unwrap_or_else(|| panic!("{name}: no parse-time directive spec"))
                .into_iter()
                .map(str::to_string)
                .collect();
            let advertised: BTreeSet<String> = validator.valid_options().into_iter().collect();
            assert_eq!(
                advertised,
                spec,
                "{name}: valid_options and the parser's option_spec disagree.\n  \
                 advertised but not in the spec: {:?}\n  \
                 in the spec but not advertised: {:?}",
                advertised.difference(&spec).collect::<Vec<_>>(),
                spec.difference(&advertised).collect::<Vec<_>>(),
            );
        }
    }

    /// `literalinclude` has no `:start-line:`/`:end-line:` — those belong
    /// to docutils' `include`. Pinned in both directions so the removal
    /// cannot be undone by copy-paste from the sibling validator.
    #[test]
    fn start_line_and_end_line_are_include_only() {
        for option in ["start-line", "end-line"] {
            assert!(
                IncludeValidator::new()
                    .valid_options()
                    .contains(&option.to_string()),
                "include must still advertise {option:?}"
            );
            assert!(
                !LiteralIncludeValidator::new()
                    .valid_options()
                    .contains(&option.to_string()),
                "literalinclude must not advertise {option:?}: sphinx 9.1.0's \
                 LiteralInclude.option_spec has no such key"
            );
            let mut options = HashMap::new();
            options.insert(option.to_string(), "2".to_string());
            let directive =
                create_test_directive("literalinclude", vec!["f.py".to_string()], options, "");
            assert_eq!(
                LiteralIncludeValidator::new().validate(&directive),
                DirectiveValidationResult::Warning(format!(
                    "Unknown option '{option}' for literalinclude directive"
                ))
            );
        }
    }
    /// Panel fix round B, [17]/[31]: the integer-typed options accept
    /// whatever sphinx's converters accept. `lineno-start`/`tab-width` are
    /// plain `int` (negative and zero included), `maxdepth` is `int` with
    /// `-1` as the documented "unlimited", `dedent` is `optional_int`
    /// whose diagnostics are the parse-time converter's business. A
    /// clean sphinx project must never earn a validation warning here.
    #[test]
    fn integer_options_accept_the_values_sphinxs_converters_accept() {
        let cases: &[(&str, Vec<String>, &str, &str, &str)] = &[
            (
                "literalinclude",
                vec!["f.py".to_string()],
                "",
                "tab-width",
                "-1",
            ),
            (
                "literalinclude",
                vec!["f.py".to_string()],
                "",
                "lineno-start",
                "-3",
            ),
            (
                "literalinclude",
                vec!["f.py".to_string()],
                "",
                "lineno-start",
                "0",
            ),
            (
                "literalinclude",
                vec!["f.py".to_string()],
                "",
                "dedent",
                "-2",
            ),
            ("literalinclude", vec!["f.py".to_string()], "", "dedent", ""),
            (
                "code-block",
                vec!["python".to_string()],
                "x = 1",
                "lineno-start",
                "-3",
            ),
            (
                "code-block",
                vec!["python".to_string()],
                "x = 1",
                "lineno-start",
                "0",
            ),
            ("code-block", vec![], "x = 1", "dedent", "-2"),
            ("toctree", vec![], "a\nb", "maxdepth", "-1"),
            ("toctree", vec![], "a\nb", "maxdepth", "99"),
        ];
        let registry = crate::directives::validation::DirectiveRegistry::with_builtin_validators();
        for (name, arguments, content, option, value) in cases {
            let mut options = HashMap::new();
            options.insert((*option).to_string(), (*value).to_string());
            let directive = create_test_directive(name, arguments.clone(), options, content);
            assert_eq!(
                registry.validate_directive(&directive),
                DirectiveValidationResult::Valid,
                "{name} :{option}: {value:?} is accepted by sphinx-build"
            );
        }
    }

    /// Panel fix round B, [30]: docutils' `include` opens any path — a
    /// standard include (`<isonum.txt>`), a `.py` shown with `:literal:`,
    /// an extension-less file — and sphinx has no extension check, so the
    /// old "Unusual file extension" warning fabricated a diagnostic.
    #[test]
    fn include_has_no_opinion_on_the_targets_extension() {
        let registry = crate::directives::validation::DirectiveRegistry::with_builtin_validators();
        for (target, option) in [
            ("<isonum.txt>", None),
            ("snippet.py", Some("literal")),
            ("snippet.py", None),
            ("NOTES", None),
            ("data.csv", Some("code")),
        ] {
            let mut options = HashMap::new();
            if let Some(option) = option {
                options.insert(option.to_string(), String::new());
            }
            let directive = create_test_directive("include", vec![target.to_string()], options, "");
            assert_eq!(
                registry.validate_directive(&directive),
                DirectiveValidationResult::Valid,
                ".. include:: {target}"
            );
        }
    }

    /// Panel fix round B, [17]: an empty `code-block` is legal sphinx
    /// (`CodeBlock.run` builds an empty `literal_block` and says nothing),
    /// so it is not a validation finding either.
    #[test]
    fn an_empty_code_block_is_not_a_finding() {
        let registry = crate::directives::validation::DirectiveRegistry::with_builtin_validators();
        for arguments in [vec![], vec!["python".to_string()]] {
            let directive = create_test_directive("code-block", arguments, HashMap::new(), "");
            assert_eq!(
                registry.validate_directive(&directive),
                DirectiveValidationResult::Valid
            );
        }
    }
}
