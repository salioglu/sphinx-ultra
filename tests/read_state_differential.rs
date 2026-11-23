//! Differential test: the document state our read leaves behind vs Sphinx
//! 9.1.0's, for the committed read-state corpus
//! (`tests/fixtures/read_state_differential.json`; research report "read
//! document state" §11).
//!
//! Regenerate the fixture (manual, never in CI), with the locked core
//! oracle profile:
//!     PYTHONNOUSERSITE=1 uv run --locked --python 3.12 \
//!         --project tools/oracle_profiles/core python tools/gen_read_state_fixture.py
//!
//! Each case is one `index.rst` snippet read standalone, and five keys of
//! the read are compared separately:
//!
//! - `pseudo_xml`: the post-read doctree ([`read_standalone`]);
//! - `warnings`: the read's warning stream, one entry per line;
//! - `id_trace`: every id the read allocated, `[phase, kind, id, attached]`
//!   (the parse's [`IdAllocator`](sphinx_ultra::transforms::state::IdAllocator)
//!   trace, with `attached` filled in by the read). The fixture's whole
//!   trace is compared: until the read transforms land (T9b), ours holds
//!   parse events only;
//! - `registries_post_parse`: docutils' node registries and name table as
//!   the parse leaves them
//!   ([`registries_dump`](sphinx_ultra::transforms::state::DocumentState::registries_dump)
//!   of [`parse_rst_full`]'s state);
//! - `final_position`: where docutils locates a message with neither a node
//!   nor a line, `[source, line]`.
//!
//! The mismatching `(case, key)` pairs must equal [`KNOWN_READ_STATE_GAPS`]
//! exactly; [`read_state_census`] prints the rows.

mod support {
    pub mod read_gaps;
}

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use sphinx_ultra::config::BuildConfig;
use sphinx_ultra::read::{read_standalone, ReadSettings};
use sphinx_ultra::rst::{parse_rst_full, ParseOptions};
use sphinx_ultra::transforms::state::SourceRef;
use support::read_gaps::{self, Census, GapFix};

#[derive(serde::Deserialize)]
struct Fixture {
    docutils_version: String,
    sphinx_version: String,
    settings: FixtureSettings,
    cases: Vec<Case>,
}

/// The read settings the generator recorded in the fixture header: the ones
/// the read consumes.
#[derive(serde::Deserialize)]
struct FixtureSettings {
    keep_warnings: bool,
    smartquotes: bool,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    rst: String,
    pseudo_xml: Value,
    warnings: Value,
    id_trace: Value,
    registries_post_parse: Value,
    final_position: Value,
}

impl Case {
    /// The fixture's value of `key`.
    fn expected(&self, key: &str) -> &Value {
        match key {
            "pseudo_xml" => &self.pseudo_xml,
            "warnings" => &self.warnings,
            "id_trace" => &self.id_trace,
            "registries_post_parse" => &self.registries_post_parse,
            "final_position" => &self.final_position,
            other => panic!("no key {other:?}"),
        }
    }
}

/// The compared keys, in the fixture's order of interest.
const KEYS: [&str; 5] = [
    "pseudo_xml",
    "warnings",
    "id_trace",
    "registries_post_parse",
    "final_position",
];

fn load_fixture() -> Fixture {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/read_state_differential.json"
    ));
    let fixture: Fixture = serde_json::from_str(raw).expect("fixture parses");
    assert_eq!(fixture.docutils_version, "0.22.4");
    assert_eq!(fixture.sphinx_version, "9.1.0");
    assert!(
        fixture.cases.len() >= 80,
        "fixture truncated? only {} cases",
        fixture.cases.len()
    );
    fixture
}

/// The read settings the fixture's header gives through a [`BuildConfig`]
/// (M1's validation off, decision 7), with the id trace on.
fn read_settings_of(settings: &FixtureSettings) -> ReadSettings {
    assert!(
        !settings.smartquotes,
        "no smart-quote transform exists yet, so only `smartquotes = False` \
         reads like the oracle"
    );
    ReadSettings {
        trace_ids: true,
        ..ReadSettings::from_config(&BuildConfig {
            validate_directives: false,
            keep_warnings: settings.keep_warnings,
            ..BuildConfig::default()
        })
    }
}

/// Our value of every key for `rst`, read as the oracle harness reads a
/// snippet (`<SRCDIR>/index.rst`, docname `index`, no project).
fn ours(rst: &str, settings: &ReadSettings) -> BTreeMap<&'static str, Value> {
    let parse = ParseOptions {
        source_path: "<SRCDIR>/index.rst".to_string(),
        sphinx: true,
        docname: "index".to_string(),
        ..ParseOptions::default()
    };
    let read = read_standalone(rst, &parse, settings);
    let doctree = read.outcome.doctree();
    let state = read.outcome.state();

    // Sphinx's stream is each record's text and a line break.
    let stream: String = read
        .diagnostics
        .iter()
        .map(|diagnostic| format!("{}\n", diagnostic.render(true)))
        .collect();
    let warnings: Vec<&str> = match stream.strip_suffix('\n') {
        Some(text) => text.split('\n').collect(),
        None => Vec::new(),
    };
    let trace: Vec<Value> = state
        .ids
        .trace()
        .iter()
        .map(|event| json!([event.phase, event.kind, event.id, event.attached]))
        .collect();
    let position = state.final_position;
    let source = position.source.map(|SourceRef::Table(id)| {
        doctree
            .sources
            .get(usize::from(id))
            .cloned()
            .unwrap_or_default()
    });
    let parsed = parse_rst_full(rst, &parse);

    BTreeMap::from([
        ("pseudo_xml", Value::String(doctree.root.pformat())),
        ("warnings", json!(warnings)),
        ("id_trace", Value::Array(trace)),
        (
            "registries_post_parse",
            parsed.state.registries_dump(&parsed.doctree.root),
        ),
        ("final_position", json!([source, position.line])),
    ])
}

/// Every mismatching `(case, key)`, with the fixture's value and ours.
fn mismatches(fixture: &Fixture) -> BTreeMap<(String, &'static str), (Value, Value)> {
    let settings = read_settings_of(&fixture.settings);
    let mut found = BTreeMap::new();
    for case in &fixture.cases {
        let rst = case.rst.clone();
        let settings = settings.clone();
        let ours = match std::panic::catch_unwind(move || ours(&rst, &settings)) {
            Ok(ours) => ours,
            Err(_) => KEYS
                .iter()
                .map(|key| (*key, Value::String("PANICKED".to_string())))
                .collect(),
        };
        for key in KEYS {
            let expected = case.expected(key);
            if &ours[key] != expected {
                found.insert(
                    (case.name.clone(), key),
                    (expected.clone(), ours[key].clone()),
                );
            }
        }
    }
    found
}

/// The `(case, key)` pairs whose read state is not Sphinx's yet, each with
/// the fix families its divergence shows and the latest task that fixes
/// one of them. Checked strictly: a listed pair that matches fails the
/// test, and so does an unlisted pair that differs. Rows are printed by
/// [`read_state_census`], one per line as the census prints them, so a task
/// deletes or re-attributes a row by editing one line and a grep for an
/// owner shows whole rows.
#[rustfmt::skip]
const KNOWN_READ_STATE_GAPS: &[(&str, &str, &str)] = &[
    ("state.alias_target_before_reference", "id_trace", "dangling refs+indirect targets: 5a-T10"),
    ("state.alias_target_before_reference", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.alias_target_before_reference", "warnings", "dangling refs+indirect targets: 5a-T10"),
    ("state.citation_backrefs", "id_trace", "citations: 5a-T11"),
    ("state.citation_backrefs", "pseudo_xml", "citations: 5a-T11"),
    ("state.dangling_footnote_ref_in_title", "id_trace", "dangling refs: 5a-T10"),
    ("state.dangling_footnote_ref_in_title", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.dangling_footnote_ref_in_title", "warnings", "dangling refs: 5a-T10"),
    ("state.docfields_footnote_order", "pseudo_xml", "footnotes: 5a-T11"),
    ("state.docinfo_abstract_footnote_order", "id_trace", "docinfo+footnotes: 5a-T12"),
    ("state.docinfo_abstract_footnote_order", "pseudo_xml", "docinfo+footnotes: 5a-T12"),
    ("state.dup_indirect_same_refname", "pseudo_xml", "indirect targets+system_message: 5a-T10"),
    ("state.final_position.anonymous_target", "pseudo_xml", "indirect targets: 5a-T10"),
    ("state.final_position.block_quote", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.block_quote", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.block_quote", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.block_quote_ending_in_list", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.block_quote_ending_in_list", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.block_quote_ending_in_list", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.bullet_list", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.bullet_list", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.bullet_list", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.citation", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.citation", "pseudo_xml", "citations+dangling refs: 5a-T11"),
    ("state.final_position.citation", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.comment", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.comment", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.comment", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.definition_list", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.definition_list", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.definition_list", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.directive", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.directive", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.directive", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.directive_then_target", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.directive_then_target", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.directive_then_target", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.doctest_block", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.doctest_block", "pseudo_xml", "dangling refs+doctest class: 5a-T10"),
    ("state.final_position.doctest_block", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.empty_comment", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.empty_comment", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.empty_comment", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.empty_comment_blank", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.empty_comment_blank", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.empty_comment_blank", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.enumerated_list", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.enumerated_list", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.enumerated_list", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.external_target_continued", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.external_target_continued", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.external_target_continued", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.field_list", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.field_list", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.field_list", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.footnote", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.footnote", "pseudo_xml", "dangling refs+footnotes: 5a-T11"),
    ("state.final_position.footnote", "warnings", "dangling refs+footnotes: 5a-T11"),
    ("state.final_position.grid_table", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.grid_table", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.grid_table", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.image", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.image", "pseudo_xml", "candidates+dangling refs: 5a-T13a"),
    ("state.final_position.image", "warnings", "candidates+dangling refs: 5a-T13a"),
    ("state.final_position.line_block", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.line_block", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.line_block", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.line_block_blank", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.line_block_blank", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.line_block_blank", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.line_block_single", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.line_block_single", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.line_block_single", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.list_item_paragraph", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.list_item_paragraph", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.list_item_paragraph", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.literal_block", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.literal_block", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.literal_block", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.literal_expected", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.literal_expected", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.literal_expected", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.literal_expected_blank", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.literal_expected_blank", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.literal_expected_blank", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.nested_section_list", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.nested_section_list", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.nested_section_list", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.nested_section_paragraph", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.nested_section_paragraph", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.nested_section_paragraph", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.option_list", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.option_list", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.option_list", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.paragraph", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.paragraph", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.paragraph", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.paragraph_after_target", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.paragraph_after_target", "pseudo_xml", "dangling refs+propagate targets: 5a-T10"),
    ("state.final_position.paragraph_after_target", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.paragraph_trailing_blanks", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.paragraph_trailing_blanks", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.paragraph_trailing_blanks", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.quoted_literal", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.quoted_literal", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.quoted_literal", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.quoted_literal_blank", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.quoted_literal_blank", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.quoted_literal_blank", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.section_ends_in_list", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.section_ends_in_list", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.section_ends_in_list", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.section_overline", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.section_overline", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.section_overline", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.section_title", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.section_title", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.section_title", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.simple_table", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.simple_table", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.simple_table", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.substitution_definition", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.substitution_definition", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.substitution_definition", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.target", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.target", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.target", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.target_trailing_blanks", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.target_trailing_blanks", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.target_trailing_blanks", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.title_then_directive", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.title_then_directive", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.final_position.title_then_directive", "warnings", "dangling refs: 5a-T10"),
    ("state.final_position.transition", "id_trace", "dangling refs: 5a-T10"),
    ("state.final_position.transition", "pseudo_xml", "dangling refs+transitions: 5a-T10"),
    ("state.final_position.transition", "warnings", "dangling refs+transitions: 5a-T10"),
    ("state.footnote_ref_kinds", "pseudo_xml", "footnotes: 5a-T11"),
    ("state.glossary_sorted_footnote_order", "pseudo_xml", "footnotes: 5a-T11"),
    ("state.location.admonition_title", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.admonition_title", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.admonition_title", "warnings", "dangling refs: 5a-T10"),
    ("state.location.attribution", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.attribution", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.attribution", "warnings", "dangling refs: 5a-T10"),
    ("state.location.block_quote", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.block_quote", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.block_quote", "warnings", "dangling refs: 5a-T10"),
    ("state.location.bullet_item", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.bullet_item", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.bullet_item", "warnings", "dangling refs: 5a-T10"),
    ("state.location.citation_body", "id_trace", "citations+dangling refs: 5a-T11"),
    ("state.location.citation_body", "pseudo_xml", "citations+dangling refs: 5a-T11"),
    ("state.location.citation_body", "warnings", "dangling refs: 5a-T10"),
    ("state.location.classifier", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.classifier", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.classifier", "warnings", "dangling refs: 5a-T10"),
    ("state.location.definition", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.definition", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.definition", "warnings", "dangling refs: 5a-T10"),
    ("state.location.enumerated_item", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.enumerated_item", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.enumerated_item", "warnings", "dangling refs: 5a-T10"),
    ("state.location.field_body", "id_trace", "dangling refs+docinfo: 5a-T12"),
    ("state.location.field_body", "pseudo_xml", "docinfo: 5a-T12"),
    ("state.location.field_body", "warnings", "dangling refs: 5a-T10"),
    ("state.location.figure_caption", "id_trace", "autonumber ids+dangling refs: 5a-T12"),
    ("state.location.figure_caption", "pseudo_xml", "autonumber ids+candidates+dangling refs: 5a-T13a"),
    ("state.location.figure_caption", "warnings", "candidates+dangling refs: 5a-T13a"),
    ("state.location.footnote_body", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.footnote_body", "pseudo_xml", "dangling refs+footnotes: 5a-T11"),
    ("state.location.footnote_body", "warnings", "dangling refs: 5a-T10"),
    ("state.location.glossary_definition", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.glossary_definition", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.glossary_definition", "warnings", "dangling refs: 5a-T10"),
    ("state.location.grid_table_cell", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.grid_table_cell", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.grid_table_cell", "warnings", "dangling refs: 5a-T10"),
    ("state.location.image_target", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.image_target", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.image_target", "warnings", "dangling refs: 5a-T10"),
    ("state.location.image_target_mid", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.image_target_mid", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.image_target_mid", "warnings", "dangling refs: 5a-T10"),
    ("state.location.line_block", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.line_block", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.line_block", "warnings", "dangling refs: 5a-T10"),
    ("state.location.list_table_cell", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.list_table_cell", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.list_table_cell", "warnings", "dangling refs: 5a-T10"),
    ("state.location.note_body", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.note_body", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.note_body", "warnings", "dangling refs: 5a-T10"),
    ("state.location.option_description", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.option_description", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.option_description", "warnings", "dangling refs: 5a-T10"),
    ("state.location.paragraph_continuation", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.paragraph_continuation", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.paragraph_continuation", "warnings", "dangling refs: 5a-T10"),
    ("state.location.rubric", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.rubric", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.rubric", "warnings", "dangling refs: 5a-T10"),
    ("state.location.section_title", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.section_title", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.section_title", "warnings", "dangling refs: 5a-T10"),
    ("state.location.sidebar_title", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.sidebar_title", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.sidebar_title", "warnings", "dangling refs: 5a-T10"),
    ("state.location.simple_table_cell", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.simple_table_cell", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.simple_table_cell", "warnings", "dangling refs: 5a-T10"),
    ("state.location.substitution_definition", "id_trace", "dangling refs+substitutions: 5a-T11"),
    ("state.location.substitution_definition", "pseudo_xml", "dangling refs+substitutions: 5a-T11"),
    ("state.location.substitution_definition", "warnings", "dangling refs+substitutions: 5a-T11"),
    ("state.location.table_title", "id_trace", "autonumber ids+dangling refs: 5a-T12"),
    ("state.location.table_title", "pseudo_xml", "autonumber ids+dangling refs: 5a-T12"),
    ("state.location.table_title", "warnings", "dangling refs: 5a-T10"),
    ("state.location.topic_title", "id_trace", "dangling refs: 5a-T10"),
    ("state.location.topic_title", "pseudo_xml", "dangling refs: 5a-T10"),
    ("state.location.topic_title", "warnings", "dangling refs: 5a-T10"),
    ("state.loose_ids_chain", "id_trace", "dangling refs+substitutions: 5a-T11"),
    ("state.loose_ids_chain", "pseudo_xml", "dangling refs+substitutions: 5a-T11"),
    ("state.loose_ids_chain", "warnings", "dangling refs+substitutions: 5a-T11"),
    ("state.pending_serials", "id_trace", "pending: 5a-T18b"),
    ("state.pending_serials", "pseudo_xml", "class attribute+footnotes+indirect targets+pending+system_message: 5a-T18b"),
    ("state.pending_serials", "registries_post_parse", "pending: 5a-T18a"),
    ("state.pending_serials", "warnings", "pending: 5a-T18a"),
    ("state.problematic_id_shapes", "id_trace", "dangling refs+footnotes: 5a-T11"),
    ("state.problematic_id_shapes", "pseudo_xml", "dangling refs+footnotes: 5a-T11"),
    ("state.problematic_id_shapes", "warnings", "dangling refs+footnotes: 5a-T11"),
    ("state.rejected_list_table_refs", "id_trace", "footnotes: 5a-T11"),
    ("state.rejected_list_table_refs", "pseudo_xml", "footnotes: 5a-T11"),
    ("state.rejected_list_table_refs", "warnings", "footnotes: 5a-T11"),
    ("state.rejected_subst_autofootnote", "id_trace", "footnotes: 5a-T11"),
    ("state.rejected_subst_autofootnote", "pseudo_xml", "footnotes: 5a-T11"),
    ("state.rejected_subst_autofootnote", "warnings", "footnotes: 5a-T11"),
    ("state.rejected_subst_indirect_target", "id_trace", "indirect targets: 5a-T10"),
    ("state.rejected_subst_indirect_target", "warnings", "indirect targets: 5a-T10"),
    ("state.rejected_subst_plain_target", "pseudo_xml", "indirect targets: 5a-T10"),
    ("state.rejected_table_refs", "id_trace", "footnotes: 5a-T11"),
    ("state.rejected_table_refs", "pseudo_xml", "footnotes: 5a-T11"),
    ("state.rejected_table_refs", "warnings", "footnotes: 5a-T11"),
    ("state.rst_class_before_section", "pseudo_xml", "class attribute: 5a-T12"),
    ("state.sortids_startswith", "pseudo_xml", "propagate targets: 5a-T10"),
    ("state.subst_copy_refnames_order", "id_trace", "indirect targets+substitutions: 5a-T11"),
    ("state.subst_copy_refnames_order", "pseudo_xml", "dangling refs+substitutions: 5a-T11"),
    ("state.subst_copy_refnames_order", "warnings", "indirect targets: 5a-T10"),
    ("state.unreferenced_order", "pseudo_xml", "footnotes: 5a-T11"),
    ("state.unreferenced_order", "warnings", "footnotes: 5a-T11"),
];

/// Holds every key of every read-state case to the oracle, strictly, against
/// [`KNOWN_READ_STATE_GAPS`].
#[test]
fn read_state_differential() {
    let fixture = load_fixture();
    let names: BTreeSet<&str> = fixture
        .cases
        .iter()
        .map(|case| case.name.as_str())
        .collect();
    assert_eq!(names.len(), fixture.cases.len(), "a case name repeats");

    let found = mismatches(&fixture);
    let known: BTreeSet<(&str, &str)> = KNOWN_READ_STATE_GAPS
        .iter()
        .map(|(case, key, _)| (*case, *key))
        .collect();
    assert_eq!(
        known.len(),
        KNOWN_READ_STATE_GAPS.len(),
        "KNOWN_READ_STATE_GAPS lists a (case, key) twice"
    );
    for (case, key) in &known {
        assert!(names.contains(case), "no case {case:?} in the fixture");
        assert!(KEYS.contains(key), "no key {key:?}");
    }
    let observed: BTreeSet<(&str, &str)> = found
        .keys()
        .map(|(case, key)| (case.as_str(), *key))
        .collect();
    let regressions: Vec<String> = observed
        .difference(&known)
        .map(|(case, key)| {
            let (expected, actual) = &found[&(case.to_string(), *key)];
            format!(
                "    ({case:?}, {key:?})\n        expected: {expected}\n        actual:   {actual}"
            )
        })
        .collect();
    let fixed: Vec<&(&str, &str)> = known.difference(&observed).collect();
    assert!(
        regressions.is_empty() && fixed.is_empty(),
        "the read-state mismatch set differs from KNOWN_READ_STATE_GAPS: {} regressions, {} \
         fixed (delete their rows: {fixed:?})\n{}",
        regressions.len(),
        fixed.len(),
        regressions.join("\n")
    );
}

/// Prints the current read-state mismatch set as `KNOWN_READ_STATE_GAPS`
/// rows. A row's reason lists the fix families its divergence shows and the
/// latest task that fixes one of them ([`support::read_gaps`]); a
/// divergence no family explains gets a `?` family and a comment above its
/// row, and fails the census once every row is printed:
///
/// `cargo test --locked --test read_state_differential read_state_census -- --ignored --nocapture`
#[test]
#[ignore]
fn read_state_census() {
    let fixture = load_fixture();
    let found = mismatches(&fixture);
    let mut table = String::new();
    let mut unexplained_rows = 0;
    for ((case, key), (expected, actual)) in &found {
        // The case's tree divergence: what an id whose node left the tree,
        // or a record about copied content, depends on.
        let tree = found
            .get(&(case.clone(), "pseudo_xml"))
            .map(|(expected, actual)| {
                read_gaps::tree_census(
                    expected.as_str().unwrap_or_default(),
                    actual.as_str().unwrap_or_default(),
                )
            });
        let census = match *key {
            "pseudo_xml" => tree.unwrap_or_default(),
            "warnings" => warning_census(expected, actual, tree.as_ref()),
            "id_trace" => trace_census(expected, actual, tree.as_ref()),
            "registries_post_parse" => registries_census(expected, actual),
            _ => Census {
                unexplained: vec![format!("{key}: expected {expected}, actual {actual}")],
                ..Census::default()
            },
        };
        if !census.unexplained.is_empty() {
            unexplained_rows += 1;
        }
        for note in &census.unexplained {
            for line in note.lines() {
                table.push_str(&format!("    // ? {line}\n"));
            }
        }
        table.push_str(&format!(
            "    ({case:?}, {key:?}, {:?}),\n",
            census.reason()
        ));
    }
    println!("    // {} rows\n{table}", found.len());
    assert_eq!(
        unexplained_rows, 0,
        "rows with a `?` family need a family the census does not know"
    );
}

/// The records of a warning stream given as its lines: a record starts at
/// a located line or a bare `WARNING:`/`ERROR:`/`CRITICAL:` one, and the
/// lines after it are its continuation.
fn records(lines: &Value) -> Vec<String> {
    let mut records: Vec<String> = Vec::new();
    for line in lines.as_array().into_iter().flatten() {
        let line = line.as_str().unwrap_or_default();
        let starts_record = line.starts_with("<SRCDIR>")
            || line.starts_with("internal padding after ")
            || ["WARNING: ", "ERROR: ", "CRITICAL: "]
                .iter()
                .any(|prefix| line.starts_with(prefix));
        match records.last_mut() {
            Some(record) if !starts_record => {
                record.push('\n');
                record.push_str(line);
            }
            _ => records.push(line.to_string()),
        }
    }
    records
}

/// Records only Sphinx prints, by message, with the transform or collector
/// that raises them [TW §6.3].
const SPHINX_ONLY_RECORDS: &[(&str, GapFix)] = &[
    ("Unknown target name: ", read_gaps::DANGLING_REFS),
    ("Anonymous hyperlink mismatch: ", read_gaps::DANGLING_REFS),
    ("Indirect hyperlink target ", read_gaps::INDIRECT_TARGETS),
    (
        "Undefined substitution referenced: ",
        read_gaps::SUBSTITUTIONS,
    ),
    (" footnote references: only ", read_gaps::FOOTNOTES),
    ("is not referenced. [ref.footnote]", read_gaps::FOOTNOTES),
    ("image file not readable: ", read_gaps::CANDIDATES),
    (" a transition.", read_gaps::TRANSITIONS),
];

/// The tree families of the case, among `families`, that a key beside
/// the tree depends on.
fn tree_families(tree: Option<&Census>, families: &[GapFix]) -> Vec<GapFix> {
    tree.into_iter()
        .flat_map(|tree| tree.fixes.iter().copied())
        .filter(|fix| families.contains(fix))
        .collect()
}

/// The families of a `warnings` divergence, record by record. AnonymousHyperlinks
/// and SphinxDanglingReferences report per reference, so where the case's
/// tree divergence (`tree`) shows Substitutions (220) copying references
/// in, their records need 220 as well.
fn warning_census(expected: &Value, actual: &Value, tree: Option<&Census>) -> Census {
    let expected = records(expected);
    let actual = records(actual);
    let expected: Vec<&str> = expected.iter().map(String::as_str).collect();
    let actual: Vec<&str> = actual.iter().map(String::as_str).collect();
    let mut census = Census::default();
    for hunk in read_gaps::line_hunks(&expected, &actual) {
        for record in hunk.removed {
            match SPHINX_ONLY_RECORDS
                .iter()
                .find(|(message, _)| record.contains(message))
            {
                Some((_, fix)) => census.add(*fix),
                None => census
                    .unexplained
                    .push(format!("a record only Sphinx prints: {record:?}")),
            }
        }
        for record in hunk.added {
            // The structure directives T18a ports are unknown to us.
            let pending = record.contains("Unknown directive type")
                && ["contents", "sectnum", "target-notes"]
                    .iter()
                    .any(|name| record.contains(&format!("\"{name}\"")));
            if pending {
                census.add(read_gaps::PENDING_DIRECTIVES);
            } else {
                census
                    .unexplained
                    .push(format!("a record only we print: {record:?}"));
            }
        }
    }
    if census.fixes.contains(&read_gaps::DANGLING_REFS) {
        for fix in tree_families(tree, &[read_gaps::SUBSTITUTIONS]) {
            census.add(fix);
        }
    }
    census
}

/// The transforms that allocate ids in the corpus, by `python_name`, with
/// the task that ports each [DS §5.2].
const ALLOCATING_TRANSFORMS: &[(&str, GapFix)] = &[
    (
        "docutils.transforms.references.Substitutions",
        read_gaps::SUBSTITUTIONS,
    ),
    (
        "docutils.transforms.references.AnonymousHyperlinks",
        read_gaps::DANGLING_REFS,
    ),
    (
        "docutils.transforms.references.IndirectHyperlinks",
        read_gaps::INDIRECT_TARGETS,
    ),
    (
        "sphinx.transforms.references.SphinxDanglingReferences",
        read_gaps::DANGLING_REFS,
    ),
    (
        "docutils.transforms.references.Footnotes",
        read_gaps::FOOTNOTES,
    ),
    (
        "docutils.transforms.references.TargetNotes",
        read_gaps::PENDING_TRANSFORMS,
    ),
    (
        "docutils.transforms.parts.Contents",
        read_gaps::PENDING_TRANSFORMS,
    ),
    ("sphinx.transforms.AutoNumbering", read_gaps::AUTONUMBER_IDS),
];

/// The families of an `id_trace` divergence. Each transform-phase event is
/// owned by the task that ports its transform. Parse events must match in
/// order; one whose node left the tree (`attached` differs) is explained by
/// the case's tree divergence, `tree`. Whatever the phase, the ids a
/// transform allocates in content Substitutions (220) copied in, and
/// whether a node survives the docinfo pop (340, 880), need those families
/// too.
fn trace_census(expected: &Value, actual: &Value, tree: Option<&Census>) -> Census {
    let events = |trace: &Value| -> Vec<(String, String, String, bool)> {
        trace
            .as_array()
            .into_iter()
            .flatten()
            .map(|event| {
                (
                    event[0].as_str().unwrap_or_default().to_string(),
                    event[1].as_str().unwrap_or_default().to_string(),
                    event[2].as_str().unwrap_or_default().to_string(),
                    event[3].as_bool().unwrap_or_default(),
                )
            })
            .collect()
    };
    let expected = events(expected);
    let actual = events(actual);
    let mut census = Census::default();
    for fix in tree_families(tree, &[read_gaps::SUBSTITUTIONS, read_gaps::DOCINFO]) {
        census.add(fix);
    }
    for (phase, kind, id, _) in expected.iter().filter(|event| event.0 != "parse") {
        match ALLOCATING_TRANSFORMS.iter().find(|(name, _)| name == phase) {
            Some((_, fix)) => census.add(*fix),
            None => census
                .unexplained
                .push(format!("{phase} allocates {id} for a {kind}")),
        }
    }
    let parse_events = |events: &[(String, String, String, bool)]| -> Vec<(String, String, bool)> {
        events
            .iter()
            .filter(|event| event.0 == "parse")
            .map(|(_, kind, id, attached)| (kind.clone(), id.clone(), *attached))
            .collect()
    };
    let without_attachment = |events: &[(String, String, bool)]| -> Vec<(String, String)> {
        events
            .iter()
            .map(|(kind, id, _)| (kind.clone(), id.clone()))
            .collect()
    };
    let expected_events = parse_events(&expected);
    let actual_events = parse_events(&actual);
    let expected_parse = without_attachment(&expected_events);
    let actual_parse = without_attachment(&actual_events);
    if expected_parse != actual_parse {
        // The contents directive's topic is the parse event T18a adds.
        let missing: BTreeSet<_> = expected_parse
            .iter()
            .filter(|event| !actual_parse.contains(event))
            .collect();
        let extra = actual_parse
            .iter()
            .any(|event| !expected_parse.contains(event));
        if !extra
            && missing
                .iter()
                .all(|(kind, id)| kind == "topic" && id == "contents")
        {
            census.add(read_gaps::PENDING_DIRECTIVES);
        } else {
            census.unexplained.push(format!(
                "parse events differ: expected {expected_parse:?}, actual {actual_parse:?}"
            ));
        }
    } else {
        let flipped = expected_events
            .iter()
            .zip(&actual_events)
            .any(|(expected, actual)| expected.2 != actual.2);
        if flipped {
            match tree {
                Some(tree) => {
                    for fix in &tree.fixes {
                        census.add(*fix);
                    }
                }
                None => census
                    .unexplained
                    .push("a parse event's node left a tree that matches".to_string()),
            }
        }
    }
    census
}

/// The families of a `registries_post_parse` divergence: names the parse
/// registers that ours does not.
fn registries_census(expected: &Value, actual: &Value) -> Census {
    let mut census = Census::default();
    let keys: BTreeSet<&String> = expected
        .as_object()
        .into_iter()
        .chain(actual.as_object())
        .flat_map(|registries| registries.keys())
        .collect();
    for key in keys {
        if expected[key] == actual[key] {
            continue;
        }
        let names = |value: &Value| -> Vec<String> {
            value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry[0].as_str().map(str::to_string))
                .collect()
        };
        let missing: Vec<String> = names(&expected[key])
            .into_iter()
            .filter(|name| !names(&actual[key]).contains(name))
            .collect();
        // The contents directive's topic registers `contents` (T18a).
        let only_contents = key == "names"
            && missing == ["contents"]
            && names(&actual[key])
                .iter()
                .all(|name| names(&expected[key]).contains(name));
        if only_contents {
            census.add(read_gaps::PENDING_DIRECTIVES);
        } else {
            census.unexplained.push(format!(
                "{key}: expected {}, actual {}",
                expected[key], actual[key]
            ));
        }
    }
    census
}
