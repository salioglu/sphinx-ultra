//! Built-in directive validators for common Sphinx directives
//!
//! ## No checks (decision D1, wave 5 sub-project 1)
//!
//! Every validator here is silent: `validate` returns `Valid` for any
//! directive. The checks they used to make fell into two classes, and both
//! are gone.
//!
//! * **docutils or Sphinx already reports it.** A missing argument or
//!   content block, an unknown option, a flag given a value, an image
//!   `width`/`height`/`scale`/`align` the option converter rejects: docutils
//!   raises each as a `DirectiveError` while parsing and the parser
//!   records it on the reporter channel (`crate::rst::diagnostics`) -- the
//!   message `sphinx-build` prints as `file:line: ERROR: ... [docutils]`. A
//!   validator message for the same mistake was a second line in words
//!   Sphinx never uses.
//! * **No Sphinx counterpart, and it fired on markup `sphinx-build`
//!   accepts.** `image`/`figure`'s "unusual extension" (every remote URL
//!   tripped it), an empty `toctree`, an empty `math`, unbalanced braces in
//!   `math` (`\left\{ x \right.` is balanced LaTeX).
//!
//! The two classes are exhaustive: a check Sphinx does not report can only
//! fire on markup Sphinx accepts silently, so none survives the audit. Each
//! validator's `<name>_is_silent_*` tests below pin the markup that drew its
//! removed checks -- the docutils half against the parse's own reporter
//! channel, the accepted half against a probe of Sphinx 9.1.0.
//!
//! What remains is the framework (the registry, the statistics, the build's
//! `validate_directives` pass) and each validator's metadata
//! (`valid_options`, `expected_arguments`, ...), which the default
//! [`DirectiveValidator::get_suggestions`] reads.
//!
//! ## Option lists
//!
//! Each validator's option list is spelled ONCE, as a shared `&[&str]`
//! const. A validator that spells its options twice drifts: wave 4.5 found
//! `Unknown option 'lines'` warned against an option the very same
//! validator advertised, and the same shape in `code-block` (`force`),
//! `figure` (`figwidth`/`figclass`) and `image` (`loading`).
//!
//! Every list mirrors the directive's parse-time `option_spec` in
//! `src/rst/block.rs`, which is this crate's probe-verified transcription
//! of the real docutils/sphinx spec. The test
//! `validator_option_lists_match_the_parser_spec` holds the two together in
//! BOTH directions.

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

/// `ADMONITION_OPTS` — shared by `note`, `warning` and `admonition`.
const ADMONITION_OPTIONS: &[&str] = &["class", "name"];

/// `IMAGE_OPTS` (`DU/parsers/rst/directives/images.py` Image.option_spec).
const IMAGE_OPTIONS: &[&str] = &[
    "alt", "height", "width", "scale", "align", "target", "loading", "class", "name",
];

/// `FIGURE_OPTS`: the image set plus the three figure-only options.
const FIGURE_OPTIONS: &[&str] = &[
    "alt", "height", "width", "scale", "align", "target", "loading", "class", "name", "figwidth",
    "figclass", "figname",
];

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

    fn validate(&self, _directive: &ParsedDirective) -> DirectiveValidationResult {
        // Silent (D1, see the module docs): `:linenos:`/`:force:` given a
        // value and an unknown option are docutils `Error in "code-block"
        // directive` messages; a missing language or an empty body is valid
        // Sphinx (`CodeBlock.run`, `directives/code.py`); the value
        // converters (`lineno-start` is plain `int`, `dedent` is
        // `optional_int`) own their own diagnostics at parse time.
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

    fn validate(&self, _directive: &ParsedDirective) -> DirectiveValidationResult {
        // Silent (D1, see the module docs): an empty body is docutils'
        // `Content block expected for the "note" directive; none found.`
        // and an unknown option its `unknown option` error.
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

    fn validate(&self, _directive: &ParsedDirective) -> DirectiveValidationResult {
        // Silent (D1, see the module docs): the same two docutils errors
        // as `note`.
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

    fn validate(&self, _directive: &ParsedDirective) -> DirectiveValidationResult {
        // Silent (D1, see the module docs): a missing argument, an
        // invalid `width`/`height`/`scale`/`align` and an unknown option
        // are docutils `Error in "image" directive` messages. The
        // "unusual extension" check had no counterpart and judged the
        // target by `split('.').last()`, so a local `.ico`, an
        // extension-less file and every remote URL tripped it.
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

    fn validate(&self, _directive: &ParsedDirective) -> DirectiveValidationResult {
        // Silent (D1, see the module docs): `figure` shares the image
        // option spec, so docutils reports the same `Error in "figure"
        // directive` messages, and the extension heuristic it borrowed from
        // `image` fired on the same accepted markup.
        DirectiveValidationResult::Valid
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

    fn validate(&self, _directive: &ParsedDirective) -> DirectiveValidationResult {
        // Silent (D1, see the module docs): a flag given a value and an
        // unknown option are docutils `Error in "toctree" directive`
        // messages; an empty toctree is accepted without a word (`TocTree`
        // never asserts content), so "Toctree directive is empty" had no
        // counterpart. `maxdepth` and `numbered` take typed values whose
        // converters report at parse time.
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

    fn validate(&self, _directive: &ParsedDirective) -> DirectiveValidationResult {
        // Silent (D1, see the module docs): a missing path is docutils'
        // `1 argument(s) required, 0 supplied.`; docutils opens whatever
        // path it is given (`<isonum.txt>`, a `.py` under `:literal:`), and
        // Sphinx has no extension check to mirror.
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

    fn validate(&self, _directive: &ParsedDirective) -> DirectiveValidationResult {
        // Silent (D1, see the module docs): a missing path, a flag given a
        // value and an unknown option are docutils errors. `lineno-start`
        // and `tab-width` are plain `int` (`code.py`), negative and zero
        // included, and `dedent` is `optional_int`: their converters own
        // every value diagnostic at parse time.
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

    fn validate(&self, _directive: &ParsedDirective) -> DirectiveValidationResult {
        // Silent (D1, see the module docs): a missing title is docutils'
        // `1 argument(s) required, 0 supplied.` and an empty body its
        // `Content block expected for the "admonition" directive; none
        // found.`
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

    fn validate(&self, _directive: &ParsedDirective) -> DirectiveValidationResult {
        // Silent (D1, see the module docs): Sphinx's `MathDirective`
        // (`patches.py`) asserts no content and never reads the LaTeX, so
        // an empty `.. math::` and braces the counter could not balance
        // (`\left\{ x \right.`) both build clean.
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
    use crate::directives::validation::audit_support::{
        assert_directive_silent, assert_docutils_reports,
    };
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

    /// Every registered built-in directive validator.
    fn every_validator() -> Vec<Box<dyn DirectiveValidator>> {
        vec![
            Box::new(CodeBlockValidator::new()),
            Box::new(NoteValidator::new()),
            Box::new(WarningValidator::new()),
            Box::new(ImageValidator::new()),
            Box::new(FigureValidator::new()),
            Box::new(TocTreeValidator::new()),
            Box::new(IncludeValidator::new()),
            Box::new(LiteralIncludeValidator::new()),
            Box::new(AdmonitionValidator::new()),
            Box::new(MathValidator::new()),
        ]
    }

    /// Each validator's advertised list (what the default `get_suggestions`
    /// calls valid) must equal the directive's parse-time `option_spec`
    /// (`directive_option_names`, src/rst/block.rs), which is this crate's
    /// probe-verified transcription of the real docutils/sphinx spec.
    ///
    /// Both directions matter. An option in the spec but not the list is
    /// advice that a valid option is unknown (this caught `code-block`'s
    /// `class`, `image`/`figure`'s `loading`, and `include`'s
    /// `parser`/`class`/`name`). An option in the list but not the spec is
    /// a name the validator blesses and the parser then rejects -- which is
    /// what `literalinclude`'s `start-line`/`end-line` were, borrowed from
    /// docutils' `include`, where they do exist.
    #[test]
    fn validator_option_lists_match_the_parser_spec() {
        use std::collections::BTreeSet;
        for validator in every_validator() {
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
    // -----------------------------------------------------------------
    // D1 audit (wave 5, sub-project 1). Once docutils' own messages print,
    // a validator check that restates one of them -- or that has no Sphinx
    // counterpart and fires on markup sphinx-build accepts -- is a
    // double-report or a fabrication. Every claim below is pinned two ways:
    // the parse's reporter channel shows docutils saying it (the feed is the
    // build's own, see `audit_support`), and the validator stays silent.
    // The "accepted" claims were probed on Sphinx 9.1.0 / docutils 0.22.4
    // (scratchpad probe-task5, the case id is named per test): a project
    // holding the markup builds with no warning at all.
    // -----------------------------------------------------------------

    /// docutils' `flag` converter on a valued option (`directives.flag`).
    fn flag_error(directive: &str, option: &str) -> String {
        format!(
            "Error in \"{directive}\" directive:\ninvalid option value: (option: \"{option}\"; \
             value: 'yes')\nno argument is allowed; \"yes\" supplied."
        )
    }

    /// docutils' `unknown option` message (`states.py` `parse_extension_options`).
    fn unknown_option_error(directive: &str) -> String {
        format!("Error in \"{directive}\" directive:\nunknown option: \"bogus\".")
    }

    /// docutils' missing-argument message (`states.py` `parse_directive_arguments`).
    fn arguments_error(directive: &str) -> String {
        format!("Error in \"{directive}\" directive:\n1 argument(s) required, 0 supplied.")
    }

    /// docutils' `assert_has_content` message.
    fn content_error(directive: &str) -> String {
        format!("Content block expected for the \"{directive}\" directive; none found.")
    }

    /// Probe cases `cb_flag_value`, `cb_force_value`, `cb_unknown_opt`: the
    /// `linenos`/`force` flags with a value and an unknown option are
    /// docutils `ERROR`s at parse time.
    #[test]
    fn code_block_is_silent_where_docutils_already_reports() {
        let validator = CodeBlockValidator::new();
        for option in ["linenos", "force"] {
            let source = format!(".. code-block:: python\n   :{option}: yes\n\n   x = 1\n");
            assert_docutils_reports(&source, &flag_error("code-block", option));
            assert_directive_silent(&validator, &source);
        }
        let source = ".. code-block:: python\n   :bogus:\n\n   x = 1\n";
        assert_docutils_reports(source, &unknown_option_error("code-block"));
        assert_directive_silent(&validator, source);
    }

    /// Probe cases `note_empty`, `note_unknown_opt`: `assert_has_content`
    /// and the option parser both raise docutils errors.
    #[test]
    fn note_is_silent_where_docutils_already_reports() {
        let validator = NoteValidator::new();
        let empty = ".. note::\n";
        assert_docutils_reports(empty, &content_error("note"));
        assert_directive_silent(&validator, empty);
        let unknown = ".. note::\n   :bogus: x\n\n   Body.\n";
        assert_docutils_reports(unknown, &unknown_option_error("note"));
        assert_directive_silent(&validator, unknown);
    }

    /// Probe cases `warning_empty`, `warning_unknown_opt`.
    #[test]
    fn warning_is_silent_where_docutils_already_reports() {
        let validator = WarningValidator::new();
        let empty = ".. warning::\n";
        assert_docutils_reports(empty, &content_error("warning"));
        assert_directive_silent(&validator, empty);
        let unknown = ".. warning::\n   :bogus: x\n\n   Body.\n";
        assert_docutils_reports(unknown, &unknown_option_error("warning"));
        assert_directive_silent(&validator, unknown);
    }

    /// The `image` option-value and argument failures are all docutils
    /// errors (probe cases `image_noarg`, `image_width_bad`,
    /// `image_height_bad`, `image_scale_bad`, `image_align_bad`,
    /// `image_unknown_opt`).
    #[test]
    fn image_is_silent_where_docutils_already_reports() {
        let value_error = |option: &str, detail: &str| {
            format!(
                "Error in \"image\" directive:\ninvalid option value: (option: \"{option}\"; \
                 value: 'bogus')\n{detail}"
            )
        };
        let cases = [
            (".. image::\n", arguments_error("image")),
            (
                ".. image:: x.png\n   :width: bogus\n",
                value_error("width", "\"bogus\" is no valid measure.."),
            ),
            (
                ".. image:: x.png\n   :height: bogus\n",
                value_error("height", "\"bogus\" is no valid measure.."),
            ),
            (
                ".. image:: x.png\n   :scale: bogus\n",
                value_error("scale", "invalid literal for int() with base 10: 'bogus'."),
            ),
            (
                ".. image:: x.png\n   :align: bogus\n",
                value_error(
                    "align",
                    "\"bogus\" unknown; choose from \"top\", \"middle\", \"bottom\", \"left\", \
                     \"center\", or \"right\".",
                ),
            ),
            (
                ".. image:: x.png\n   :bogus: 1\n",
                unknown_option_error("image"),
            ),
        ];
        for (source, reported) in cases {
            assert_docutils_reports(source, &reported);
            assert_directive_silent(&ImageValidator::new(), source);
        }
    }

    /// The old "Unusual image extension" warning has no Sphinx counterpart
    /// and judged the target by `split('.').last()`, so it fired on a local
    /// `.ico`, an extension-less file, and every remote URL (`com/badge/logo`
    /// was "the extension" of `https://example.com/badge/logo`). Probe cases
    /// `image_ico_local`, `image_noext_local`, `image_remote_noext`,
    /// `image_remote_host_dots`: sphinx-build builds each with no warning.
    #[test]
    fn image_is_silent_on_markup_sphinx_accepts() {
        for source in [
            ".. image:: favicon.ico\n",
            ".. image:: _static/logo\n",
            ".. image:: https://example.com/badge/logo\n",
            ".. image:: https://img.shields.io/badge/a-b-green\n",
        ] {
            assert_directive_silent(&ImageValidator::new(), source);
        }
    }

    /// Probe cases `figure_noarg`, `figure_width_bad`, `figure_unknown_opt`.
    #[test]
    fn figure_is_silent_where_docutils_already_reports() {
        let cases = [
            (".. figure::\n\n   caption\n", arguments_error("figure")),
            (
                ".. figure:: x.png\n   :width: bogus\n\n   cap\n",
                "Error in \"figure\" directive:\ninvalid option value: (option: \"width\"; \
                 value: 'bogus')\n\"bogus\" is no valid measure.."
                    .to_string(),
            ),
            (
                ".. figure:: x.png\n   :bogus: 1\n\n   cap\n",
                unknown_option_error("figure"),
            ),
        ];
        for (source, reported) in cases {
            assert_docutils_reports(source, &reported);
            assert_directive_silent(&FigureValidator::new(), source);
        }
    }

    /// Probe case `figure_ico`, and the remote form from `accepted_all`:
    /// the figure borrowed the image extension heuristic.
    #[test]
    fn figure_is_silent_on_markup_sphinx_accepts() {
        for source in [
            ".. figure:: favicon.ico\n\n   cap\n",
            ".. figure:: https://example.com/badge/logo\n\n   cap\n",
        ] {
            assert_directive_silent(&FigureValidator::new(), source);
        }
    }

    /// Probe cases `toctree_flag_value`, `toctree_titlesonly_value`,
    /// `toctree_unknown_opt`: every flag option (`directives.flag`) and the
    /// unknown-option case are docutils errors.
    #[test]
    fn toctree_is_silent_where_docutils_already_reports() {
        let validator = TocTreeValidator::new();
        for option in ["titlesonly", "glob", "reversed", "hidden", "includehidden"] {
            let source = format!(".. toctree::\n   :{option}: yes\n\n   other\n");
            assert_docutils_reports(&source, &flag_error("toctree", option));
            assert_directive_silent(&validator, &source);
        }
        let source = ".. toctree::\n   :bogus:\n\n   other\n";
        assert_docutils_reports(source, &unknown_option_error("toctree"));
        assert_directive_silent(&validator, source);
    }

    /// Probe cases `toctree_empty`, `toctree_empty_opts`: an empty toctree
    /// is accepted silently (`TocTree` never calls `assert_has_content`),
    /// so "Toctree directive is empty" had no counterpart at all.
    #[test]
    fn toctree_is_silent_on_markup_sphinx_accepts() {
        for source in [".. toctree::\n", ".. toctree::\n   :maxdepth: 2\n"] {
            assert_directive_silent(&TocTreeValidator::new(), source);
        }
    }

    /// Probe case `include_noarg`.
    #[test]
    fn include_is_silent_where_docutils_already_reports() {
        let source = ".. include::\n";
        assert_docutils_reports(source, &arguments_error("include"));
        assert_directive_silent(&IncludeValidator::new(), source);
    }

    /// Probe cases `li_noarg`, `li_flag_value`, `li_unknown_opt`.
    #[test]
    fn literalinclude_is_silent_where_docutils_already_reports() {
        let validator = LiteralIncludeValidator::new();
        let source = ".. literalinclude::\n";
        assert_docutils_reports(source, &arguments_error("literalinclude"));
        assert_directive_silent(&validator, source);
        for option in ["linenos", "force", "lineno-match"] {
            let source = format!(".. literalinclude:: f.py\n   :{option}: yes\n");
            assert_docutils_reports(&source, &flag_error("literalinclude", option));
            assert_directive_silent(&validator, &source);
        }
        let source = ".. literalinclude:: f.py\n   :bogus: 1\n";
        assert_docutils_reports(source, &unknown_option_error("literalinclude"));
        assert_directive_silent(&validator, source);
    }

    /// Probe cases `adm_noarg`, `adm_nocontent`.
    #[test]
    fn admonition_is_silent_where_docutils_already_reports() {
        let validator = AdmonitionValidator::new();
        let no_title = ".. admonition::\n\n   body\n";
        assert_docutils_reports(no_title, &arguments_error("admonition"));
        assert_directive_silent(&validator, no_title);
        let no_body = ".. admonition:: Title\n";
        assert_docutils_reports(no_body, &content_error("admonition"));
        assert_directive_silent(&validator, no_body);
    }

    /// Probe cases `math_empty`, `math_unbalanced_left_brace`: Sphinx's
    /// `MathDirective` (`patches.py`) does not assert content and never
    /// looks at the LaTeX, so an empty `.. math::` and `\left\{ x \right.`
    /// -- balanced LaTeX whose `\{` the brace counter counted as a bare
    /// opening brace -- both build with no warning.
    #[test]
    fn math_is_silent_on_markup_sphinx_accepts() {
        for source in [".. math::\n", ".. math::\n\n   \\left\\{ x \\right.\n"] {
            assert_directive_silent(&MathValidator::new(), source);
        }
    }
}
