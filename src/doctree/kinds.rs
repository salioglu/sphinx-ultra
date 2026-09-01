//! docutils element tagnames.
//!
//! Names ARE the wire format: `pformat` prints them and the differential
//! fixture (tests/fixtures/doctree_differential.json) compares them
//! byte-for-byte against docutils 0.22.4 output. Later waves append consts;
//! never rename one without regenerating the fixture.

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
