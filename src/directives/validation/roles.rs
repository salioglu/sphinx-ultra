//! Built-in role validators for common Sphinx roles
//!
//! ## No checks (decision D1, wave 5 sub-project 1)
//!
//! Every validator here is silent: `validate` returns `Valid` for any role.
//! Each one made one of two kinds of check, and both are gone (the same
//! audit as the directive validators' in `builtin.rs`, whose module docs
//! give the reasoning).
//!
//! * **An empty target.** It only reaches a validator through `<>` --
//!   `:kbd:`x <>`` splits into target `""` and display `"x"` -- and only the
//!   cross-reference roles (`doc`, `ref`, `download`) do anything with a
//!   target: for those Sphinx reports it itself (`unknown document: ''`,
//!   `undefined label: ''`, a failed download copy). For every other role
//!   `<>` is ordinary text and `sphinx-build` accepts it.
//! * **House-style heuristics with no Sphinx counterpart** -- `doc`'s file
//!   extension, `download`'s extension whitelist and URL check, `math`'s
//!   brace count, `abbr`'s capitals, `command`'s shell characters, `file`'s
//!   path characters, `guilabel`'s `&` -- each of which fired on markup
//!   `sphinx-build` builds with no warning.
//!
//! Each validator's `<name>_is_silent_*` tests pin the markup that drew its
//! removed checks; the Sphinx claims were probed on Sphinx 9.1.0.

use super::{ParsedRole, RoleValidationResult, RoleValidator};

/// Validator for doc role
#[derive(Default)]
pub struct DocRoleValidator;

impl DocRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for DocRoleValidator {
    fn name(&self) -> &str {
        "doc"
    }

    fn validate(&self, _role: &ParsedRole) -> RoleValidationResult {
        // Silent (D1, see the module docs): a `:doc:` whose target is not
        // a document (an extension, an empty `<>`) is Sphinx's own
        // `unknown document: '...' [ref.doc]`, which the build prints; and
        // a project whose documents really are `notes.rst.rst` resolves
        // `:doc:`notes.rst``, which the extension check warned on anyway.
        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        true
    }
}

/// Validator for ref role
#[derive(Default)]
pub struct RefRoleValidator;

impl RefRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for RefRoleValidator {
    fn name(&self) -> &str {
        "ref"
    }

    fn validate(&self, _role: &ParsedRole) -> RoleValidationResult {
        // Silent (D1, see the module docs): an empty target is Sphinx's own
        // `undefined label: '' [ref.ref]`. Labels may hold spaces and
        // capitals (`.. _My Label:`), so there is no format to police.
        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        true
    }
}

/// Validator for download role
#[derive(Default)]
pub struct DownloadRoleValidator;

impl DownloadRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for DownloadRoleValidator {
    fn name(&self) -> &str {
        "download"
    }

    fn validate(&self, _role: &ParsedRole) -> RoleValidationResult {
        // Silent (D1, see the module docs): an empty target earns Sphinx's
        // own download-copy warning; any extension is downloadable, and a
        // `://` target is a link Sphinx keeps as it is
        // (`DownloadFileCollector.process_doc`), so the extension whitelist
        // and the "no URLs" check fired on accepted markup.
        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        true
    }
}

/// Validator for math role
#[derive(Default)]
pub struct MathRoleValidator;

impl MathRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for MathRoleValidator {
    fn name(&self) -> &str {
        "math"
    }

    fn validate(&self, _role: &ParsedRole) -> RoleValidationResult {
        // Silent (D1, see the module docs): Sphinx never reads the LaTeX of
        // a `:math:` role, `\{` made valid LaTeX look unbalanced, and
        // `<>` is ordinary text to a non-cross-reference role.
        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        false
    }
}

/// Validator for abbreviation role
#[derive(Default)]
pub struct AbbreviationRoleValidator;

impl AbbreviationRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for AbbreviationRoleValidator {
    fn name(&self) -> &str {
        "abbr"
    }

    fn validate(&self, _role: &ParsedRole) -> RoleValidationResult {
        // Silent (D1, see the module docs): `:abbr:`rst (reStructuredText)``
        // is the documented form; "abbreviations contain uppercase" was the
        // validator's own taste.
        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        true
    }
}

/// Validator for command role
#[derive(Default)]
pub struct CommandRoleValidator;

impl CommandRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for CommandRoleValidator {
    fn name(&self) -> &str {
        "command"
    }

    fn validate(&self, _role: &ParsedRole) -> RoleValidationResult {
        // Silent (D1, see the module docs): `:command:` renders its text as
        // a literal; a pipeline in it is documentation, not an injection.
        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        false
    }
}

/// Validator for file role
#[derive(Default)]
pub struct FileRoleValidator;

impl FileRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for FileRoleValidator {
    fn name(&self) -> &str {
        "file"
    }

    fn validate(&self, _role: &ParsedRole) -> RoleValidationResult {
        // Silent (D1, see the module docs): `:file:` text is prose, not a
        // path the build opens (`C:\Windows`, `a?b*c` are fine).
        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        false
    }
}

/// Validator for kbd role
#[derive(Default)]
pub struct KbdRoleValidator;

impl KbdRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for KbdRoleValidator {
    fn name(&self) -> &str {
        "kbd"
    }

    fn validate(&self, _role: &ParsedRole) -> RoleValidationResult {
        // Silent (D1, see the module docs): any key name is legal
        // (`:kbd:`Cmd``, `:kbd:`PgUp``), and `<>` is just text.
        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        false
    }
}

/// Validator for menuselection role
#[derive(Default)]
pub struct MenuSelectionRoleValidator;

impl MenuSelectionRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for MenuSelectionRoleValidator {
    fn name(&self) -> &str {
        "menuselection"
    }

    fn validate(&self, _role: &ParsedRole) -> RoleValidationResult {
        // Silent (D1, see the module docs): a single menu item needs no
        // separator, and `<>` is just text.
        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        false
    }
}

/// Validator for guilabel role
#[derive(Default)]
pub struct GuiLabelRoleValidator;

impl GuiLabelRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for GuiLabelRoleValidator {
    fn name(&self) -> &str {
        "guilabel"
    }

    fn validate(&self, _role: &ParsedRole) -> RoleValidationResult {
        // Silent (D1, see the module docs): a lone `&` is the documented
        // access-key marker (`:guilabel:`&File``, `GUILabel.amp_re` in
        // `sphinx/roles.py`) and `&&` the literal ampersand; `&amp;` is
        // not what Sphinx asks for.
        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directives::validation::audit_support::assert_role_silent;

    // -----------------------------------------------------------------
    // D1 audit (wave 5, sub-project 1): see the note above the directive
    // audit in `builtin.rs`. The feed is the build's own (`audit_support`:
    // the real parser's role records). Probed on Sphinx 9.1.0 / docutils
    // 0.22.4 (scratchpad probe-task5; the case id is named per test).
    //
    // Every role validator here checked one of two things: that the target
    // is non-empty, or a house-style heuristic. An empty target only
    // reaches a validator through `<>` -- `:kbd:`x <>``, whose record splits
    // into target "" and display "x" -- and only the cross-reference roles
    // (`doc`, `ref`, `download`) do anything with the target, so only they
    // earn a Sphinx message for it. For every other role `<>` is just text.
    // -----------------------------------------------------------------

    /// Probe cases `doc_ext_missing`, `doc_ext_md_missing`,
    /// `doc_empty_target`: a `:doc:` whose target is not a document is
    /// Sphinx's `unknown document: 'other.rst' [ref.doc]` (a `warn_dangling`
    /// role, reported without `-n`), and the build already prints it
    /// byte-for-byte -- the extension check only said it a second time, in
    /// other words.
    #[test]
    fn doc_role_is_silent_where_docutils_already_reports() {
        for source in [
            ":doc:`other.rst`",
            ":doc:`other.md`",
            ":doc:`Title <other.rst>`",
            ":doc:`Title <>`",
        ] {
            assert_role_silent(&DocRoleValidator::new(), source);
        }
    }

    /// Probe case `doc_ext_real_docname` (and `accepted_all`): a project
    /// whose documents are `notes.rst.rst` and `notes.md.rst` has the
    /// docnames `notes.rst` and `notes.md`, and `:doc:`notes.rst`` resolves
    /// -- Sphinx builds it with no warning. The extension check could not
    /// tell, because it never looked at the project.
    #[test]
    fn doc_role_is_silent_on_markup_sphinx_accepts() {
        assert_role_silent(
            &DocRoleValidator::new(),
            ":doc:`notes.rst` and :doc:`notes.md`",
        );
    }

    /// Probe case `ref_empty_target`: `undefined label: '' [ref.ref]`.
    #[test]
    fn ref_role_is_silent_where_docutils_already_reports() {
        assert_role_silent(&RefRoleValidator::new(), ":ref:`Title <>`");
    }

    /// Probe case `download_empty_target`: Sphinx's own `cannot copy
    /// downloadable file` warning fires for an empty target.
    #[test]
    fn download_role_is_silent_where_docutils_already_reports() {
        assert_role_silent(&DownloadRoleValidator::new(), ":download:`Title <>`");
    }

    /// Probe cases `download_whl`, `download_noext`, `download_url`,
    /// `download_url_bare`: `:download:` takes any file whatever its
    /// extension, and a `://` target is a link Sphinx keeps as it is
    /// (`DownloadFileCollector.process_doc`, `environment/collectors/
    /// asset.py`) -- no warning for any of them. The old extension
    /// whitelist and the "reference local files, not URLs" check
    /// contradicted that.
    #[test]
    fn download_role_is_silent_on_markup_sphinx_accepts() {
        for source in [
            ":download:`pkg <pkg-1.0-py3-none-any.whl>`",
            ":download:`license <LICENSE>`",
            ":download:`x <https://example.com/file.pdf>`",
            ":download:`https://example.com/file.pdf`",
        ] {
            assert_role_silent(&DownloadRoleValidator::new(), source);
        }
    }

    /// Probe cases `math_role_left_brace`, `math_role_unbalanced`,
    /// `math_role_angle`: Sphinx never inspects the LaTeX of a `:math:`
    /// role (the brace counter also counted `\{` as an opening brace, so
    /// valid LaTeX tripped it), and `<>` is just text to it.
    #[test]
    fn math_role_is_silent_on_markup_sphinx_accepts() {
        for source in [
            ":math:`\\left\\{ x \\right.`",
            ":math:`\\frac{a}{b`",
            ":math:`a <>`",
        ] {
            assert_role_silent(&MathRoleValidator::new(), source);
        }
    }

    /// Probe cases `abbr_lowercase`, `abbr_angle`: `:abbr:`rst
    /// (reStructuredText)`` is the documented form and "abbreviations
    /// typically contain uppercase letters" is the validator's own opinion.
    #[test]
    fn abbreviation_role_is_silent_on_markup_sphinx_accepts() {
        for source in [
            ":abbr:`rst (reStructuredText)`",
            ":abbr:`etc`",
            ":abbr:`x <>`",
        ] {
            assert_role_silent(&AbbreviationRoleValidator::new(), source);
        }
    }

    /// Probe cases `command_pipe`, `command_angle`: `:command:` renders its
    /// text as a literal; a shell pipeline in it is ordinary documentation.
    #[test]
    fn command_role_is_silent_on_markup_sphinx_accepts() {
        for source in [
            ":command:`ls | grep foo`",
            ":command:`a && b; c`",
            ":command:`x <>`",
        ] {
            assert_role_silent(&CommandRoleValidator::new(), source);
        }
    }

    /// Probe cases `file_colon`, `file_angle`: `C:\Windows` and `a?b*c`
    /// are fine `:file:` text -- it is prose, not a path the build opens.
    #[test]
    fn file_role_is_silent_on_markup_sphinx_accepts() {
        for source in [
            ":file:`C:\\Windows`",
            ":file:`a?b*c`",
            ":file:`/dir/<name>/x`",
            ":file:`x <>`",
        ] {
            assert_role_silent(&FileRoleValidator::new(), source);
        }
    }

    /// Probe case `kbd_angle`.
    #[test]
    fn kbd_role_is_silent_on_markup_sphinx_accepts() {
        for source in [":kbd:`<>`", ":kbd:`x <>`"] {
            assert_role_silent(&KbdRoleValidator::new(), source);
        }
    }

    /// Probe case `menu_angle`.
    #[test]
    fn menuselection_role_is_silent_on_markup_sphinx_accepts() {
        for source in [":menuselection:`<>`", ":menuselection:`x <>`"] {
            assert_role_silent(&MenuSelectionRoleValidator::new(), source);
        }
    }

    /// Probe cases `guilabel_amp`, `guilabel_angle`: a lone `&` is the
    /// documented access-key marker (`:guilabel:`&File`` wraps `F` in an
    /// `accelerator` span, `GUILabel.amp_re`, `sphinx/roles.py`) and `&&` is
    /// the literal ampersand, so the validator's "use `&amp;`" advice was
    /// wrong as well as unwanted.
    #[test]
    fn guilabel_role_is_silent_on_markup_sphinx_accepts() {
        for source in [
            ":guilabel:`&File`",
            ":guilabel:`Save &As`",
            ":guilabel:`a && b`",
            ":guilabel:`<>`",
        ] {
            assert_role_silent(&GuiLabelRoleValidator::new(), source);
        }
    }
}
