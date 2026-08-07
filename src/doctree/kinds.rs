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
