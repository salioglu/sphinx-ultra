//! Recursive-descent RST parser with docutils-0.22.4 fidelity (M2 wave 1:
//! block grammar only — the inline parser arrives in wave 2).
//!
//! Fidelity contract: output `pformat()` is byte-identical to
//! `docutils.parsers.rst.Parser` parse-layer output for the construct set in
//! `tests/fixtures/doctree_differential.json`. Transforms (doctitle
//! promotion, target propagation, transition hoisting, message filtering)
//! are explicitly NOT applied here; they arrive as separate components in
//! later waves. Behavior sources: the committed differential fixture and the
//! probe notes in docs/superpowers/plans/2026-08-07-m2-wave1-probes.md.

mod block;
pub mod lines;

use crate::doctree::Doctree;

#[derive(Debug, Clone)]
pub struct ParseOptions {
    /// What `<document source="...">` prints (docutils `new_document` name).
    pub source_path: String,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions {
            source_path: "<string>".to_string(),
        }
    }
}

/// Parse RST source into a doctree. Total: never panics, never errors —
/// problems become `system_message` nodes, exactly like docutils.
pub fn parse_rst(source: &str, opts: &ParseOptions) -> Doctree {
    let lines = lines::Lines::new(source);
    let root = block::BlockParser::new(&lines, &opts.source_path, source.len()).parse_document();
    Doctree { root }
}
