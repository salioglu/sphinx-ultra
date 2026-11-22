//! Sphinx's read transforms over a parsed document (wave 5a).
//!
//! The parser stays transform-free (decision 1); it hands its docutils
//! document state on beside the doctree, and this stage reads both.

pub mod reporter;
pub mod state;
