#!/usr/bin/env python3
"""Generate tests/fixtures/sphinx_doctree_differential.json from Sphinx 9.1.0.

Regenerate with:

    uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' \
        python tools/gen_sphinx_fixture.py

THE SPHINX ORACLE. This fixture records what a REAL `sphinx-build` read phase
produces for each snippet: the probe-validated minimal deterministic harness
(docs/superpowers/plans/2026-08-13-m2-wave3-probes.md, section "## sphinx-oracle",
"harness3") drives `sphinx.util.docutils._parse_str_to_doctree` with
`default_settings=env.settings` and `transforms=app.registry.get_transforms()`
against a temp srcdir carrying a minimal conf.py with `extensions = []`.
That path was re-verified in this session to be byte-identical to a full
`SphinxTestApp(buildername='dummy')` + `app.build()` + `env.get_doctree()`
build for representative snippets (plain constructs, admonitions, images,
errors, tables, targets).

DO NOT use `sphinx.testing.restructuredtext.parse()`: it builds an ad-hoc
settings dict that omits `doctitle_xform=False` (and the other
`sphinx.environment.default_settings` pins), so docutils' DocTitle transform
promotes lone top-level sections -- a document shape no real Sphinx build
produces (probes doc, "TRAP" finding).

Pinned configuration (recorded in the fixture header, asserted at runtime):
  - conf.py: extensions=[], master_doc='index', exclude_patterns=['_build']
  - confoverrides: smartquotes=False  (Sphinx enables docutils smartquotes by
    default; disabling keeps wave-1/2 text conventions -- probes doc gotcha)
  - confoverrides: keep_warnings=True (FilterSystemMessages keeps WARNING(2)/
    ERROR(3)/SEVERE(4) system_messages in-tree like the wave-1/2 fixtures;
    DEBUG(0)/INFO(1) are still stripped -- probes doc FilterSystemMessages
    finding; INFO-emitting snippets are therefore excluded from this corpus)
  - env.settings pins (sphinx.environment.default_settings): auto_id_prefix='id',
    halt_level=5, doctitle_xform=False, sectsubtitle_xform=False
  - report_level: docutils default 2 (Sphinx pins none; probe-verified inert
    for tree shape -- system_message insertion is not gated by it)
  - language: 'en' (Sphinx default), docname: 'index'
  - sphinx.util.console.nocolor(): warning-stream text must not carry ANSI
  - per-case isolation: env.clear_doc + env.ref_context.clear() +
    env.prepare_settings before every parse (probes doc math-domain finding)

Read-phase transforms INSEPARABLE in this harness (probes doc enumeration --
this exact pipeline runs on every real Sphinx read; there is no lighter subset
through public API). Reader set: Substitutions(220), PropagateTargets(260),
DocTitle(320, disabled via settings), DocInfo(340), SectionSubTitle(350,
disabled), AnonymousHyperlinks(440), IndirectHyperlinks(460), Footnotes(620),
ExternalTargets(640), InternalTargets(660), StripComments(740, inert),
Decorations(820, inert), Transitions(830), ExposeInternals(840, inert).
Sphinx registry set (extensions=[]): ApplySourceWorkaround(10), i18n(10-25,
source of the document translation_progress attribute), RefOnlyBulletList(100),
DefaultSubstitutions/MoveModuleTargets/HandleCodeBlocks/AutoNumbering/
AutoIndexUpgrader(210), ReorderConsecutiveTargetAndIndexNodes(220), SortIds(261),
DoctestTransform(500), GlossarySorter(500), citation transforms(619),
UnreferencedFootnotesDetector(622), FootnoteDocnameUpdater(700),
SphinxSmartQuotes(750, disabled), SphinxDanglingReferences/SphinxDomains(850),
DoctreeReadEvent(880, fires doctree-read -> environment collectors, e.g.
ImageCollector adds `candidates` to every image node), UIDTransform(880,
invisible), AddTranslationClasses(950, inert), FilterSystemMessages(999),
RemoveTranslatableInline(999).

Normalizations applied to recorded pseudo_xml (the ONLY two rewrites):
  1. the temp srcdir's index.rst absolute path -> "<snippet>" (the Rust test
     passes the same token as ParseOptions.source_path); generation fails if
     any srcdir path survives;
  2. the document-level `translation_progress="{'total': 0, 'translated': 0}"`
     attribute (added unconditionally by i18n.TranslationProgressTotaliser) is
     stripped, because the Rust parse layer does not model it yet; generation
     fails if the attribute survives anywhere else.

CORPUS POLICY (merge bar): every emitted case is byte-identical between this
Sphinx oracle and the wave-1/2/3 docutils parse layer, hence zero-divergence
against the Rust parser. Candidate cases where the Sphinx read phase genuinely
diverges from the docutils parse layer (INFO stripping, PropagateTargets/
IndirectHyperlinks/ExternalTargets/AnonymousHyperlinks target rewrites,
Footnotes+FootnoteDocnameUpdater, DoctestTransform classes, doc-start docinfo
consumption, image `candidates`, Transitions edge warnings, Sphinx role
replacements for pep/rfc/code/index) are EXCLUDED and documented with full
diffs in the wave-3 recon report (sphinx-harness-report.md). Later tasks extend
this corpus with Sphinx-specific directives (toctree, code-block, versionadded/
versionchanged/deprecated, seealso, only, highlight, math, index, rst-class,
...) once the Rust side grows the sphinx registry + env surface.

Provenance: cases whose (family, name) mirror a case of
tests/fixtures/doctree_differential.json reuse that case's exact rst input;
three inputs are new (marked). Never remove or rename existing cases; later
waves only EXTEND the corpus and SUPPORTED_KINDS.
"""

import io
import json
import shutil
import sys
import tempfile
from pathlib import Path

import docutils
import sphinx

EXPECTED_DOCUTILS = "0.22.4"
EXPECTED_SPHINX = "9.1.0"

assert docutils.__version__ == EXPECTED_DOCUTILS, (
    f"docutils {docutils.__version__} != {EXPECTED_DOCUTILS}; "
    "regenerate with the pinned command in the module docstring"
)
assert sphinx.__version__ == EXPECTED_SPHINX, (
    f"sphinx {sphinx.__version__} != {EXPECTED_SPHINX}; "
    "regenerate with the pinned command in the module docstring"
)

from sphinx.util.console import nocolor  # noqa: E402

nocolor()  # warning text must not carry environment-dependent ANSI escapes

from sphinx.parsers import RSTParser  # noqa: E402
from sphinx.testing.util import SphinxTestApp  # noqa: E402
from sphinx.util.docutils import (  # noqa: E402
    _parse_str_to_doctree,
    docutils_namespace,
    patch_docutils,
)

SOURCE_TOKEN = "<snippet>"
TP_ATTR = " translation_progress=\"{'total': 0, 'translated': 0}\""

CONF_PY = (
    "project = 'fixture'\n"
    "extensions = []\n"
    "master_doc = 'index'\n"
    "exclude_patterns = ['_build']\n"
)

CONFOVERRIDES = {"smartquotes": False, "keep_warnings": True}

# Node kinds the corpus may produce (post-transform tagnames). A snippet
# producing anything else is a generator ERROR: the corpus must stay inside
# what the Rust parser implements. Later tasks EXTEND this set (toctree,
# compound wrappers, versionmodified, pending_xref, ...).
SUPPORTED_KINDS = {
    "#text",
    "document",
    "section",
    "title",
    "subtitle",
    "paragraph",
    "transition",
    "bullet_list",
    "enumerated_list",
    "list_item",
    "definition_list",
    "definition_list_item",
    "term",
    "classifier",
    "definition",
    "block_quote",
    "attribution",
    "literal_block",
    "line_block",
    "line",
    "comment",
    "target",
    "system_message",
    "problematic",
    # inline
    "emphasis",
    "strong",
    "literal",
    "reference",
    "title_reference",
    "subscript",
    "superscript",
    "abbreviation",
    "acronym",
    "math",
    # field lists
    "field_list",
    "field",
    "field_name",
    "field_body",
    # tables
    "table",
    "tgroup",
    "colspec",
    "thead",
    "tbody",
    "row",
    "entry",
    # admonitions
    "note",
    "warning",
    "tip",
    "hint",
    "important",
    "caution",
    "danger",
    "error",
    "attention",
    "admonition",
    # body directives
    "image",
    "topic",
    "sidebar",
    "rubric",
    "compound",
    "container",
    # wave-3 task 7: sphinx directives + xref roles
    "subtitle",
    "caption",
    "versionmodified",
    "inline",
    "seealso",
    "pending_xref",
    "highlightlang",
    "only",
    "toctree",
}

CASES = [
    # ===== sx_plain =====
    ('sx_plain', 'paragraphs_single', 'Just some text.\n'),
    ('sx_plain', 'paragraphs_multiline', 'line one\nline two\n'),
    ('sx_plain', 'paragraphs_blank_separated', 'para one\n\n\npara two\n'),
    ('sx_plain', 'paragraphs_punctuation_text', 'x -- y; z: w, (v) [u].\n'),
    ('sx_plain', 'sections_simple_nested', 'Title\n=====\n\nPara under title.\n\nSub\n---\n\nPara under sub.\n'),
    ('sx_plain', 'sections_three_levels', 'A\n=\n\nB\n-\n\nC\n~\n\ndeep text\n\nD\n-\n\nback at two\n'),
    ('sx_plain', 'sections_over_under', '=====\nOver\n=====\n\nbody here\n'),
    ('sx_plain', 'sections_over_under_centered', '==========\n  Title\n==========\n\nbody\n'),
    ('sx_plain', 'sections_underline_exact_length', 'AB\n==\n\nx\n'),
    ('sx_plain', 'sections_underline_longer', 'AB\n=====\n\nx\n'),
    ('sx_plain', 'sections_unicode_title', 'Überblick\n=========\n\ntext\n'),
    ('sx_plain', 'sections_digit_title', '123\n=====\n\ntext\n'),
    ('sx_plain', 'sections_numbered_title', '1. Intro\n========\n\ntext\n'),
    ('sx_plain', 'sections_same_level_siblings', 'Alpha\n-----\n\none\n\nB\n----------\n\ntwo\n'),
    ('sx_plain', 'sections_whitespace_collapse_title', 'My  Section    Title!\n=====================\n\nx\n'),
    ('sx_plain', 'transition_basic', 'Para.\n\n----\n\nMore.\n'),
    ('sx_plain', 'transition_other_chars', 'a\n\n====\n\nb\n\n~~~~\n\nc\n\n****\n\nd\n\n::::\n\ne\n\n____\n\nf\n'),
    ('sx_plain', 'lists_bullet_simple', '- one\n- two\n- three\n'),
    ('sx_plain', 'lists_bullet_loose', '- one\n\n- two\n'),
    ('sx_plain', 'lists_bullet_nested', '- outer one\n\n  * inner a\n\n  * inner b\n\n- outer two\n'),
    ('sx_plain', 'lists_bullet_multi_paragraph_item', '- first para of item\n\n  second para of item\n'),
    ('sx_plain', 'lists_bullet_star_and_plus', '* star one\n* star two\n\n+ plus one\n+ plus two\n'),
    ('sx_plain', 'lists_bullet_marker_alone', '-\n  body from next line\n'),
    ('sx_plain', 'lists_bullet_ends_no_blank', '- item\nplain\n'),
    ('sx_plain', 'lists_bullet_deep_nesting', '- a\n\n  - b\n\n    - c\n\n      - d\n\n        - e\n'),
    ('sx_plain', 'lists_bullet_different_bullet_adjacent', '- a\n* b\n'),
    ('sx_plain', 'lists_bullet_item_with_quote', '- item\n\n      quoted deeper\n'),
    ('sx_plain', 'lists_enum_arabic', '1. one\n2. two\n3. three\n'),
    ('sx_plain', 'lists_enum_loweralpha', 'a. x\nb. y\n'),
    ('sx_plain', 'lists_enum_paren_arabic', '(1) x\n(2) y\n'),
    ('sx_plain', 'lists_enum_upper_paren', 'A) x\nB) y\n'),
    ('sx_plain', 'lists_enum_auto', '#. x\n#. y\n'),
    ('sx_plain', 'lists_enum_auto_continue', '1. one\n#. two\n'),
    ('sx_plain', 'lists_enum_single_i', 'i. single\n'),
    ('sx_plain', 'lists_enum_not_a_list', '1. one\nnot an item\n'),
    ('sx_plain', 'lists_enum_type_switch_aborts', '1. one\na. alpha\n'),
    ('sx_plain', 'lists_enum_continuation_lines', '1. first\n   more of first\n2. second\n'),
    ('sx_plain', 'deflist_simple', 'term\n    definition here\n'),
    ('sx_plain', 'deflist_classifiers', 'term2 : classifier one : classifier two\n    Definition2.\n'),
    ('sx_plain', 'deflist_colon_no_space', 'term:not a classifier\n    Definition.\n'),
    ('sx_plain', 'deflist_merge_items', 'term1\n    Def1.\n\nterm2\n    Def2.\n'),
    ('sx_plain', 'deflist_multi_para_definition', 'term\n    para one\n\n    para two\n'),
    ('sx_plain', 'deflist_nested_list_in_def', 'term\n    - a\n    - b\n'),
    ('sx_plain', 'deflist_ends_no_blank', 'term\n    def\nplain\n'),
    ('sx_plain', 'deflist_adjacent_items', 'term\n    def\nterm2\n    def2\n'),
    ('sx_plain', 'quote_simple', 'Para.\n\n    No matter where you go, there you are.\n'),
    ('sx_plain', 'quote_attribution', 'Para.\n\n    No matter where you go, there you are.\n\n    -- Buckaroo Banzai\n'),
    ('sx_plain', 'quote_attribution_em_dash', 'Para.\n\n    Quoted here.\n\n    — Author\n'),
    ('sx_plain', 'quote_two_quotes_split', 'Para.\n\n    First quote.\n\n    -- First Author\n\n    Second quote.\n\n    -- Second Author\n'),
    ('sx_plain', 'quote_nested_quote', 'Para.\n\n    outer quote\n\n        inner quote\n'),
    ('sx_plain', 'quote_list_inside_quote', 'Para.\n\n    - a\n    - b\n'),
    ('sx_plain', 'quote_multi_paragraph', 'Para.\n\n    first quoted para\n\n    second quoted para\n'),
    ('sx_plain', 'literal_expanded', 'Paragraph introducing::\n\n    literal line one\n    literal line two\n'),
    ('sx_plain', 'literal_minimized', 'Paragraph ends with ::\n\n    literal here\n'),
    ('sx_plain', 'literal_triple_colon', 'text:::\n\n    x\n'),
    ('sx_plain', 'literal_only_colons', '::\n\n    literal\n'),
    ('sx_plain', 'literal_quoted', 'Next is a quoted literal::\n\n> quoted line one\n> quoted line two\n'),
    ('sx_plain', 'literal_missing', 'Intro::\n\nNot indented.\n'),
    ('sx_plain', 'literal_ends_no_blank', 'para::\n\n    lit\nback\n'),
    ('sx_plain', 'literal_internal_blank_lines', 'code::\n\n    line one\n\n    line two\n'),
    ('sx_plain', 'literal_deeper_relative_indent', 'code::\n\n      six spaces\n        eight spaces\n'),
    ('sx_plain', 'lineblock_simple', '| Lend us a couple of bob till Thursday.\n| I am absolutely skint.\n'),
    ('sx_plain', 'lineblock_nested', '| top one\n| top two\n|     nested one\n| back\n|\n| after empty\n'),
    ('sx_plain', 'lineblock_continuation', '| A very long line\n  continued here\n| second\n'),
    ('sx_plain', 'lineblock_after_paragraph', 'Intro para.\n\n| line one\n| line two\n'),
    ('sx_plain', 'comment_target_comment_multiline', '.. This is a comment\n   that continues on\n   multiple lines.\n'),
    ('sx_plain', 'comment_target_comment_empty_start', '..\n\n   Indented block attached\n   to an empty comment start.\n'),
    ('sx_plain', 'comment_target_comment_bare', '..\n'),
    ('sx_plain', 'comment_target_comment_weird_colons', '.. just a comment::  with weird colons\n'),
    ('sx_plain', 'comment_target_comment_ragged', '.. first\n      deep\n   shallow\n'),
    ('sx_plain', 'comment_target_comment_adjacent_pair', '.. one\n.. two\n'),
    ('sx_plain', 'review_comment_triple_space', '..   comment text\n'),
    ('sx_plain', 'hardening_target_camel_name', '.. _CamelCase  Name: https://x/\n'),
    ('sx_plain', 'comment_target_target_backtick_name', '.. _`name with: colon`: https://x/\n'),
    ('sx_plain', 'comment_target_target_escaped_colon', '.. _a\\: b: https://y/\n'),
    ('sx_plain', 'comment_target_target_dup_external', '.. _dup: https://1/\n\n.. _dup: https://2/\n'),
    ('sx_plain', 'review_explicit_double_space_target', '..  _t: https://x/\n'),
    ('sx_plain', 'inline_basics_simple_emphasis', 'before *emph* after\n'),
    ('sx_plain', 'inline_basics_three_kinds', '*a* **b** ``c``\n'),
    ('sx_plain', 'inline_basics_word_chars_block', 'a*b*c\n\n2*3*4\n'),
    ('sx_plain', 'inline_basics_punct_after_end', '*emph*. and *emph*-like and *emph*, done\n'),
    ('sx_plain', 'inline_basics_escaped_stars_plain', '\\*not markup\\*\n'),
    ('sx_plain', 'inline_basics_escaped_space_joins', 'one\\ two\n'),
    ('sx_plain', 'inline_basics_markup_spans_lines', '*multi\nline* end\n'),
    ('sx_plain', 'inline_basics_triple_stars', '***x***\n'),
    ('sx_plain', 'inline_basics_first_end_wins', '*word *word*\n'),
    ('sx_plain', 'inline_basics_no_nesting_emphasis', '*a **b** c*\n'),
    ('sx_plain', 'inline_basics_literal_protects_markup', '``*not markup*``\n'),
    ('sx_plain', 'inline_basics_unclosed_emphasis', '*oops\n'),
    ('sx_plain', 'inline_basics_unclosed_strong', '**oops\n'),
    ('sx_plain', 'inline_basics_unclosed_literal', '``oops\n'),
    ('sx_plain', 'inline_basics_double_problematic', '(*emph *nope\n'),
    ('sx_plain', 'inline_basics_emphasis_in_list', '- item *emph* text\n'),
    ('sx_plain', 'inline_carriers_markup_in_title', 'The *Great* Title\n=================\n\nbody\n'),
    ('sx_plain', 'inline_carriers_literal_in_title', 'Using ``code`` Here\n===================\n\nbody\n'),
    ('sx_plain', 'inline_carriers_markup_in_term', '*term* text\n    definition\n'),
    ('sx_plain', 'inline_carriers_markup_in_attribution', 'Para.\n\n    body\n\n    -- *Anon* Author\n'),
    ('sx_plain', 'inline_carriers_markup_in_lineblock', '| plain line\n| *emph* line\n| ``lit`` line\n'),
    ('sx_plain', 'inline_refs_standalone_uris', 'Go to https://x and http://example.com/path?q=1 now.\n'),
    ('sx_plain', 'inline_refs_bare_interpreted', 'See `interpreted` here.\n'),
    ('sx_plain', 'inline_roles_generic_roles', ':emphasis:`text` and :strong:`text` and :literal:`text` end.\n'),
    ('sx_plain', 'inline_roles_sub_sup', 'Water :sub:`2` and x :sup:`2` end.\n'),
    ('sx_plain', 'inline_roles_title_aliases', ':title-reference:`Some Title` :title:`Some Title` :t:`Some Title` end.\n'),
    ('sx_plain', 'inline_roles_abbrev_acronym', ':ab:`St. Nick` and :ac:`NATO` end.\n'),
    ('sx_plain', 'inline_roles_math_role', ':math:`x^2 + y_1` and :math:`a\\\\b` end.\n'),
    ('sx_plain', 'inline_roles_literal_role_escapes', ':literal:`a\\*b` end.\n'),
    ('sx_plain', 'inline_roles_suffix_syntax', '`text`:emphasis: and `text`:strong: end.\n'),
    ('sx_plain', 'fields_basic_mid_document', 'A paragraph first.\n\n:name: value\n:other: thing\n'),
    ('sx_plain', 'tables_grid_minimal_2x2', '+----+----+\n| A  | B  |\n+----+----+\n| C  | D  |\n+----+----+\n'),
    ('sx_plain', 'tables_grid_colwidths', '+---+---------+--+\n| a | bbbbbbb | c|\n+---+---------+--+\n'),
    ('sx_plain', 'tables_grid_header_sep', '+----+----+\n| H1 | H2 |\n+====+====+\n| C  | D  |\n+----+----+\n'),
    ('sx_plain', 'tables_grid_two_header_rows', '+----+----+\n| H1 | H2 |\n+----+----+\n| H3 | H4 |\n+====+====+\n| C  | D  |\n+----+----+\n'),
    ('sx_plain', 'tables_grid_empty_header', '+----+----+\n+====+====+\n| C  | D  |\n+----+----+\n'),
    ('sx_plain', 'tables_grid_column_span', '+----+----+\n| A  | B  |\n+----+----+\n| merged  |\n+----+----+\n'),
    ('sx_plain', 'tables_grid_row_span', '+------+----+\n| span | B  |\n|      +----+\n|      | D  |\n+------+----+\n'),
    ('sx_plain', 'tables_grid_multiline_cell', '+----------+----+\n| Cells may| B  |\n| span.    |    |\n+----------+----+\n'),
    ('sx_plain', 'tables_grid_multi_para_cell', '+-------------+----+\n| para one    | B  |\n|             |    |\n| para two    |    |\n+-------------+----+\n'),
    ('sx_plain', 'tables_grid_list_in_cell', '+----------+----+\n| - item   | B  |\n| - two    |    |\n+----------+----+\n'),
    ('sx_plain', 'tables_grid_empty_cells', '+----+----+\n|    |    |\n+----+----+\n'),
    ('sx_plain', 'tables_grid_borders_only', '+----+----+\n+----+----+\n'),
    ('sx_plain', 'tables_grid_right_border_misaligned', '+----+----+\n| A  | B   |\n+----+----+\n'),
    ('sx_plain', 'tables_grid_short_bottom_border', '+----+----+\n| A  | B  |\n+----+---+\n'),
    ('sx_plain', 'tables_grid_unclosed_table', '+----+----+\n| A  | B  |\n'),
    ('sx_plain', 'tables_grid_nested_indent_in_cell', '+------------+\n|   deep     |\n| shallow    |\n+------------+\n'),
    ('sx_plain', 'tables_grid_table_in_list_item', '- item\n\n  +----+----+\n  | A  | B  |\n  +----+----+\n'),
    ('sx_plain', 'tables_grid_text_after_table', '+----+----+\n| A  | B  |\n+----+----+\n\nafter para\n'),
    ('sx_plain', 'review2_grid_cjk_cells', '+--------+------+\n| 漢字   | col2 |\n+--------+------+\n| x      | y    |\n+--------+------+\n'),
    ('sx_plain', 'tables_simple_basic', '=====  =====\nA      B\nC      D\n=====  =====\n'),
    ('sx_plain', 'tables_simple_header', '=====  =====\nH1     H2\n=====  =====\nA      B\n=====  =====\n'),
    ('sx_plain', 'tables_simple_multiline_row', '=====  =====\nfirst  cell\nmore   text\n-----  -----\nnext   row\n=====  =====\n'),
    ('sx_plain', 'tables_simple_column_span_rule', '=====  =====\nmerged cells\n------------\nA      B\n=====  =====\n'),
    ('sx_plain', 'tables_simple_right_edge_overflow', '=====  =====\nA      B and this extends beyond\n=====  =====\n'),
    ('sx_plain', 'tables_simple_borders_only', '=====  =====\n=====  =====\n'),
    ('sx_plain', 'tables_simple_border_mismatch', '=====  =====\nA      B\n===  ===\n'),
    ('sx_plain', 'tables_simple_margin_text', '=====  =====\nA     xB\n=====  =====\n'),
    ('sx_plain', 'tables_simple_three_columns', '===  ===  ===\na    b    c\nd    e    f\n===  ===  ===\n'),
    ('sx_plain', 'review2_simple_cjk_cells', '=====  =====\ncol 1  col 2\n=====  =====\n漢字   B\n=====  =====\n'),
    ('sx_plain', 'errors_underline_too_short', 'Long Section Title\n======\n'),
    ('sx_plain', 'errors_unexpected_indent', 'line one\nline two\n    Indented without blank line.\n'),
    ('sx_plain', 'errors_nested_transition', 'Para.\n\n    ----\n\n    quoted\n'),
    ('sx_plain', 'errors_nested_title', 'Para.\n\n    Fake\n    ====\n'),
    ('sx_plain', 'hardening_sections_no_blank_between', 'A\n=\nB\n=\n'),
    ('sx_plain', 'hardening_body_adjacent_after_underline', 'Title\n=====\nbody adjacent\n'),
    ('sx_plain', 'hardening_tab_in_literal', 'code::\n\n    a\tb\n'),
    ('sx_plain', 'hardening_lone_double_colon', '::\n'),
    ('sx_plain', 'mixtures_everything_adjacent', 'Head\n====\n\nterm\n    def\n\n- a\n- b\n\n1. one\n2. two\n\n::\n\n    lit\n\n.. done\n'),
    ('sx_plain', 'mixtures_literal_in_list', '- item with code::\n\n      indented code\n\n- next item\n'),
    ('sx_plain', 'mixtures_list_quote_list', '- outer\n\n      quoted in item\n\n  - inner after quote\n'),
    ('sx_plain', 'mixtures_comment_between_paragraphs', 'one\n\n.. hidden note\n\ntwo\n'),
    ('sx_plain', 'mixtures_lineblock_then_list', '| a\n| b\n\n- item\n'),
    ('sx_plain', 'mixtures_tabbed_list', '- item\n\n\tcontinued via tab\n'),
    ('sx_plain', 'target_external_only', '.. _docutils: https://docutils.sourceforge.io/\n\npara\n'),  # new input (not in docutils fixture)
    ('sx_plain', 'two_external_targets', '.. _a: https://x/\n.. _b: https://y/\n\npara here\n'),  # new input (not in docutils fixture)
    # ===== sx_admonitions =====
    ('sx_admonitions', 'dir_admonitions_note_indented_body', '.. note::\n\n   Body text.\n'),
    ('sx_admonitions', 'dir_admonitions_note_inline_content', '.. note:: inline text\n'),
    ('sx_admonitions', 'dir_admonitions_note_inline_plus_body', '.. note:: inline text\n\n   Body.\n'),
    ('sx_admonitions', 'dir_admonitions_note_class_option', '.. note:: inline text\n   :class: foo\n\n   Body.\n'),
    ('sx_admonitions', 'dir_admonitions_note_unknown_option', '.. note::\n   :bogus: x\n\n   Body.\n'),
    ('sx_admonitions', 'dir_admonitions_empty_note_error', '.. note::\n'),
    ('sx_admonitions', 'dir_admonitions_all_admonition_kinds', '.. warning:: w\n\n.. tip:: t\n\n.. danger:: d\n\n.. attention:: a\n'),
    ('sx_admonitions', 'dir_admonitions_generic_admonition', '.. admonition:: Custom Title\n\n   Body text.\n'),
    ('sx_admonitions', 'dir_admonitions_generic_admonition_class', '.. admonition:: T\n   :class: special\n\n   Body.\n'),
    ('sx_admonitions', 'dir_admonitions_generic_missing_arg', '.. admonition::\n\n   Body.\n'),
    ('sx_admonitions', 'dir_admonitions_note_nested_list', '.. note::\n\n   - a\n   - b\n'),
    ('sx_admonitions', 'dir_admonitions_note_named', '.. note::\n   :name: my-note\n\n   Body.\n'),
    ('sx_admonitions', 'dir_admonitions_nested_admonition', '.. note::\n\n   .. warning::\n\n      inner\n'),
    ('sx_admonitions', 'dir_admonitions_directive_no_blank_after', '.. note:: content\nadjacent para\n'),
    ('sx_admonitions', 'dir_options_duplicate_option', '.. note::\n   :class: a\n   :class: b\n\n   Body.\n'),
    ('sx_admonitions', 'dir_options_duplicate_option_mixed_case', '.. note::\n   :Class: a\n   :class: b\n\n   Body.\n'),
    ('sx_admonitions', 'dir_options_multiword_field_name', '.. note::\n   :class extra: v\n\n   Body.\n'),
    ('sx_admonitions', 'dir_options_class_empty_value', '.. note::\n   :class:\n\n   Body.\n'),
    ('sx_admonitions', 'dir_options_name_empty_value', '.. note::\n   :name:\n\n   Body text.\n'),
    ('sx_admonitions', 'dir_options_name_and_class', '.. note::\n   :class: foo bar\n   :name: target one\n\n   Body.\n'),
    ('sx_admonitions', 'dir_options_option_value_continuation', '.. note::\n   :class: foo\n      bar continued\n\n   Body text.\n'),
    ('sx_admonitions', 'dir_options_options_after_blank_are_content', '.. note::\n   :class: foo\n\n   :name: bar\n\n   Body.\n'),
    ('sx_admonitions', 'dir_options_two_blanks_before_content', '.. note::\n   :class: foo\n\n\n   Body after two blank lines.\n'),
    ('sx_admonitions', 'dir_options_malformed_field_marker_to_content', '.. note::\n   :class value\n\n   Body.\n'),
    ('sx_admonitions', 'dir_options_admonition_multiline_title', '.. admonition:: The Title\n   continues here\n\n   Body text.\n'),
    ('sx_admonitions', 'dir_options_admonition_punct_title_class', '.. admonition:: !!!\n\n   Body.\n'),
    ('sx_admonitions', 'dir_options_note_empty_uppercase', '.. NOTE::\n'),
    ('sx_admonitions', 'dir_options_note_marker_line_content_only', '.. note:: This whole line becomes content, not an argument.\n'),
    ('sx_admonitions', 'dir_options_warning_continuation_content', '.. warning:: Danger\n   ahead. This continues the paragraph.\n\n   Second paragraph of warning.\n'),
    ('sx_admonitions', 'dir_options_unexpected_indentation_in_note', '.. note::\n\n   a\n     b\n'),
    ('sx_admonitions', 'dir_options_note_content_unindent_warning', '.. note::\n\n   para\nafter\n'),
    ('sx_admonitions', 'dir_options_consecutive_directives_no_blank', '.. note:: one\n.. note:: two\n'),
    ('sx_admonitions', 'dir_core_no_space_paragraph', '..note::\n\n   Body text.\n'),
    ('sx_admonitions', 'dir_core_single_colon_comment', '.. note:\n\n   Body text.\n'),
    ('sx_admonitions', 'dir_core_two_spaces_comment', '.. note  ::\n\n   Body text.\n'),
    ('sx_admonitions', 'dir_core_one_space_before_colons_ok', '.. note ::\n\n   Body text.\n'),
    ('sx_admonitions', 'dir_core_case_insensitive', '.. NOTE::\n\n   Body text.\n'),
    ('sx_admonitions', 'remaining_kinds', '.. hint:: h\n\n.. important:: i\n\n.. caution:: c\n\n.. error:: e\n'),  # new input (not in docutils fixture)
    # ===== sx_body =====
    ('sx_body', 'dir_body_topic_basic', '.. topic:: Topic Title\n\n   Topic body paragraph.\n'),
    ('sx_body', 'dir_body_topic_no_body', '.. topic:: Topic Title\n'),
    ('sx_body', 'dir_body_topic_in_note', '.. note::\n\n   .. topic:: Inner\n\n      body\n'),
    ('sx_body', 'dir_body_topic_in_list_item', '- item\n\n  .. topic:: Inner\n\n     body\n'),
    ('sx_body', 'dir_body_topic_class_name', '.. topic:: T\n   :class: special\n   :name: my topic\n\n   Body.\n'),
    ('sx_body', 'dir_body_topic_markup_title', '.. topic:: *emphasized* title\n\n   Body.\n'),
    ('sx_body', 'dir_body_sidebar_title_body', '.. sidebar:: Sidebar Title\n\n   Sidebar body.\n'),
    ('sx_body', 'dir_body_sidebar_subtitle', '.. sidebar:: Sidebar Title\n   :subtitle: Sidebar Subtitle\n\n   Sidebar body.\n'),
    ('sx_body', 'dir_body_sidebar_subtitle_no_title', '.. sidebar::\n   :subtitle: A Subtitle\n\n   Body text.\n'),
    ('sx_body', 'dir_body_sidebar_no_title', '.. sidebar::\n\n   Body only.\n'),
    ('sx_body', 'dir_body_sidebar_nested_error', '.. sidebar:: Outer\n\n   Outer body.\n\n   .. sidebar:: Inner\n\n      Inner body.\n'),
    ('sx_body', 'dir_body_topic_in_sidebar', '.. sidebar:: Outer\n\n   .. topic:: Inner Topic\n\n      body\n'),
    ('sx_body', 'dir_body_rubric_minimal', '.. rubric:: This is a rubric\n'),
    ('sx_body', 'dir_body_rubric_options', '.. rubric:: Named rubric\n   :class: myrubricclass\n   :name: rub1\n'),
    ('sx_body', 'dir_body_rubric_markup', '.. rubric:: A *marked up* rubric\n'),
    ('sx_body', 'dir_body_rubric_content_error', '.. rubric:: Title\n\n   body not allowed\n'),
    ('sx_body', 'dir_body_rubric_missing_arg', '.. rubric::\n'),
    ('sx_body', 'dir_body_epigraph_attribution', '.. epigraph::\n\n   Epigraph text.\n\n   -- Attribution\n'),
    ('sx_body', 'dir_body_highlights_basic', '.. highlights::\n\n   Highlighted text.\n'),
    ('sx_body', 'dir_body_pull_quote_basic', '.. pull-quote::\n\n   Pulled text.\n'),
    ('sx_body', 'dir_body_epigraph_empty', '.. epigraph::\n'),
    ('sx_body', 'dir_body_epigraph_marker_line', '.. epigraph:: text on the marker line\n'),
    ('sx_body', 'dir_body_epigraph_unknown_option', '.. epigraph::\n   :class: x\n\n   text\n'),
    ('sx_body', 'dir_body_compound_two_paras', '.. compound::\n\n   First paragraph of compound.\n\n   Second paragraph of compound.\n'),
    ('sx_body', 'dir_body_compound_empty_error', '.. compound::\n'),
    ('sx_body', 'dir_body_compound_class', '.. compound::\n   :class: custom\n\n   Body.\n'),
    ('sx_body', 'dir_body_container_no_class', '.. container::\n\n   Container body.\n'),
    ('sx_body', 'dir_body_container_classes', '.. container:: custom-class another-class\n\n   Container body.\n'),
    ('sx_body', 'dir_body_container_bad_class', '.. container:: !!!\n\n   Body.\n'),
    ('sx_body', 'dir_body_container_named', '.. container:: cls\n   :name: cont\n\n   Body.\n'),
    ('sx_body', 'dir_body_parsed_literal_inline', '.. parsed-literal::\n\n   Text with *emphasis* and **strong** and a\n   `link <http://example.com>`_.\n'),
    ('sx_body', 'dir_body_parsed_literal_class', '.. parsed-literal::\n   :class: code-ish\n   :name: pl1\n\n   plain \\*escaped\\* text\n'),
    # ===== sx_image =====
    ('sx_image', 'dir_image_missing_arg', '.. image::\n'),
    ('sx_image', 'dir_image_content_not_permitted', '.. image:: pic.png\n\n   caption text\n'),
    ('sx_image', 'dir_image_align_vertical_error', '.. image:: pic.png\n   :align: top\n'),
    ('sx_image', 'dir_image_align_invalid_choice', '.. image:: pic.png\n   :align: sideways\n'),
    ('sx_image', 'dir_image_scale_not_number', '.. image:: pic.png\n   :scale: notanumber\n'),
    ('sx_image', 'dir_image_scale_negative', '.. image:: pic.png\n   :scale: -5\n'),
    ('sx_image', 'dir_image_width_banana', '.. image:: pic.png\n   :width: banana\n'),
    ('sx_image', 'dir_image_height_bad_unit', '.. image:: pic.png\n   :height: 10banana\n'),
    ('sx_image', 'dir_image_target_empty', '.. image:: pic.png\n   :target:\n'),
    # ----- wave-3 task 7: Sphinx directives + xref roles -----
    ('sx_directives', 'versionadded_bare', '.. versionadded:: 1.2\n'),
    ('sx_directives', 'versionadded_content', '.. versionadded:: 1.2\n\n   Some explanation text.\n'),
    ('sx_directives', 'versionadded_single_line', '.. versionadded:: 1.2 Available since this release.\n'),
    ('sx_directives', 'versionchanged', '.. versionchanged:: 2.0\n\n   Something changed.\n'),
    ('sx_directives', 'deprecated', '.. deprecated:: 3.0\n\n   Use something else.\n'),
    ('sx_directives', 'versionremoved', '.. versionremoved:: 4.0\n\n   Gone now.\n'),
    ('sx_directives', 'versionadded_markup', '.. versionadded:: 1.2\n\n   Text with *emphasis*.\n'),
    ('sx_directives', 'seealso_block', '.. seealso::\n\n   Some related thing.\n   Second line same paragraph.\n\n   A second paragraph.\n'),
    ('sx_directives', 'seealso_role', '.. seealso:: :doc:`somepage`, Chapter 3\n'),
    ('sx_directives', 'code_block_lang', '.. code-block:: python\n\n   x = 1\n   y = 2\n'),
    ('sx_directives', 'code_block_no_lang', '.. code-block::\n\n   plain text block\n   (no language argument at all)\n'),
    ('sx_directives', 'highlight_then_code_block', '.. highlight:: c\n   :linenothreshold: 5\n\n.. code-block::\n\n   int x = 1;\n'),
    ('sx_directives', 'highlight_bare', '.. highlight:: python\n'),
    ('sx_directives', 'code_block_full_options', '.. code-block:: python\n   :linenos:\n   :emphasize-lines: 2,4-5\n   :caption: example.py\n   :name: mycode\n\n   x = 1\n   y = 2\n   z = 3\n   w = 4\n   v = 5\n'),
    ('sx_directives', 'code_block_name_only', '.. code-block:: python\n   :name: mycode2\n\n   x = 1\n'),
    ('sx_directives', 'only_simple', '.. only:: html\n\n   HTML only content.\n'),
    ('sx_directives', 'only_expr', '.. only:: html and not epub\n\n   Complex expr content.\n'),
    ('sx_directives', 'rst_class', 'Title\n=====\n\n.. rst-class:: myclass otherclass\n\nParagraph after.\n'),
    ('sx_directives', 'toctree_bare_entries', '.. toctree::\n   :maxdepth: 2\n\n   installation\n   Linked Title <other>\n'),
    ('sx_roles', 'doc_role', 'See :doc:`somepage` here.\n'),
    ('sx_roles', 'doc_role_explicit_title', 'See :doc:`The Guide <somepage>` here.\n'),
    ('sx_roles', 'ref_role', 'See :ref:`Some Label` here.\n'),
    ('sx_roles', 'func_role', 'Call :func:`mymod.myfunc` now.\n'),
    ('sx_roles', 'func_role_tilde', 'Call :func:`~mymod.myfunc` now.\n'),
    ('sx_roles', 'domain_qualified_role', 'Call :py:meth:`obj.method` now.\n'),
]


def make_app(base: Path) -> SphinxTestApp:
    if base.exists():
        shutil.rmtree(base)
    base.mkdir(parents=True)
    (base / "conf.py").write_text(CONF_PY, encoding="utf-8")
    (base / "index.rst").write_text("Placeholder\n===========\n", encoding="utf-8")
    return SphinxTestApp(
        buildername="dummy",
        srcdir=base,
        status=io.StringIO(),
        warning=io.StringIO(),
        confoverrides=dict(CONFOVERRIDES),
    )


def probe(app: SphinxTestApp, base: Path, rst_text: str, docname: str = "index"):
    """harness3 (probes doc): parse one snippet exactly like Builder.read_doc."""
    env = app.env
    env.clear_doc(docname)
    env.ref_context.clear()
    env.prepare_settings(docname)
    parser = RSTParser()
    parser._config = app.config
    parser._env = env
    filename = (
        env.doc2path(docname)
        if docname in app.project.docnames
        else base / f"{docname}.rst"
    )
    doctree = _parse_str_to_doctree(
        rst_text,
        filename=Path(filename),
        default_settings=env.settings,
        env=env,
        events=app.events,
        parser=parser,
        transforms=app.registry.get_transforms(),
    )
    env.current_document.docname = ""
    return doctree


def normalize(pseudo_xml: str, base: Path) -> str:
    text = pseudo_xml.replace(str(base / "index.rst"), SOURCE_TOKEN)
    assert str(base) not in text, f"srcdir path leaked into pseudo_xml:\n{text}"
    text = text.replace(TP_ATTR, "")
    assert "translation_progress" not in text, (
        f"unexpected translation_progress form:\n{text}"
    )
    return text


def check_effective_settings(app: SphinxTestApp, doctree) -> dict:
    """Assert the settings combination this fixture claims, and return the
    header record. Guards against a future Sphinx/docutils default shifting
    silently underneath the harness. NOTE: smartquotes is a Sphinx CONFIG
    gate (SphinxSmartQuotes.is_available checks config.smartquotes); the
    docutils settings.smart_quotes value stays True and is intentionally
    not what we assert."""
    s = doctree.settings
    effective = {
        "report_level": s.report_level,
        "halt_level": s.halt_level,
        "auto_id_prefix": s.auto_id_prefix,
        "id_prefix": s.id_prefix,
        "language": s.language_code,
        "smartquotes": bool(app.config.smartquotes),
        "doctitle_xform": bool(s.doctitle_xform),
        "sectsubtitle_xform": bool(s.sectsubtitle_xform),
    }
    expected = {
        "report_level": 2,
        "halt_level": 5,
        "auto_id_prefix": "id",
        "id_prefix": "",
        "language": "en",
        "smartquotes": False,
        "doctitle_xform": False,
        "sectsubtitle_xform": False,
    }
    assert effective == expected, f"settings drifted: {effective} != {expected}"
    effective["keep_warnings"] = True
    effective["extensions"] = []
    effective["docname"] = "index"
    return effective


def main() -> int:
    names = [f"{family}.{name}" for family, name, _ in CASES]
    assert len(names) == len(set(names)), "family-qualified case names must be unique"
    assert len(CASES) >= 40, f"corpus degenerated: {len(CASES)} cases"

    floors = {
        "sx_plain": 15,
        "sx_admonitions": 10,
        "sx_body": 30,
        "sx_image": 6,
        "sx_directives": 18,
        "sx_roles": 6,
    }
    counts: dict = {}
    for family, _, _ in CASES:
        counts[family] = counts.get(family, 0) + 1
    assert set(counts) == set(floors), f"unexpected families: {sorted(counts)}"
    for family, floor in floors.items():
        assert counts.get(family, 0) >= floor, (
            f"family {family}: {counts.get(family, 0)} < floor {floor}"
        )

    # resolve(): on macOS mkdtemp returns /var/... while Sphinx resolves the
    # srcdir to /private/var/...; the path-normalization replace must match.
    base = Path(tempfile.mkdtemp(prefix="sphinx_oracle_srcdir_")).resolve() / "src"

    with docutils_namespace(), patch_docutils(str(base)):
        app = make_app(base)
        try:
            settings_record = check_effective_settings(app, probe(app, base, "sanity\n"))

            out_cases = []
            bad = []
            for family, name, rst in CASES:
                doctree = probe(app, base, rst)
                stray = {node.tagname for node in doctree.findall()} - SUPPORTED_KINDS
                if stray:
                    bad.append(f"{family}.{name}: unsupported kinds {sorted(stray)}")
                    continue
                pseudo = normalize(doctree.pformat(), base)
                assert pseudo.startswith(f'<document source="{SOURCE_TOKEN}">\n'), (
                    f"{family}.{name}: unexpected document start tag:\n{pseudo}"
                )
                out_cases.append(
                    {
                        "name": f"{family}.{name}",
                        "family": family,
                        "rst": rst,
                        "pseudo_xml": pseudo,
                    }
                )
            if bad:
                print("CORPUS SCOPE VIOLATIONS:", file=sys.stderr)
                for b in bad:
                    print(f"  {b}", file=sys.stderr)
                return 1

            # In-process determinism check: a second full pass over the corpus
            # must be byte-identical (catches cross-case env leakage).
            for case in out_cases:
                again = normalize(probe(app, base, case["rst"]).pformat(), base)
                assert again == case["pseudo_xml"], (
                    f"{case['name']}: second parse differs (cross-case state leak?)\n"
                    f"--- first ---\n{case['pseudo_xml']}\n--- second ---\n{again}"
                )
        finally:
            app.cleanup()
            shutil.rmtree(base.parent, ignore_errors=True)

    fixture = {
        "docutils_version": docutils.__version__,
        "sphinx_version": sphinx.__version__,
        "generator": "tools/gen_sphinx_fixture.py",
        "harness": (
            "sphinx.util.docutils._parse_str_to_doctree with env.settings + "
            "registry transforms (probes-doc 'harness3'; byte-identical to a "
            "full dummy-builder build + env.get_doctree)"
        ),
        "settings": settings_record,
        "normalizations": [
            f"srcdir index.rst absolute path -> {SOURCE_TOKEN}",
            "document translation_progress attribute stripped "
            "(i18n.TranslationProgressTotaliser artifact)",
        ],
        "cases": out_cases,
    }
    out_path = (
        Path(__file__).resolve().parent.parent
        / "tests"
        / "fixtures"
        / "sphinx_doctree_differential.json"
    )
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(fixture, f, indent=2, sort_keys=True, ensure_ascii=False)
        f.write("\n")
    print(
        f"wrote {out_path}: {len(out_cases)} cases, "
        f"sphinx {sphinx.__version__}, docutils {docutils.__version__}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
