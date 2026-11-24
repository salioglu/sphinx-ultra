//! docutils element tagnames, and the class predicates the transforms
//! test a node's kind with.
//!
//! Names ARE the wire format: `pformat` prints them and the differential
//! fixture (tests/fixtures/doctree_differential.json) compares them
//! byte-for-byte against docutils 0.22.4 output. Later waves append consts;
//! never rename one without regenerating the fixture.
//!
//! A docutils transform asks what class a node is an instance of
//! (`isinstance(node, nodes.FixedTextElement)`); here it asks what its kind
//! names: [`is_fixed_text_element`], [`is_special`] and
//! [`is_non_smartquotable`], beside `nodes.TextElement`'s
//! [`crate::doctree::is_text_element`]. Each lists the tagnames of the
//! concrete classes of docutils 0.22.4 (`nodes.py`) and Sphinx 9.1.0
//! (`addnodes.py`) that subclass the class; the abstract bases have no
//! tagname a doctree can hold.

/// Sentinel kind for text leaves; never printed as a tag.
pub const TEXT: &str = "#text";

pub const DOCUMENT: &str = "document";
pub const SECTION: &str = "section";
pub const TITLE: &str = "title";
pub const PARAGRAPH: &str = "paragraph";
pub const TRANSITION: &str = "transition";
pub const BULLET_LIST: &str = "bullet_list";
pub const ENUMERATED_LIST: &str = "enumerated_list";
pub const LIST_ITEM: &str = "list_item";
pub const DEFINITION_LIST: &str = "definition_list";
pub const DEFINITION_LIST_ITEM: &str = "definition_list_item";
pub const TERM: &str = "term";
pub const CLASSIFIER: &str = "classifier";
pub const DEFINITION: &str = "definition";
pub const BLOCK_QUOTE: &str = "block_quote";
pub const ATTRIBUTION: &str = "attribution";
pub const LITERAL_BLOCK: &str = "literal_block";
pub const DOCTEST_BLOCK: &str = "doctest_block";
pub const LINE_BLOCK: &str = "line_block";
pub const LINE: &str = "line";
pub const COMMENT: &str = "comment";
pub const TARGET: &str = "target";
pub const SYSTEM_MESSAGE: &str = "system_message";

// wave 2: inline nodes
pub const EMPHASIS: &str = "emphasis";
pub const STRONG: &str = "strong";
pub const LITERAL: &str = "literal";
pub const PROBLEMATIC: &str = "problematic";
pub const REFERENCE: &str = "reference";
pub const TITLE_REFERENCE: &str = "title_reference";
pub const FOOTNOTE_REFERENCE: &str = "footnote_reference";
pub const CITATION_REFERENCE: &str = "citation_reference";
pub const SUBSTITUTION_REFERENCE: &str = "substitution_reference";
pub const SUBSCRIPT: &str = "subscript";
pub const SUPERSCRIPT: &str = "superscript";
pub const ABBREVIATION: &str = "abbreviation";
pub const ACRONYM: &str = "acronym";
pub const MATH: &str = "math";

// wave 2: footnotes/citations
pub const FOOTNOTE: &str = "footnote";
pub const CITATION: &str = "citation";
pub const LABEL: &str = "label";
pub const FIELD_LIST: &str = "field_list";
pub const FIELD: &str = "field";
pub const FIELD_NAME: &str = "field_name";
pub const FIELD_BODY: &str = "field_body";
pub const OPTION_LIST: &str = "option_list";
pub const OPTION_LIST_ITEM: &str = "option_list_item";
pub const OPTION_GROUP: &str = "option_group";
pub const OPTION: &str = "option";
pub const OPTION_STRING: &str = "option_string";
pub const OPTION_ARGUMENT: &str = "option_argument";
pub const DESCRIPTION: &str = "description";
// wave 3: directives
pub const SUBTITLE: &str = "subtitle";

pub const IMAGE: &str = "image";

// sphinx-only element names (sphinx/addnodes.py). They print through the
// same pformat path as the docutils ones; the oracle fixtures pin them.
pub const COMPACT_PARAGRAPH: &str = "compact_paragraph";
pub const COMPOUND: &str = "compound";
pub const ONLY: &str = "only";
pub const PENDING_XREF: &str = "pending_xref";
pub const TOCTREE: &str = "toctree";

pub const TABLE: &str = "table";
pub const TGROUP: &str = "tgroup";
pub const COLSPEC: &str = "colspec";
pub const THEAD: &str = "thead";
pub const TBODY: &str = "tbody";
pub const ROW: &str = "row";
pub const ENTRY: &str = "entry";

/// Whether `kind` names a `nodes.FixedTextElement` class: an element whose
/// text keeps its whitespace and markup characters as written, which
/// SmartQuotes neither educates nor takes as a block
/// (`SP/docutils/transforms/universal.py:248`), Sphinx's signature parts
/// included.
//
// oracle: every class `c` of `docutils.nodes` and `sphinx.addnodes` with
//   `issubclass(c, nodes.FixedTextElement)`, by tagname ([SQ] Appendix B,
//   re-probed in wave 5a task 16b).
pub fn is_fixed_text_element(kind: &str) -> bool {
    matches!(
        kind,
        "address"
            | "comment"
            | "doctest_block"
            | "literal_block"
            | "math_block"
            | "raw"
            | "desc_addname"
            | "desc_annotation"
            | "desc_name"
            | "desc_optional"
            | "desc_parameter"
            | "desc_parameterlist"
            | "desc_returns"
            | "desc_signature_line"
            | "desc_type"
            | "desc_type_parameter"
            | "desc_type_parameter_list"
            | "manpage"
            | "production"
    )
}

/// Whether `kind` names a `nodes.Special` class: the "special internal body
/// elements" (`SP/docutils/nodes.py`), which SmartQuotes never takes as a
/// block (`SP/docutils/transforms/universal.py:248`). Of them, `pending`
/// and `system_message` are not `TextElement`s, so a paragraph in a
/// `system_message` is a block of its own.
//
// oracle: as [`is_fixed_text_element`]'s, for `nodes.Special`.
pub fn is_special(kind: &str) -> bool {
    matches!(
        kind,
        "comment"
            | "index"
            | "pending"
            | "raw"
            | "substitution_definition"
            | "system_message"
            | "target"
    )
}

/// Whether `kind` names one of Sphinx's `NON_SMARTQUOTABLE_PARENT_NODES`
/// (`SP/sphinx/util/nodes.py:697-705`): a `FixedTextElement`, `literal`,
/// `math`, `image`, `raw`, `problematic`, or a class marked
/// `not_smartquotable` (Sphinx's signature nodes, `literal_emphasis`,
/// `literal_strong`). A Text with such an ancestor is context for the
/// quotes around it and is not educated itself (`is_smartquotable`,
/// `:708-716`).
//
// oracle: as [`is_fixed_text_element`]'s, for
//   `issubclass(c, NON_SMARTQUOTABLE_PARENT_NODES)`.
pub fn is_non_smartquotable(kind: &str) -> bool {
    is_fixed_text_element(kind)
        || matches!(
            kind,
            "literal"
                | "math"
                | "image"
                | "problematic"
                | "desc_inline"
                | "desc_signature"
                | "desc_sig_keyword"
                | "desc_sig_keyword_type"
                | "desc_sig_literal_char"
                | "desc_sig_literal_number"
                | "desc_sig_literal_string"
                | "desc_sig_name"
                | "desc_sig_operator"
                | "desc_sig_punctuation"
                | "desc_sig_space"
                | "literal_emphasis"
                | "literal_strong"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctree::is_text_element;

    /// Every concrete class of docutils 0.22.4 and Sphinx 9.1.0 that
    /// subclasses `TextElement`, `Special` or one of Sphinx's
    /// `NON_SMARTQUOTABLE_PARENT_NODES`, by tagname, with `(TextElement,
    /// FixedTextElement, Special, NON_SMARTQUOTABLE_PARENT_NODES)`, and two
    /// elements that are none of them.
    //
    // oracle: [SQ] Appendix B (`probe_classes.out`), re-probed in wave 5a
    //   task 16b with `issubclass` over every `nodes.Element` subclass of
    //   `docutils.nodes` and `sphinx.addnodes` in the locked core profile.
    const CLASSES: &[(&str, bool, bool, bool, bool)] = &[
        ("abbreviation", true, false, false, false),
        ("acronym", true, false, false, false),
        ("address", true, true, false, true),
        ("attribution", true, false, false, false),
        ("author", true, false, false, false),
        ("caption", true, false, false, false),
        ("citation_reference", true, false, false, false),
        ("classifier", true, false, false, false),
        ("comment", true, true, true, true),
        ("contact", true, false, false, false),
        ("copyright", true, false, false, false),
        ("date", true, false, false, false),
        ("doctest_block", true, true, false, true),
        ("emphasis", true, false, false, false),
        ("field_name", true, false, false, false),
        ("footnote_reference", true, false, false, false),
        ("generated", true, false, false, false),
        ("inline", true, false, false, false),
        ("label", true, false, false, false),
        ("line", true, false, false, false),
        ("literal", true, false, false, true),
        ("literal_block", true, true, false, true),
        ("math", true, false, false, true),
        ("math_block", true, true, false, true),
        ("option_argument", true, false, false, false),
        ("option_string", true, false, false, false),
        ("organization", true, false, false, false),
        ("paragraph", true, false, false, false),
        ("problematic", true, false, false, true),
        ("raw", true, true, true, true),
        ("reference", true, false, false, false),
        ("revision", true, false, false, false),
        ("rubric", true, false, false, false),
        ("status", true, false, false, false),
        ("strong", true, false, false, false),
        ("subscript", true, false, false, false),
        ("substitution_definition", true, false, true, false),
        ("substitution_reference", true, false, false, false),
        ("subtitle", true, false, false, false),
        ("superscript", true, false, false, false),
        ("target", true, false, true, false),
        ("term", true, false, false, false),
        ("title", true, false, false, false),
        ("title_reference", true, false, false, false),
        ("version", true, false, false, false),
        ("centered", true, false, false, false),
        ("compact_paragraph", true, false, false, false),
        ("desc_addname", true, true, false, true),
        ("desc_annotation", true, true, false, true),
        ("desc_inline", true, false, false, true),
        ("desc_name", true, true, false, true),
        ("desc_optional", true, true, false, true),
        ("desc_parameter", true, true, false, true),
        ("desc_parameterlist", true, true, false, true),
        ("desc_returns", true, true, false, true),
        ("desc_sig_keyword", true, false, false, true),
        ("desc_sig_keyword_type", true, false, false, true),
        ("desc_sig_literal_char", true, false, false, true),
        ("desc_sig_literal_number", true, false, false, true),
        ("desc_sig_literal_string", true, false, false, true),
        ("desc_sig_name", true, false, false, true),
        ("desc_sig_operator", true, false, false, true),
        ("desc_sig_punctuation", true, false, false, true),
        ("desc_sig_space", true, false, false, true),
        ("desc_signature", true, false, false, true),
        ("desc_signature_line", true, true, false, true),
        ("desc_type", true, true, false, true),
        ("desc_type_parameter", true, true, false, true),
        ("desc_type_parameter_list", true, true, false, true),
        ("download_reference", true, false, false, false),
        ("index", true, false, true, false),
        ("literal_emphasis", true, false, false, true),
        ("literal_strong", true, false, false, true),
        ("manpage", true, true, false, true),
        ("number_reference", true, false, false, false),
        ("pending_xref_condition", true, false, false, false),
        ("production", true, true, false, true),
        ("versionmodified", true, false, false, false),
        ("image", false, false, false, true),
        ("pending", false, false, true, false),
        ("system_message", false, false, true, false),
        (SECTION, false, false, false, false),
        (PENDING_XREF, false, false, false, false),
    ];

    #[test]
    fn class_predicates_follow_the_class_hierarchy() {
        for &(kind, text, fixed, special, literal) in CLASSES {
            assert_eq!(
                (
                    is_text_element(kind),
                    is_fixed_text_element(kind),
                    is_special(kind),
                    is_non_smartquotable(kind),
                ),
                (text, fixed, special, literal),
                "{kind}"
            );
        }
        // Every TextElement kind is in the table, so the table pins the
        // other three predicates for each: 45 docutils classes and 33
        // Sphinx ones.
        for kind in crate::doctree::TEXT_ELEMENT_KINDS {
            assert!(
                CLASSES.iter().any(|row| row.0 == *kind),
                "the table leaves out the TextElement {kind}"
            );
        }
        assert_eq!(crate::doctree::TEXT_ELEMENT_KINDS.len(), 78);
    }
}
