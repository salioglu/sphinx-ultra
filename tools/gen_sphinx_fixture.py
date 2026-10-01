#!/usr/bin/env python3
"""Generate tests/fixtures/sphinx_doctree_differential.json from Sphinx 9.1.0.

Regenerate with:

    PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' \
        --with 'docutils==0.22.4' python tools/gen_sphinx_fixture.py

PYTHONNOUSERSITE=1 is NOT optional: `uv run` keeps the user's site-packages
on sys.path, and a user-site Pygments there silently re-records every
`code:: python` case as tokenized output. Regenerating without the flag
produces spurious fixture churn.

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

ORACLE VENUE NOTE (wave 4.5): the `include`/`literalinclude` directives are
NOT exercised by this corpus and must not be added to it -- every case here
is a single rst string against a fixed one-file srcdir, and file-inserting
directives need aux files beside the document. Their oracle venue is the
env fixture (tools/gen_env_fixture.py projects, which ship real member
files) plus unit/e2e tests.

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
    finding. Since M2 wave 5 the consumer runs the same filter
    (sphinx_ultra::transforms), so INFO-emitting snippets are admitted:
    family tx_filter)
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
Sphinx oracle and the crate's parse layer FOLLOWED BY its read-transform pass
(`sphinx_ultra::transforms::parse_and_transform`, which
tests/sphinx_doctree_differential.rs runs under this fixture's
keep_warnings=True / smartquotes=False), in its tree and in its printed
records, hence zero-divergence against the Rust pipeline. (Until M2 wave 5 the
bar was the bare wave-1/2/3 docutils parse layer: the crate ran no transform.)
Candidate cases whose tree a read transform the crate does not run yet
changes are EXCLUDED; the wave-3 recon report (sphinx-harness-report.md)
documents the enumerated divergences with full diffs: INFO stripping,
PropagateTargets/IndirectHyperlinks/ExternalTargets/AnonymousHyperlinks target
rewrites, Footnotes+FootnoteDocnameUpdater, DoctestTransform classes,
doc-start docinfo consumption, image `candidates`, Transitions edge warnings,
Sphinx role replacements for pep/rfc/code/index. Each wave-5 transform task
re-admits the constructs of the transform it lands, in a `tx_*` family of its
own: `tx_filter` (FilterSystemMessages -- INFO stripping) first, then
`tx_targets` (MoveModuleTargets, ReorderConsecutiveTargetAndIndexNodes,
PropagateTargets, SortIds -- which also re-admitted the propagation-visible
`py.module_basic`/`py.duplicate_modules` from EXCLUDED), then `tx_subst`
(DefaultSubstitutions, docutils Substitutions), then `tx_links`
(AnonymousHyperlinks, IndirectHyperlinks, ExternalTargets, InternalTargets,
SphinxDanglingReferences -- every `refname` reference), then `tx_footnotes`
(the citation transforms, Footnotes, UnreferencedFootnotesDetector,
FootnoteDocnameUpdater -- every footnote and citation). Later tasks
extend this corpus with Sphinx-specific directives (toctree, code-block,
versionadded/versionchanged/deprecated, seealso, only, highlight, math, index,
rst-class, ...) once the Rust side grows the sphinx registry + env surface.

Wave-4 task 9 tried to add a sphinx-mode `.. figure::` case (to pin where the
`:name:` id lands, which the docutils-mode fixture already covers as
`dir_media.figure_name_option`). It is EXCLUDED by the policy above: a figure
must contain an `image`, and `ImageCollector.process_doc` stamps every image
with `candidates="{'*': 'pic.png'}"` — one of the enumerated excluded
divergences. Verified by hand against the oracle in that task: sphinx-mode
output for `.. figure:: pic.png` + `:name: myfig` is `<figure ids="myfig"
names="myfig">`, byte-identical to ours apart from that one attribute (an
unnamed figure additionally picks up `ids="id1"` from Sphinx's `AutoNumbering`
transform, which this crate does not run). src/rst/block.rs's
`a_figure_name_lands_on_the_image_in_docutils_and_the_figure_in_sphinx` pins
the id placement until the image-collection task can fold the case in here.

Wave-4 task 9 also left `ObjectDescription`'s `parse_content_to_nodes(
allow_section_headings=True)` (`directives/__init__.py:288`) out of the corpus.
That flag is docutils' `nested_parse(match_titles=True)`, which makes a section
title inside a description body open a real `section` AND lifts the
`BasePseudoSection` guard, so `.. topic::`/`.. sidebar::` are legal there. This
crate's nested parse is `match_titles=False` throughout, so both come back as
`Unexpected section title.` / `The "topic" directive may not be used within
topics or body elements.` — verified against the oracle in that task. Threading
a real `match_titles` through the nested parse is a change to the section
machinery itself, not to these directives, so it is deferred with the two
probe cases removed rather than committed as a knowingly-red corpus.

Provenance: cases whose (family, name) mirror a case of
tests/fixtures/doctree_differential.json reuse that case's exact rst input;
three inputs are new (marked). Never remove or rename existing cases; later
waves only EXTEND the corpus and SUPPORTED_KINDS.

PER-CASE WARNINGS (M2 wave 5, reporter channel): every case records
`warnings`, the records the read phase printed for the snippet, in the order
it printed them — docutils reporter messages (written when the Reporter
CREATES them, `docutils/utils/__init__.py:213-215`, re-logged by Sphinx's
`WarningStream`, `sphinx/util/docutils.py:385-393`) interleaved with the
directives' and domains' own `logger.warning` calls, then whatever the read
transforms log. The app's warning stream is a per-write recorder
(`StreamHandler.emit` writes each record in one call, message + '\n');
each record keeps its full printed form minus that terminator, with the
same `<snippet>` path normalization as the pseudo-XML (the doubled
`<snippet>.rst` of a tuple `location=` is Sphinx's own `doc2path` output
and stays). Only the FIRST parse of a case is recorded; the determinism
re-parse must print the same records.

PER-CASE CONFOVERRIDES (wave-4.5 task 8): a case tuple may carry a fourth
element, a dict of confoverrides applied ON TOP of the fixed CONFOVERRIDES
base (smartquotes/keep_warnings are never overridden per-case). One
SphinxTestApp is constructed per DISTINCT conf dict (cases grouped by their
JSON-serialized conf, mirroring the [SIG] appendix probe scripts) so fifty
conf cases do not spin fifty apps; the base settings assertions run against
every app. The fixture schema emits "conf" on a case ONLY when non-empty —
absent means defaults — and the Rust consumer maps every conf key onto
ParseOptions.py (PySigConfig), ERRORING on unmapped keys so a future conf
addition here fails loudly there instead of silently parsing under defaults.

WAVE-4.5 EXCLUSIONS (py-domain corpus; every entry in EXCLUDED below carries
its reason and the assert keeps CASES disjoint from it — see that dict).

SOURCE_DATE_EPOCH PIN (M2 wave 5, Task 8): `|today|` with an empty `today`
is Sphinx's `format_date(today_fmt or '%b %d, %Y', language=...)`
(`sphinx/transforms/__init__.py:130-135`), which reads the build date from
$SOURCE_DATE_EPOCH when it is set and from the clock otherwise
(`sphinx/util/i18n.py:271-280`). This module sets
SOURCE_DATE_EPOCH=1234567890 (2009-02-13 23:31:30 UTC) for the whole run, at
import, so every case that substitutes `|today|` (tx_subst.today_fmt,
tx_subst.today_default_format) records the same text on every regeneration.
The pin is recorded in the fixture header (`settings.source_date_epoch`), and
the Rust consumer hands the same instant to the transform pass
(`TransformConfig.build_date`). No other recorded output reads the variable:
the pin was verified inert for every case that predates it (extend-only
regeneration, byte-identical).
"""

import io
import json
import os
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

# The build date `|today|` formats (module docstring, "SOURCE_DATE_EPOCH PIN").
SOURCE_DATE_EPOCH = 1234567890
os.environ["SOURCE_DATE_EPOCH"] = str(SOURCE_DATE_EPOCH)

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
    "math_block",
    "index",
    "hlist",
    "hlistcol",
    "glossary",
    # wave-4 task 9: std-domain object directives + generic desc anatomy
    "desc",
    "desc_signature",
    "desc_name",
    "desc_addname",
    "desc_content",
    # wave-4.5 task 8: py-domain signatures, annotations and doc fields
    "desc_parameterlist",
    "desc_parameter",
    "desc_optional",
    "desc_returns",
    "desc_annotation",
    "desc_type_parameter_list",
    "desc_type_parameter",
    "desc_sig_name",
    "desc_sig_operator",
    "desc_sig_punctuation",
    "desc_sig_space",
    "desc_sig_keyword",
    "desc_sig_keyword_type",
    "desc_sig_literal_number",
    "desc_sig_literal_string",
    "desc_sig_literal_char",
    "literal_strong",
    "literal_emphasis",
    "pending_xref_condition",
    # M2 wave 5, Task 8: the definitions Substitutions leaves in the tree
    # (`Invisible`; the writers skip them).
    "substitution_definition",
    # M2 wave 5, Task 9: an option list ending a document (where the
    # anonymous-mismatch error is located).
    "option_list",
    "option_list_item",
    "option_group",
    "option",
    "option_string",
    "description",
    # M2 wave 5, Task 10: footnotes and citations, which the transforms
    # number, label and link (citation references are `pending_xref`s by
    # then).
    "footnote",
    "footnote_reference",
    "citation",
    "label",
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
    ('sx_directives', 'math_labeled', '.. math::\n   :label: eq1\n\n   E = mc^2\n'),
    ('sx_directives', 'math_unlabeled', '.. math::\n\n   a + b\n'),
    ('sx_directives', 'math_nowrap', '.. math::\n   :nowrap:\n\n   x\n'),
    ('sx_directives', 'math_marker_arg', '.. math:: E = mc^2\n'),
    ('sx_directives', 'index_single', '.. index:: single: MyTerm\n'),
    ('sx_directives', 'index_pair', '.. index:: pair: MyTerm; OtherTerm\n'),
    ('sx_directives', 'index_bare', '.. index:: MyTerm\n'),
    ('sx_directives', 'index_comma_main', '.. index:: foo, bar, !baz\n'),
    ('sx_directives', 'hlist_columns', '.. hlist::\n   :columns: 3\n\n   * a\n   * b\n   * c\n   * d\n   * e\n'),
    ('sx_directives', 'hlist_default', '.. hlist::\n\n   * one\n   * two\n   * three\n'),
    ('sx_directives', 'glossary_basic', '.. glossary::\n\n   environment\n      A structure where information about all documents under the root is\n      saved.\n\n   source directory\n      The directory which holds all source files.\n'),
    ('sx_directives', 'glossary_multi_term', '.. glossary::\n\n   term a\n   term b\n      Shared definition.\n'),
    ('sx_directives', 'glossary_case_and_underscores', '.. glossary::\n\n   HTTP_Method\n      A method.\n'),
    ('sx_directives', 'glossary_unusable_term_text', '.. glossary::\n\n   !!!\n      Punctuation only.\n\n   ???\n      More punctuation.\n'),
    ('sx_directives', 'glossary_serial_is_not_the_index_serial', '.. glossary::\n\n   !!!\n      Punctuation only.\n\n.. index:: Something\n'),
    ('sx_directives', 'glossary_term_with_markup', '.. glossary::\n\n   *emphasized* term\n      A def.\n'),
    ('sx_directives', 'glossary_sorted_classifier', '.. glossary::\n   :sorted:\n\n   zeta : key\n      Z def.\n'),
    # panel fix round A: `GlossarySorter` (priority 500) really reorders the
    # definition list, keyed on `unicodedata.normalize('NFD', term.lower())`.
    # >= 3 terms, mixed case, and a non-ASCII term so the NFD/lower key is
    # pinned rather than a plain byte sort.
    ('sx_directives', 'glossary_sorted_reorders_terms', '.. glossary::\n   :sorted:\n\n   zeta\n      Z def.\n\n   Alpha\n      A def.\n\n   \u00e9clair\n      E def.\n\n   beta\n      B def.\n'),
    # Without `:sorted:` the source order stands.
    ('sx_directives', 'glossary_unsorted_keeps_source_order', '.. glossary::\n\n   zeta\n      Z def.\n\n   Alpha\n      A def.\n'),
    ('sx_directives', 'glossary_comment_lines', '.. glossary::\n\n   .. a comment line\n   alpha\n      The first letter.\n\n   .. a comment line\n   beta\n      The second letter.\n'),
    ('sx_directives', 'glossary_comment_swallows_its_continuation', '.. glossary::\n\n   .. a comment line\n      continued under the comment\n\n   alpha\n      The first letter.\n'),
    # Wave-4.5 task 16: the three `Glossary.run` misformat warnings
    # (`domains/std/__init__.py:461-503`). They are reporter warnings, so
    # they land in the tree as system_message nodes BEFORE the glossary
    # node, and they are reported ONE LINE LOW (0-based `content.items`
    # offset rendered as a 1-based line) -- both of which these cases pin.
    ('sx_directives', 'glossary_term_without_preceding_blank_line', '.. glossary::\n\n   term A\n      def A\n   term B\n      def B\n'),
    ('sx_directives', 'glossary_terms_separated_by_empty_line', '.. glossary::\n\n   term A\n\n   term B\n      def AB\n'),
    ('sx_directives', 'glossary_terms_separated_by_empty_lines_twice', '.. glossary::\n\n   term A\n\n   term B\n\n   term C\n      def\n'),
    ('sx_directives', 'glossary_misformatted_indentation', '.. glossary::\n\n      stray indented line\n\n   term A\n      def A\n'),
    ('sx_directives', 'glossary_comment_does_not_split_multi_term', '.. glossary::\n\n   term A\n   .. a comment\n   term B\n      shared def\n'),
    ('sx_directives', 'glossary_comment_after_definition_warns', '.. glossary::\n\n   term A\n      def A\n   .. comment\n   term B\n      def B\n'),
    ('sx_directives', 'glossary_definition_dedents_by_its_first_line', '.. glossary::\n\n   term A\n         deep def\n      shallow\n'),
    # ... and `line[indent_len:]` (`:501`) slices CHARACTERS, not bytes:
    # the first case's offset falls inside a two-byte 'e-acute' and the
    # second keeps one character too many under a byte-count slice.
    ('sx_directives', 'glossary_definition_dedent_splits_no_multibyte_char', '.. glossary::\n\n   term A\n      deep\n     éx\n'),
    ('sx_directives', 'glossary_definition_dedent_counts_characters', '.. glossary::\n\n   term A\n       deep\n     ébcdef\n'),
    ('sx_roles', 'pep_role', 'See :pep:`8` for style.\n'),
    ('sx_roles', 'pep_role_anchor', 'See :pep:`8#imports` here.\n'),
    ('sx_roles', 'pep_role_explicit', 'See :pep:`the style guide <8>` here.\n'),
    ('sx_roles', 'rfc_role', 'See :rfc:`2324` for details.\n'),
    ('sx_roles', 'rfc_role_section', 'See :rfc:`2324#section-5.1` here.\n'),
    ('sx_roles', 'cve_role', 'See :cve:`2020-10735` here.\n'),
    ('sx_roles', 'cwe_role', 'See :cwe:`787` here.\n'),
    ('sx_directives', 'code_block_emphasize_invalid', '.. code-block:: python\n   :emphasize-lines: 5-3\n\n   x = 1\n'),
    ('sx_directives', 'code_block_emphasize_open_range', '.. code-block:: python\n   :emphasize-lines: 2-\n\n   a\n   b\n   c\n'),
    ('sx_directives', 'code_block_emphasize_out_of_range', '.. code-block:: python\n   :emphasize-lines: 1,99\n\n   a\n   b\n'),
    ('sx_directives', 'toctree_bare_angle_entry', '.. toctree::\n\n   <foo>\n'),
    # ----- wave-4 task 9: std-domain object directives -----
    # `describe`/`object` are registered with the BASE ObjectDescription
    # (sphinx/directives/__init__.py:375-377), whose handle_signature raises
    # and whose add_target_and_index is `pass`: desc anatomy but no ids, no
    # index entries, no std objects.
    ('sx_std', 'describe_plain', '.. describe:: widget\n\n   A generic described object.\n'),
    ('sx_std', 'describe_no_content', '.. describe:: widget\n'),
    ('sx_std', 'object_plain', '.. object:: thing\n\n   Body of the object.\n'),
    ('sx_std', 'envvar_plain', '.. envvar:: HOME_A\n\n   Home directory variable.\n'),
    ('sx_std', 'envvar_no_index', '.. envvar:: HOME_B\n   :no-index:\n\n   Not registered.\n'),
    ('sx_std', 'confval_plain', '.. confval:: my_setting\n\n   A config value.\n'),
    ('sx_std', 'confval_typed', '.. confval:: my_setting\n   :type: ``str``\n   :default: ``\'x\'``\n\n   A config value.\n'),
    ('sx_std', 'confval_type_only', '.. confval:: other_setting\n   :type: text with *emphasis*\n'),
    ('sx_std', 'option_no_program', '.. option:: --global-opt\n\n   A global (unscoped) option.\n'),
    ('sx_std', 'option_with_program', '.. program:: myprog\n\n.. option:: --verbose\n\n   Enables verbose output.\n'),
    ('sx_std', 'program_none_pop', '.. program:: myprog\n\n.. option:: --scoped\n\n.. program:: None\n\n.. option:: --unscoped\n'),
    ('sx_std', 'program_whitespace_name', '.. program:: my prog\n\n.. option:: --opt\n'),
    ('sx_std', 'option_malformed', '.. option:: =bad\n\n   Body.\n'),
    ('sx_std', 'option_multiple_names', '.. option:: -f, --file\n\n   Two spellings.\n'),
    ('sx_std', 'option_with_args', '.. option:: --output=FILE\n\n   Writes to FILE.\n'),
    ('sx_std', 'option_positional_arg', '.. option:: filename\n\n   A positional argument.\n'),
    ('sx_std', 'option_bracketed_value', '.. option:: --color[=WHEN]\n'),
    ('sx_std', 'option_multi_signature', '.. option:: --one\n            --two\n\n   Two signatures.\n'),
    ('sx_std', 'confval_no_typesetting', '.. confval:: quiet_setting\n   :no-typesetting:\n\n   Body.\n'),
    ('sx_std', 'describe_no_typesetting', '.. describe:: widget\n   :no-typesetting:\n\n   Body.\n'),
    ('sx_std', 'cmdoption_alias', '.. cmdoption:: --legacy\n\n   The old directive name.\n'),
    ('sx_std', 'option_duplicate_signature', '.. option:: --dup\n            --dup\n\n   Same name twice.\n'),
    ('sx_std', 'envvar_deprecated_noindex', '.. envvar:: HOME_C\n   :noindex:\n\n   Old spelling of the flag.\n'),
    ('sx_std', 'default_domain', '.. default-domain:: py\n\nText after the default-domain.\n'),
    ('sx_roles', 'envvar_role', 'See :envvar:`HOME_A` for details.\n'),
    ('sx_roles', 'envvar_role_explicit_title', 'See :envvar:`the home dir <HOME_A>` here.\n'),
    ('sx_roles', 'option_role', 'Use :option:`--verbose` now.\n'),
    ('sx_roles', 'option_role_in_program_scope', '.. program:: myprog\n\nUse :option:`--verbose` now.\n'),
    ('sx_roles', 'confval_role', 'See :confval:`my_setting` here.\n'),
    # panel fix round A: the BASE `XRefRole.process_link` collapses every
    # whitespace run in the target (`roles.py:165`, `ws_re.sub(' ', target)`),
    # so every role that does not override it without calling super does too.
    ('sx_roles', 'xref_target_whitespace_collapsed', 'See :term:`foo  bar` and :doc:`some  page` and :envvar:`FOO  BAR`.\n'),
    ('sx_roles', 'xref_target_wrapped_across_lines', 'See :term:`foo\nbar` here.\n'),
    # ... and the two std roles whose overrides skip super keep the run.
    ('sx_roles', 'xref_target_no_collapse_token_option', 'See :token:`a  b` and :option:`-x  y`.\n'),
    # panel fix round C: `ws_re` is Python's `\s` == `str.isspace`, which
    # admits \x1c-\x1f; \x1f is the one `splitlines` does not eat first.
    ('sx_roles', 'xref_target_python_whitespace_collapsed', 'See :doc:`a\x1fb` and :term:`x\x1fy` here.\n'),
    # ... an explicit title's target is taken VERBATIM between the brackets
    # (`explicit_title_re`), so the padding survives every role's
    # `process_link` — collapsed, never stripped; the title loses its
    # trailing `\s*`.
    ('sx_roles', 'xref_explicit_target_padding_kept', 'See :term:`x < foo   bar >` and :doc:`t < a  b >` and :any:`x < p  q >` and :ref:`r < l >` here.\n'),
    # ... `:ref:`/`:numref:` are `lowercase=True` XRefRoles and nothing
    # more: `target.lower()` then `ws_re.sub`, NOT docutils'
    # `fully_normalize_name` (which would strip the padding as well).
    ('sx_roles', 'xref_ref_target_lowercased_but_not_stripped', 'See :ref:`r < L abc >` and :numref:`n < X  y >` here.\n'),
    # ... and a padded py target keeps its `()` through
    # `update_title_and_target` (the target does not END with them).
    ('sx_roles', 'xref_explicit_target_padding_kept_py', 'See :py:func:`x < f() >` and :func:`y\x1f<g>` here.\n'),
    # ... `:eq:` is the math domain's, registered domainless like `:any:`
    # (`app.add_role('eq', MathReferenceRole(warn_dangling=True))`):
    # refdomain="math", classes "xref eq", refwarn=1.
    ('sx_roles', 'eq_role_is_the_math_domains', 'See :eq:`euler` and :eq:`the one <euler>` and :eq:`a  b` here.\n'),
    # panel fix round D: the std-domain NAME sites run the same `ws_re`
    # (`ws_re.sub(' ', sig)` for envvar/confval, `ws_re.sub('-', …)` for
    # program), so \x1f collapses in the index entry, `fullname`/toc name
    # and the option's program scope.
    ('sx_std', 'envvar_python_whitespace_name', '.. envvar:: FOO\x1fBAR\n\n   Body.\n'),
    ('sx_std', 'confval_python_whitespace_name', '.. confval:: FOO\x1fBAR\n\n   Body.\n'),
    ('sx_std', 'program_python_whitespace_name', '.. program:: git\x1fadd\n\n.. option:: -x\n\n   Body.\n'),
    # ... and on the docutils side a label NAME is `' '.join(name.split())`,
    # Python's `str.split()`: `.. _a\x1fb:` is the label `a b`, which both
    # spellings of the `:ref:` reach. Each bare target is followed by a
    # comment so `PropagateTargets` has nothing to donate to.
    ('sx_roles', 'ref_label_python_whitespace_name', 'See :ref:`a\x1fb` and :ref:`A <a b>` here.\n\n.. _a\x1fb:\n.. _ a b :\n'),
    # ... `.. _ pad  lbl :` (space after `_`) is a COMMENT under docutils'
    # `\.\.[ ]+_(?![ ]|$)`, so beside the real label there is no
    # `Duplicate explicit target name` message and no second name.
    ('sx_roles', 'target_leading_space_is_a_comment', 'See :ref:`P <pad lbl>` here.\n\n.. _pad  lbl:\n.. _ pad  lbl :\n'),
    # ----- wave-4.5 task 8: std-desc doc fields (T7 fix round 1) -----
    # DocFieldTransformer runs for EVERY object description; std kinds use
    # the empty typemap, so every field takes the unknown branch (renamed,
    # body untouched).
    ('sx_std', 'envvar_param_field', '.. envvar:: SIMPLE\n\n   :param x: thing\n'),
    ('sx_std', 'envvar_meta_private_field', '.. envvar:: METAV\n\n   :meta private:\n'),
    # T7 re-review quirk: markup in the field name duplicates as raw text +
    # inline children inside the renamed field_name ("Param em x" AND the
    # <emphasis> pair).
    ('sx_std', 'envvar_param_markup_field_name', '.. envvar:: EMPH\n\n   :param *em* x: body\n'),
    # A system_message inside a field BODY passes through the transform
    # untouched. (The one-child system_message as a field_list CHILD crashes
    # real sphinx — EXCLUDED sx_std.confval_bad_type_markup — and a
    # two-child one is not expressible through parseable rst: block-level
    # errors land inside field_body, as here.)
    ('sx_std', 'envvar_field_body_system_message', '.. envvar:: SM\n\n   :param x: text\n     bad\n       indent\n'),
    # ----- round E: the remaining Python-`str.split()` sites -----
    # `option_desc_re = r'((?:/|--|-|\+)?[^\s=]+)(=?\s*.*)'` — Python `\s`,
    # so \x1f ENDS the option name and `y` becomes its argument.
    ('sx_std', 'option_name_python_whitespace', '.. program:: git\n\n.. option:: -x\x1fy\n\n   Body.\n'),
    # `field_name.astext().split(None, maxsplit=1)` (docfields.py:384-389)
    # and `fieldarg.rsplit(None, 1)` (:448-455) split the same way.
    ('sx_std', 'envvar_field_name_python_whitespace', '.. envvar:: WS\n\n   :param\x1fx: thing\n'),
    # `Element.starttag` renders a list item as `'%s' % (v,)` — the tuple's
    # `repr`, which escapes every non-`str.isprintable()` character.
    ('sx_directives', 'index_entry_repr_escapes_control_char', '.. index:: single: a\x1fb\n'),
    ('sx_directives', 'index_entry_repr_escapes_nbsp', '.. index:: single: a\xa0b\n'),
    ('sx_directives', 'index_entry_repr_escapes_del', '.. index:: single: a\x7fb\n'),
    ('sx_directives', 'glossary_index_entry_repr_escapes_control_char', '.. glossary::\n\n   term\x1fone\n      definition\n'),
    # ----- round F: the glossary term is NOT rstripped -----
    # `split_term_classifiers` (domains/std/__init__.py:366-372) splits the
    # line on ` +: +` and takes `parts[0]` VERBATIM — unlike docutils'
    # `Text.term`, which rstrips it — so whitespace before the delimiter
    # stays in the <term>, its index entry and the registered term; the
    # first classifier (`parts[1]`) is verbatim too, and only the first one
    # is used.
    ('sx_directives', 'glossary_term_nbsp_before_classifier_kept', '.. glossary::\n\n   term\xa0 : cls\n      def\n'),
    ('sx_directives', 'glossary_term_us_before_classifier_kept', '.. glossary::\n\n   term\x1f : cls\n      def\n'),
    ('sx_directives', 'glossary_wide_classifier_delimiter', '.. glossary::\n\n   term  :  cls\n      def\n'),
    ('sx_directives', 'glossary_classifier_leading_nbsp_kept', '.. glossary::\n\n   term : \xa0cls\n      def\n'),
    ('sx_directives', 'glossary_second_classifier_ignored_first_kept', '.. glossary::\n\n   term : a : b\n      def\n'),
    # round F: `process_index_entry` (util/nodes.py) `strip()`s the entry and
    # every comma value and `lstrip()`s after a leading `!` — Python's set, so
    # `\x1f` goes wherever a space would. `!` is recognised only at the very
    # start of an entry (the `single: !x` form keeps its `!`).
    ('sx_directives', 'index_bang_then_us_lstripped', '.. index:: !\x1fa\n'),
    ('sx_directives', 'index_single_leading_us_stripped', '.. index:: single: \x1fa\n'),
    ('sx_directives', 'index_single_bang_after_type_kept', '.. index:: single: !\x1fa\n'),
    ('sx_directives', 'index_comma_value_leading_us_stripped', '.. index:: a,\x1fb\n'),
    ('sx_directives', 'index_comma_bang_then_us_lstripped', '.. index:: a, !\x1fb\n'),
    ('sx_directives', 'index_pair_leading_us_stripped', '.. index:: pair: \x1fa; b\n'),
    ('sx_directives', 'index_value_trailing_us_before_comma_stripped', '.. index:: a\x1f, b\n'),
    # round F: `Cmdoption.handle_signature` `strip()`s every `, `-separated
    # synonym (Python's set) before `option_desc_re`; `parse_directive_arguments`'
    # `split(None, 0)` lstrips only the whole argument, so a \x1f opening the
    # SECOND synonym reaches that strip.
    ('sx_std', 'option_second_synonym_leading_us_stripped', '.. program:: git\n\n.. option:: -x, \x1f-y\n\n   Body.\n'),
    # round F: `parselinenos` — `begend = part.strip().split('-')` — strips each
    # comma part with Python's set BEFORE `int()` (which would reject a \x1f).
    ('sx_directives', 'code_block_emphasize_lines_leading_us_stripped', '.. code-block:: python\n   :emphasize-lines: \x1f1\n\n   x = 1\n'),
    ('sx_directives', 'code_block_emphasize_lines_part_trailing_us_stripped', '.. code-block:: python\n   :emphasize-lines: 1\x1f,2\n\n   x = 1\n   y = 2\n'),
    # `parse_directive_arguments` re-splits with `arg_text.split(None,
    # required + optional - 1)` when there are too many words — Python
    # whitespace both times, so \x1f ends the version argument.
    ('sx_directives', 'versionadded_argument_python_whitespace', '.. versionadded:: 1.0\x1fa b\n'),
    ('sx_directives', 'deprecated_argument_python_whitespace', '.. deprecated:: 2.0\x1fx y\n'),
    # ----- wave-4.5 task 8: py-domain object directives ([PY §1.6/1.7]) -----
    # Propagation-visible module shapes and known-divergence signatures are
    # in EXCLUDED above, each with its reason.
    ('py', 'function_plain_args', '.. py:function:: func(a, b)\n\n   Body.\n'),
    # round E: `:param type name:` splits with `rsplit(None, 1)` — Python
    # whitespace, so \x1f separates the type from the name.
    ('py', 'param_type_name_python_whitespace', '.. py:function:: f(x)\n\n   :param int\x1fx: the thing\n'),
    ('py', 'function_full_markers', '.. py:function:: mymod.func(a, b=1, *args, c: int = 2, **kwargs) -> str\n'),
    ('py', 'function_posonly', '.. py:function:: func(a, /, b, *, c)\n'),
    ('py', 'function_posonly_trailing', '.. py:function:: func(a, /)\n'),
    ('py', 'function_brackets_fallback', '.. py:function:: func(a[, b])\n'),
    ('py', 'function_default_str', ".. py:function:: func(name='x', items=[])\n"),
    ('py', 'function_no_arglist', '.. py:function:: func\n'),
    ('py', 'function_async', '.. py:function:: coro(x)\n   :async:\n'),
    ('py', 'function_module_option', '.. py:function:: f(x)\n   :module: optmod\n'),
    ('py', 'function_annotation_option', '.. py:function:: f(x)\n   :annotation: something extra\n'),
    ('py', 'function_bad_sig', '.. py:function:: not a signature!\n'),
    ('py', 'function_multi_sig', '.. py:function:: f(x)\n                  g(y)\n\n   Shared body.\n'),
    # round F: `get_signatures` `strip()`s each signature line (Python's set),
    # and `_filter_meta_fields` `strip()`s the field name before the `meta`
    # test — a \x1f opening either is gone.
    ('py', 'function_second_signature_leading_us_stripped', '.. py:function:: f()\n                  \x1fg()\n'),
    ('py', 'function_meta_field_leading_us_filtered', '.. py:function:: f()\n\n   :\x1fmeta private:\n'),
    ('py', 'module_deprecated', '.. py:module:: oldmod\n   :deprecated:\n'),
    ('py', 'module_noindex', '.. py:module:: quietmod\n   :no-index:\n'),
    ('py', 'module_noindexentry', '.. py:module:: halfmod\n   :no-index-entry:\n'),
    ('py', 'module_notypesetting_inert', '.. py:module:: ntmod\n   :no-typesetting:\n'),
    ('py', 'module_bad_option', '.. py:module:: badmod\n   :noindexentry:\n'),
    ('py', 'currentmodule_function', '.. py:currentmodule:: curmod\n\n.. py:function:: f(x)\n'),
    ('py', 'currentmodule_none_pops', '.. py:currentmodule:: curmod\n\n.. py:currentmodule:: None\n\n.. py:function:: f(x)\n'),
    ('py', 'class_with_bases', '.. py:class:: MyClass(Base1, Base2)\n\n   .. py:method:: meth(self, arg)\n\n      Body.\n'),
    ('py', 'method_options', '.. py:class:: C\n\n   .. py:method:: m1(x)\n      :classmethod:\n\n   .. py:method:: m2(x)\n      :staticmethod:\n\n   .. py:method:: m3(x)\n      :abstractmethod:\n      :async:\n      :final:\n'),
    ('py', 'classmethod_staticmethod_directives', '.. py:class:: C\n\n   .. py:classmethod:: cm(x)\n\n   .. py:staticmethod:: sm(x)\n'),
    ('py', 'attribute_typed', '.. py:class:: C\n\n   .. py:attribute:: attr\n      :type: int\n      :value: 42\n'),
    ('py', 'property_typed', '.. py:class:: C\n\n   .. py:property:: prop\n      :type: str\n      :abstractmethod:\n      :classmethod:\n'),
    ('py', 'data_typed', '.. py:data:: CONST\n   :type: dict[str, int]\n   :value: {}\n'),
    ('py', 'decorator_basic', '.. py:decorator:: mydeco\n'),
    ('py', 'decorator_with_args', '.. py:decorator:: mydeco(flag)\n'),
    ('py', 'decoratormethod_basic', '.. py:class:: C\n\n   .. py:decoratormethod:: dm\n'),
    ('py', 'type_alias_canonical', '.. py:type:: MyAlias\n   :canonical: list[int]\n'),
    ('py', 'exception_basic', '.. py:exception:: MyError\n'),
    ('py', 'nested_classes', '.. py:class:: Outer\n\n   .. py:class:: Inner\n\n      .. py:method:: m(x)\n'),
    ('py', 'method_class_prefix_given', '.. py:class:: C\n\n   .. py:method:: C.meth(x)\n'),
    ('py', 'method_other_prefix', '.. py:class:: C\n\n   .. py:method:: D.meth(x)\n'),
    ('py', 'function_fields', '.. py:function:: f(a, b)\n\n   :param int a: first\n   :param b: second\n   :type b: str\n   :returns: something\n   :rtype: bool\n   :raises ValueError: when bad\n'),
    ('py', 'function_meta_field', '.. py:function:: f()\n\n   :meta private:\n'),
    ('py', 'noindex_function', '.. py:function:: hidden()\n   :no-index:\n'),
    ('py', 'old_noindex_spelling', '.. py:function:: hidden()\n   :noindex:\n'),
    ('py', 'noindexentry_function', '.. py:function:: quiet()\n   :no-index-entry:\n'),
    ('py', 'nocontentsentry_function', '.. py:function:: quiet2()\n   :no-contents-entry:\n'),
    ('py', 'notypesetting_function', '.. py:function:: invisible()\n   :no-typesetting:\n'),
    ('py', 'canonical_function', '.. py:function:: new_name()\n   :canonical: old.name\n'),
    ('py', 'duplicate_functions', '.. py:function:: dup()\n\n.. py:function:: dup()\n'),
    # tp-list/arglist warning spellings (T6 row 13): the WARNING bytes live
    # in the logger (pinned by src/rst/block.rs arglist_and_tp_list_error_
    # paths_warn); the fixture pins the fallback TREE shape for exactly the
    # three probed spellings — other tp failures render exception text this
    # crate does not reproduce byte-for-byte.
    ('py', 'arglist_duplicate_param', '.. py:function:: f(a, a)\n'),
    ('py', 'tp_list_variadic_bound', '.. py:function:: f[*Ts: int](x)\n'),
    ('py', 'tp_list_tokenerror', '.. py:function:: f[(T](x)\n'),
    # read-phase xref roles ([PY §3.1]; @ on BOTH deco titles, T6 ledger 1)
    ('py', 'roles_basic', 'See :py:func:`target` and :py:func:`target()` and :py:func:`custom <target>`.\n'),
    ('py', 'roles_tilde_dot', 'See :py:meth:`~pkg.Cls.meth` and :py:meth:`.Cls.meth` and :py:mod:`pkg`.\n'),
    ('py', 'role_deco_implicit_and_explicit', 'See :py:deco:`mydeco` and :py:deco:`custom <mydeco>`.\n'),
    ('py', 'role_lstrip_edges', 'See :py:func:`..target` and :py:func:`~~pkg.f` and :py:class:`custom <.Cls>`.\n'),
    ('py', 'role_in_currentmodule_scope', '.. py:currentmodule:: rmod\n\nSee :py:func:`local` here.\n'),
    ('py', 'role_in_class_scope', '.. py:class:: C\n\n   See :py:meth:`m` here.\n'),
    # An unqualified role name resolves against `primary_domain` (py) first,
    # and `type` is a py role with no std counterpart.
    ('py', 'role_type_unqualified', '.. py:type:: MyAlias\n\nA :type:`MyAlias` here.\n'),
    # `after_content` ASSIGNS `modules.pop()`, so a `:module:` option with no
    # enclosing module scope leaves `py:module` present holding None — which
    # `AnyXRefRole`'s ref_context copy renders as the "True" sentinel.
    ('py', 'any_after_module_option', '.. py:function:: f()\n   :module: mymod\n\n   Body.\n\nAfter :any:`x`.\n'),
    # `.. py:currentmodule:: None` pops the key instead, so nothing is stamped.
    ('py', 'any_after_currentmodule_none', '.. py:currentmodule:: m\n\n.. py:currentmodule:: None\n\nAfter :any:`x`.\n'),
    # ----- wave-4.5 task 8: signature/annotation parsing shapes ([PY §2]) -----
    # Default configuration; the annotation grammar, arglist channels and
    # PEP-695 type parameter lists.
    ('pysig', 'union_pipe', '.. py:function:: f(x: int | None) -> int | str\n'),
    ('pysig', 'optional_union_rewrite', '.. py:function:: f(x: Optional[int], y: Union[int, str])\n'),
    ('pysig', 'subscript_generics', '.. py:function:: f(x: list[str], y: dict[str, int]) -> list[str]\n'),
    ('pysig', 'tilde_annotation', '.. py:function:: f(x: ~mymod.MyClass)\n'),
    ('pysig', 'dot_annotation_refspecific', '.. py:function:: f(x: .MyClass)\n'),
    ('pysig', 'string_annotation_literal', ".. py:function:: f(x: 'MyClass')\n"),
    ('pysig', 'typing_prefix_and_none', '.. py:function:: f(x: typing.Any, y: None)\n'),
    ('pysig', 'tuple_ellipsis', '.. py:function:: f(x: tuple[int, ...])\n'),
    ('pysig', 'literal_annotation_default_conf', ".. py:function:: f(x: Literal['a', 'b'] = 'a')\n"),
    ('pysig', 'typeparams_full', '.. py:class:: C[T, *Ts, **P]\n'),
    ('pysig', 'typeparams_constraint', '.. py:function:: f[T: (int, str)](x: T) -> T\n'),
    ('pysig', 'typeparams_default', '.. py:function:: f[T = int](x)\n'),
    ('pysig', 'pseudo_default_eq_shape', '.. py:function:: f(a[, b=1])\n'),
    ('pysig', 'pseudo_bracket_imbalance', '.. py:function:: f(a[, b)\n'),
    ('pysig', 'varargs_then_keyword', '.. py:function:: f(*args, k=1)\n'),
    ('pysig', 'negative_none_defaults', '.. py:function:: f(x=-1, y=None)\n'),
    ('pysig', 'annotated_default_spacing', '.. py:function:: f(x: int = 2, y=3)\n'),
    ('pysig', 'backslash_in_arglist_default', '.. py:function:: f(a\\, b)\n'),
    # ----- panel fix round A: exec-mode annotations, numeric source -----
    # ----- recovery order, and BoolOp defaults -----
    # PEP 646. `_parse_annotation` parses in EXEC mode, so `*Ts` is a legal
    # `Expr(Starred(...))` statement; CPython's `star_annotation` production
    # makes `*args` the only parameter slot that can carry one.
    ('pysig', 'star_annotation_pep646', '.. py:function:: f(*args: *Ts)\n'),
    ('pysig', 'star_annotation_bracketed_unpack', '.. py:function:: f(*args: *tuple[int, ...])\n'),
    ('pysig', 'star_annotation_neighbours', '.. py:function:: f(a, *args: *Ts, b)\n'),
    ('pysig', 'star_annotation_retann', '.. py:function:: f() -> *Ts\n'),
    # `visit_Constant` recovers numeric source text by AST position, so a
    # call's callee keeps its own spellings even when the arguments carry
    # numbers too.
    ('pysig', 'chained_call_numeric_default', '.. py:function:: f(x=a(0x10).b(16))\n'),
    ('pysig', 'called_call_numeric_default', '.. py:function:: f(x=g(0xFF)(255))\n'),
    ('pysig', 'octal_chain_numeric_default', '.. py:function:: f(x=P(0o755).mask(0o022))\n'),
    # `sphinx.pycode.ast` has a first-class `visit_BoolOp`, so `and`/`or`
    # defaults take the AST path (separators keep their `abbreviation`).
    # `_parse_annotation`'s walk has no BoolOp branch, so the same operator
    # in an ANNOTATION still falls back to one whole-text xref.
    ('pysig', 'boolop_default_keyword_only', '.. py:function:: f(a, *, x=A or B)\n'),
    ('pysig', 'boolop_default_positional_only', '.. py:function:: f(a=A and B, /)\n'),
    ('pysig', 'boolop_default_mixed_chain', '.. py:function:: f(x=a and b or c)\n'),
    ('pysig', 'boolop_annotation_falls_back', '.. py:function:: f(x: a or b)\n'),
    # PEP 695 empty bound: `_parse_annotation('')` is the empty node list,
    # so `if not annotation: continue` drops the whole type parameter.
    ('pysig', 'empty_type_param_bound', '.. py:function:: f[T:](x)\n'),
    ('pysig', 'empty_type_param_bound_default', '.. py:function:: f[T: = int](x)\n'),
    ('pysig', 'empty_type_param_bound_sibling', '.. py:function:: f[T:, U](x)\n'),
    # ----- wave-4.5 task 8: signature-config family ([SIG] A/B/C/D/E + -----
    # ----- U/L/F-U/P matrices as per-case confoverrides) -----
    ('pyconf', 'wrap_equal_no_flip', '.. py:function:: foo(aaaa)\n', {'maximum_signature_line_length': 9}),
    ('pyconf', 'wrap_over_flips', '.. py:function:: foo(aaaa)\n', {'maximum_signature_line_length': 8}),
    ('pyconf', 'wrap_retann_counts', '.. py:function:: foo(a) -> int\n', {'maximum_signature_line_length': 12}),
    ('pyconf', 'wrap_retann_equal_no_flip', '.. py:function:: foo(a) -> int\n', {'maximum_signature_line_length': 13}),
    ('pyconf', 'wrap_prefix_counts', '.. py:function:: Klass.foo(a)\n', {'maximum_signature_line_length': 11}),
    ('pyconf', 'wrap_whitespace_stripped', '.. py:function::    foo(aaaa)   \n', {'maximum_signature_line_length': 9}),
    ('pyconf', 'wrap_tp_span_subtracted', '.. py:function:: foo[T](aaaa)\n', {'maximum_signature_line_length': 10}),
    ('pyconf', 'wrap_both_flip', '.. py:function:: foo[T](aaaa)\n', {'maximum_signature_line_length': 7}),
    ('pyconf', 'wrap_neither_flips', '.. py:function:: foo[T](aaaa)\n', {'maximum_signature_line_length': 11}),
    ('pyconf', 'wrap_python_key_wins_high', '.. py:function:: foo(aaaa)\n', {'python_maximum_signature_line_length': 1000, 'maximum_signature_line_length': 1}),
    ('pyconf', 'wrap_python_key_wins_low', '.. py:function:: foo(aaaa)\n', {'python_maximum_signature_line_length': 1, 'maximum_signature_line_length': 1000}),
    ('pyconf', 'wrap_global_fallback', '.. py:function:: foo(aaaa)\n', {'maximum_signature_line_length': 1}),
    ('pyconf', 'wrap_falsy_zero_fallthrough', '.. py:function:: foo(aaaa)\n', {'python_maximum_signature_line_length': 0, 'maximum_signature_line_length': 1}),
    ('pyconf', 'wrap_defaults_never_flip', '.. py:function:: foo(aaaa)\n'),
    ('pyconf', 'single_line_parameter_list_option', '.. py:function:: foo[T](aaaa)\n   :single-line-parameter-list:\n', {'maximum_signature_line_length': 1}),
    ('pyconf', 'single_line_type_parameter_list_option', '.. py:function:: foo[T](aaaa)\n   :single-line-type-parameter-list:\n', {'maximum_signature_line_length': 1}),
    ('pyconf', 'trailing_comma_attr_both_lists', '.. py:function:: foo[T](aaaa)\n', {'maximum_signature_line_length': 1}),
    ('pyconf', 'trailing_comma_off', '.. py:function:: foo[T](aaaa)\n', {'maximum_signature_line_length': 1, 'python_trailing_comma_in_multi_line_signatures': False}),
    ('pyconf', 'annotation_qualified_default', '.. py:function:: f(x: pkg.Cls) -> pkg.Cls\n'),
    ('pyconf', 'unqualified_type_names', '.. py:function:: f(x: pkg.Cls) -> pkg.Cls\n', {'python_use_unqualified_type_names': True}),
    ('pyconf', 'short_literal_types', ".. py:function:: f(x: Literal['a', 'b'] = 'a')\n", {'python_display_short_literal_types': True}),
    ('pyconf', 'field_unqualified_type_names', '.. py:function:: f(x)\n\n   :param x: thing\n   :type x: pkg.Cls\n', {'python_use_unqualified_type_names': True}),
    # T2 ledger closure: the add_function_parentheses seam pinned at the
    # full parse_rst level (BlockParser -> inline roles), not just in the
    # role unit tests.
    ('pyconf', 'func_role_parens_off', 'Call :py:func:`mymod.myfunc` now.\n', {'add_function_parentheses': False}),
    ('pyconf', 'func_role_written_parens_removed', 'Call :py:func:`mymod.myfunc()` now.\n', {'add_function_parentheses': False}),
    ('pyconf', 'index_entry_parens_invariant', '.. py:function:: myfunc(x)\n\n   Body.\n', {'add_function_parentheses': False}),
    ('pyconf', 'toc_show_parents_hide', '.. py:class:: C\n\n   .. py:method:: m(x)\n', {'toc_object_entries_show_parents': 'hide'}),
    ('pyconf', 'toc_show_parents_all', '.. py:class:: C\n\n   .. py:method:: m(x)\n', {'toc_object_entries_show_parents': 'all'}),
    ('pyconf', 'toc_object_entries_off', '.. py:function:: f(x)\n', {'toc_object_entries': False}),
    # The `finally` in `ObjectDescription.run` gates `_toc_parts`/`_toc_name`
    # for EVERY object description, std kinds included.
    ('pyconf', 'toc_object_entries_off_confval', '.. confval:: myopt\n', {'toc_object_entries': False}),
    ('pyconf', 'toc_object_entries_on_confval', '.. confval:: myopt\n'),
    ('pyconf', 'toc_object_entries_off_envvar', '.. envvar:: MYVAR\n', {'toc_object_entries': False}),
    ('pyconf', 'add_module_names_off', '.. py:function:: f(x)\n   :module: optmod\n', {'add_module_names': False}),
    ('pyconf', 'strip_signature_backslash_on', '.. py:function:: f(a\\, b)\n', {'strip_signature_backslash': True}),
    # M2 wave 5 (reporter channel): cases pinning the per-case `warnings`
    # stream where it parts from a walk of the tree. A `:caption:` is a
    # throwaway nested parse at the directive's content offset
    # (`container_wrapper`, `code.py:78-96`): its unknown-directive ERROR is
    # printed there although only the `Invalid caption` warning survives.
    ('sx_directives', 'code_block_caption_throwaway_parse_prints', '.. code-block:: python\n   :caption: .. foo::\n\n   x = 1\n'),
    ('sx_directives', 'code_block_caption_throwaway_parse_later_line', 'Para.\n\nMore.\n\n.. code-block::\n   :caption: .. foo::\n\n   x = 1\n'),
    # `parse_inline(self.arguments[1], lineno=self.lineno + 1)`.
    ('sx_directives', 'versionadded_argument_messages_one_line_down', '.. versionadded:: 1.0 *x\n'),
    # Reporter and logger records interleave in creation order.
    ('sx_directives', 'toctree_warnings_interleave_with_reporter', 'Title\n====\n\n.. toctree::\n\n   missing\n\n*x\n\n.. toctree::\n\n   gone\n'),
    ('sx_directives', 'toctree_glob_matching_nothing', '.. toctree::\n   :glob:\n\n   nope*\n'),
    # Glossary terms parse at the content item's 0-based offset.
    ('sx_std', 'glossary_term_messages_one_line_up', '.. glossary::\n\n   term\n   *x\n      def\n'),
    ('sx_std', 'option_malformed_then_body_message', '.. option:: =bad\n\n   *x\n'),
    ('py', 'arglist_warning_then_body_message', '.. py:function:: f(a, a)\n\n   *x\n'),
    # ===== tx_filter (M2 wave 5, sub-project 1, Task 6) =====
    # FilterSystemMessages (`transforms/__init__.py:337-347`, priority 999)
    # under this fixture's keep_warnings=True filters below level 2: the
    # parse's INFO `Duplicate implicit target name` message (placed in the
    # second section after its title) is gone from Sphinx's tree, and INFO
    # never prints either. Formerly excluded by the corpus policy (INFO
    # stripping).
    ('tx_filter', 'info_message_stripped', 'Dup\n===\n\nx\n\nDup\n===\n\ny\n'),
    # ===== tx_targets (M2 wave 5, sub-project 1, Task 7) =====
    # The target transforms: MoveModuleTargets (210,
    # `sphinx/transforms/__init__.py:153-175`),
    # ReorderConsecutiveTargetAndIndexNodes (220, `:446-515`), docutils
    # PropagateTargets (260, `transforms/references.py:17-95`) and SortIds
    # (261, `:217-225`). Formerly excluded by the corpus policy
    # (PropagateTargets target rewrites).
    ('tx_targets', 'block_target_then_paragraph', '.. _t:\n\npara\n'),
    # `a` donates to the target `b` first, then `b` ("b a") to the paragraph.
    ('tx_targets', 'chained_targets', '.. _a:\n.. _b:\n\npara\n'),
    # `t1` is a document child before the first section; `t2` is the last
    # child of section "First", so next_node(ascend=True) climbs to "Second".
    ('tx_targets', 'target_then_section', '.. _t1:\n\nFirst\n=====\n\npara\n\n.. _t2:\n\nSecond\n======\n\ntext\n'),
    # SortIds: the propagated label follows the section's own `identity`,
    # which starts with "id", so it is rotated to the end.
    ('tx_targets', 'identity_section_label', '.. _lbl:\n\nIdentity\n========\n'),
    ('tx_targets', 'target_index_reorder', '.. _t:\n.. index:: x\n\npara\n'),
    ('tx_targets', 'module_target_moves_to_section', 'Mod\n===\n\n.. py:module:: mymod\n\ntext\n'),
    # U+00A0 is Python whitespace (`str.split()` in `fully_normalize_name`).
    ('tx_targets', 'target_name_nbsp', '.. _a\u00a0b:\n\npara\n'),
    # The edges of the propagation predicate and of the two sphinx
    # transforms' position tests (new inputs):
    # without an index entry the module target is at index 1, so
    # MoveModuleTargets leaves it and PropagateTargets moves it on.
    ('tx_targets', 'module_target_without_index_entry_propagates', 'Mod\n===\n\n.. py:module:: halfmod\n   :no-index-entry:\n\ntext\n'),
    # A section whose auto id is `id1` (non-ASCII title) is sorted too.
    ('tx_targets', 'non_ascii_section_label', '.. _lbl2:\n\n\u65e5\u672c\n====\n'),
    # The unknown directive's messages are stepped over, not into.
    ('tx_targets', 'target_skips_system_messages', '.. _t:\n\n.. nosuchdirective::\n\npara\n'),
    # An Invisible next node (a comment; an `index` outside the target's
    # own run) takes nothing, and neither does the end of the document.
    ('tx_targets', 'target_then_comment_stays', '.. _t:\n\n.. a comment\n\npara\n'),
    ('tx_targets', 'target_then_index_elsewhere_stays', '- item\n\n  .. _t:\n\n.. index:: x\n\npara\n'),
    ('tx_targets', 'target_at_document_end_stays', 'para\n\n.. _t:\n'),
    # A target whose parent is a TextElement is skipped: here the `:envvar:`
    # role's childless index target inside a line block's `line`.
    ('tx_targets', 'target_inside_a_line_stays', '| See :envvar:`HOME_A`.\n'),
    # Every index node of a run moves ahead of every target (a stable sort),
    # and the chain then carries all four ids to the paragraph.
    ('tx_targets', 'two_target_index_runs', '.. _a:\n.. index:: x\n.. _b:\n.. index:: y\n\npara\n'),
    # MoveModuleTargets walks a snapshot: once `ma`'s target leaves index 2,
    # `mb`'s (no index entry) moves up into it and is absorbed too, ahead.
    ('tx_targets', 'two_module_targets_move_in_order', 'Mod\n===\n\n.. py:module:: ma\n.. py:module:: mb\n   :no-index-entry:\n\ntext\n'),
    # Re-admitted from EXCLUDED: the module-target shapes (inputs as pinned by
    # src/rst/block.rs's module_basic_pre_propagation_shape and
    # duplicate_modules_take_the_module_0_serial).
    ('py', 'module_basic', '.. py:module:: mymod\n   :synopsis: A module.\n   :platform: Unix\n\n.. py:function:: f(x)\n\n   Body.\n'),
    ('py', 'duplicate_modules', '.. py:module:: dupmod\n\n.. py:module:: dupmod\n'),
    # ===== tx_subst (M2 wave 5, sub-project 1, Task 8) =====
    # Sphinx's DefaultSubstitutions (210, `sphinx/transforms/__init__.py:
    # 111-150`) and docutils' Substitutions (220, `transforms/references.py:
    # 642-764`). Formerly unreachable: SUPPORTED_KINDS lacked the
    # `substitution_definition` every definition leaves in the tree.
    ('tx_subst', 'replace', '.. |name| replace:: replacement *text*\n\nSee |name| here.\n'),
    ('tx_subst', 'unicode', '.. |copy| unicode:: 0xA9 .. copyright sign\n\n|copy| 2026\n'),
    # The reference comes first, so the copy of `a` still holds `|b|`: the
    # nested reference joins the worklist and is expanded after the others.
    ('tx_subst', 'nested', 'x |a| y\n\n.. |a| replace:: A |b| A\n.. |b| replace:: B\n'),
    # Both error texts: the nested copy of `|b|` is "referenced" (located at
    # the paragraph its chain of `ref-origin`s starts from), then each
    # definition is replaced by a "detected" message holding its source.
    ('tx_subst', 'circular', 'See |a| here.\n\n.. |a| replace:: x |b|\n.. |b| replace:: y |a|\n'),
    ('tx_subst', 'undefined', 'See |undef| here.\n'),
    ('tx_subst', 'case_insensitive_fallback', '.. |Name| replace:: value\n\n|name| and |NAME| and |Name|\n'),
    ('tx_subst', 'default_version_release', '|version| and |release|\n', {'version': '1.2', 'release': '1.2.3'}),
    ('tx_subst', 'default_today_fixed', '|today|\n', {'today': 'Sept 30'}),
    # SOURCE_DATE_EPOCH is pinned (module docstring).
    ('tx_subst', 'today_fmt', '|today|\n', {'today_fmt': '%Y'}),
    ('tx_subst', 'doc_definition_wins', '.. |version| replace:: mine\n\n|version|\n'),
    # U+00A0 is Python whitespace: both spellings name the definition "a b".
    ('tx_subst', 'name_nbsp', '.. |a b| replace:: nb\n\n|a b| and |a b|\n'),
    ('tx_subst', 'translation_progress', '|translation progress|\n'),
    # `b` grows to 11 * 1000 + 10 characters as its own references expand,
    # past docutils' 10000 `line_length_limit`; the error has no node, so it
    # is located one past the document's last line (line 5 of 4).
    ('tx_subst', 'expansion_exceeds_line_length_limit', '.. |a| replace:: ' + 'x' * 1000 + '\n.. |b| replace:: ' + ' '.join(['|a|'] * 11) + '\n\nSee |b| here.\n'),
    # The edges of the two transforms (new inputs):
    # `:trim:` strips the Python whitespace of the Text on either side.
    ('tx_subst', 'trim', 'a |x| b\n\n.. |x| unicode:: U+2014\n   :trim:\n'),
    # The definition's own reference fails first; `|a|` then deep-copies
    # the `problematic`, ids and all (`ids="id2"` twice).
    ('tx_subst', 'undefined_inside_definition_is_copied', '.. |a| replace:: x |nope|\n\n|a|\n'),
    ('tx_subst', 'circular_definitions_first', '.. |a| replace:: x |b|\n.. |b| replace:: y |a|\n\nSee |a| here.\n'),
    # DefaultSubstitutions walks the definitions too.
    ('tx_subst', 'default_inside_definition', '.. |v| replace:: v |version|\n\nx |v|\n', {'version': '1.2'}),
    # DefaultSubstitutions matches names exactly (`|version|` is not defined
    # here, so it takes the empty default); Substitutions falls back to the
    # case-insensitive name (`|VERSION|` finds `Version`).
    ('tx_subst', 'default_names_are_exact', '.. |Version| replace:: mine\n\n|version| |VERSION|\n'),
    # `today_fmt` unset: `'%b %d, %Y'` of the pinned date.
    ('tx_subst', 'today_default_format', '|today|\n'),
    # Definition names differing only in case (review, Task 8 fix round 2):
    # a reference expands the definition it names exactly, but docutils'
    # circularity test files it under the case-folded name's (last)
    # definition, so the test fires late, or never and the line-length
    # limit ends the expansion. docutils finishes all three.
    ('tx_subst', 'case_clash_circular_order', '.. |B| replace:: |B|\n.. |b| replace:: |B|\n\nSee |b|.\n'),
    ('tx_subst', 'case_clash_growth_ends_at_line_length_limit', '.. |a| replace:: |A| |a|\n.. |A| replace:: x\n\nSee |A|.\n'),
    ('tx_subst', 'case_clash_cycle_cut_by_folded_definition', '.. |A| replace:: |A|\n\nSee |A|.\n\n.. |a| replace:: |a|\n'),
    # The line-length error's no-node location when the document ends in
    # explicit markup (M2 wave 5, Task 9): docutils' top-level cursor has
    # been moved past the end of the input by the explicit list's nested
    # parse, so the message has no line at all (`<snippet>::`).
    ('tx_subst', 'expansion_exceeds_line_length_limit_ending_in_explicit_markup', '.. |a| replace:: ' + 'x' * 1000 + '\n.. |b| replace:: ' + ' '.join(['|a|'] * 11) + '\n\nSee |b| here.\n\n.. end comment\n'),
    # ===== tx_links (M2 wave 5, sub-project 1, Task 9) =====
    # docutils' AnonymousHyperlinks (440), IndirectHyperlinks (460),
    # ExternalTargets (640), InternalTargets (660) and Sphinx's
    # SphinxDanglingReferences (850) (`docutils/transforms/references.py:
    # 98-411,878-990`, `sphinx/transforms/references.py:18-30`). Formerly
    # excluded: every `refname` reference is resolved or replaced by them.
    ('tx_links', 'named_external', 'See `ext`_ here.\n\n.. _ext: https://example.com\n'),
    # `sec_` names a propagated target and `Section`_ the section's implicit
    # name: InternalTargets skips both (the target has a `refid` by now, the
    # section is no target) and DanglingReferences resolves them by name.
    ('tx_links', 'named_internal', 'See sec_ and `Section`_.\n\n.. _sec:\n\nSection\n=======\n\ntext\n'),
    # An external anonymous target, a propagated one (followed to the
    # paragraph it moved onto) and an indirect one (`refid` first, then
    # rewritten by IndirectHyperlinks through `document.refids`).
    ('tx_links', 'anonymous_pair', 'Anonymous `one`__ and `two`__ and `three`__.\n\n__ https://one.example\n\n.. __:\n\nPara.\n\n__ two_\n\n.. _two: https://two.example\n'),
    # The mismatch error has no node: the document ends in explicit markup,
    # so docutils' cursor is past the end and the record has no line.
    ('tx_links', 'anonymous_mismatch', 'A `x`__ and `y`__.\n\n__ https://only.example\n'),
    ('tx_links', 'indirect_chain', 'See `a`_ and `d`_.\n\n.. _a: b_\n.. _b: c_\n.. _c: https://example.com\n.. _d: e_\n.. _e:\n\nPara.\n'),
    # The error names `a`; `b` itself is in `refnames['a']`, so it is
    # replaced by a `problematic` too, and `b`_ still resolves (to `a`).
    ('tx_links', 'indirect_circular', 'See `a`_ and `b`_.\n\n.. _a: b_\n.. _b: a_\n'),
    ('tx_links', 'indirect_unknown', 'See `ind`_.\n\n.. _ind: missing_\n'),
    ('tx_links', 'embedded_uri', '`Python <https://python.org>`_ and `alias <python_>`_ and python_.\n'),
    ('tx_links', 'duplicate_target_reference', 'See `d`_.\n\n.. _d: https://1\n.. _d: https://2\n'),
    # Two unknown names, one with the embedded-reference hint; the
    # unreferenced target's INFO is suppressed by SphinxDanglingReferences.
    ('tx_links', 'dangling_reference', 'See `nope`_ and `a<b`_ here.\n\n.. _unused:\n\nSection\n=======\n\ntext\n'),
    # U+00A0 and U+001F are Python whitespace: each name normalizes with a
    # plain space, in the reference and in the target alike.
    ('tx_links', 'name_nbsp', 'See `a\u00a0b`_ and `c\u001fd`_.\n\n.. _a\u00a0b: https://nbsp.example\n.. _c\u001fd: https://us.example\n'),
    ('tx_links', 'anonymous_mismatch_in_nested_directive', 'A `x`__.\n\n.. note::\n\n   y\n'),
    # The edges (new inputs). Where the mismatch is located, by how the
    # document ends: one past the last line, or (after a construct whose
    # nested parse ran to the end of the input) no line.
    ('tx_links', 'anonymous_mismatch_ends_with_paragraph', 'A `x`__ and `y`__.\n\n__ https://only.example\n\nEnd.\n'),
    ('tx_links', 'anonymous_mismatch_ends_with_bullet_list', 'A `x`__.\n\n- item\n'),
    ('tx_links', 'anonymous_mismatch_ends_with_enumerated_list', 'A `x`__.\n\n#. one\n'),
    ('tx_links', 'anonymous_mismatch_ends_with_field_list', 'A `x`__.\n\n:f: v\n'),
    ('tx_links', 'anonymous_mismatch_ends_with_option_list', 'A `x`__.\n\n-o  opt\n'),
    ('tx_links', 'anonymous_mismatch_ends_with_definition_list', 'A `x`__.\n\nterm\n   def\n'),
    ('tx_links', 'anonymous_mismatch_ends_with_line_block', 'A `x`__.\n\n| one\n| two\n'),
    # A one-line line block is blank-finished: no nested parse.
    ('tx_links', 'anonymous_mismatch_ends_with_one_line_block', 'A `x`__.\n\n| one\n'),
    ('tx_links', 'anonymous_mismatch_ends_with_quoted_literal', 'A `x`__.\n\n::\n\n> quoted\n'),
    # A blank line ends the quoted block's nested parse early (`blank`
    # raises EOFError there), and the top level eats it: one past the end.
    ('tx_links', 'anonymous_mismatch_ends_with_quoted_literal_and_blank', 'A `x`__.\n\n::\n\n> quoted\n\n'),
    # `::` with nothing after it: the empty quoted parse steps back a line
    # (`previous_line`), leaving the cursor ON the last line.
    ('tx_links', 'anonymous_mismatch_ends_expecting_a_literal_block', 'A `x`__.\n\nB::\n'),
    ('tx_links', 'anonymous_mismatch_ends_expecting_a_literal_block_after_blanks', 'A `x`__.\n\nB::\n\n\n'),
    ('tx_links', 'anonymous_mismatch_ends_with_block_quote', 'A `x`__.\n\n    - a\n'),
    ('tx_links', 'anonymous_mismatch_ends_with_table', 'A `x`__.\n\n+-----+\n| - a |\n+-----+\n'),
    ('tx_links', 'indirect_to_duplicate', 'See `x`_.\n\n.. _x: d_\n.. _d: https://1\n.. _d: https://2\n'),
    # The inline target `x` is indirect (to "nope"); the reference itself
    # names "nope" and is left to DanglingReferences.
    ('tx_links', 'indirect_embedded_alias_unknown', 'See `x <nope_>`_ and x_.\n'),
    ('tx_links', 'substitution_reference_links', 'See |x|_ and |y|__.\n\n.. _x: https://x.example\n\n__ https://y.example\n\n.. |x| replace:: X\n.. |y| replace:: Y\n'),
    ('tx_links', 'inline_internal_target', 'See t_ and _`t` here.\n'),
    # The definition's reference and its copy each fail, in walk order.
    ('tx_links', 'dangling_inside_substitution', 'See |s|.\n\n.. |s| replace:: `nope`_\n'),
    # A footnote reference's `problematic` takes the reference's own id.
    ('tx_links', 'dangling_footnote_reference', 'See [1]_ here.\n'),
    ('tx_links', 'duplicate_section_name_reference', 'Sec\n===\n\nx\n\nSec\n===\n\ny\n\nSee `Sec`_.\n'),
    # `u` refers to the failing `t`: it is in `refnames['t']` and replaced,
    # and then resolved all the same (to `t`'s id), with its references.
    ('tx_links', 'indirect_error_replaces_referring_target', 'See `u`_ and `t`_.\n\n.. _t: missing_\n.. _u: t_\n'),
    ('tx_links', 'indirect_error_through_recursion', 'See `u`_ and `t`_.\n\n.. _u: t_\n.. _t: missing_\n'),
    # `p` propagates into the indirect target `t`; resolving `t` rewrites
    # `p` (in `document.refids`) to the final `refuri`.
    ('tx_links', 'propagated_target_into_indirect', 'See `p`_.\n\n.. _p:\n.. _t: ext_\n.. _ext: https://ext\n'),
    ('tx_links', 'indirect_circular_three', 'See `a`_, `b`_ and `c`_.\n\n.. _a: b_\n.. _b: c_\n.. _c: a_\n'),
    # The other four hints of the embedded-reference diagnosis.
    ('tx_links', 'dangling_reference_hints', 'See `a>`_, `x<y`_, `x <y> z`_ and `x < y >`_.\n'),
    # A directive's `:name:` is only in `document.nameids`: DanglingReferences
    # resolves it.
    ('tx_links', 'dangling_resolves_directive_name', 'See `box`_.\n\n.. rubric:: Heading\n   :name: box\n'),
    # The walk goes on into a replaced reference's children: the copy of
    # `nope`_ inside `|x|_` fails out of the tree, spending ids 5 and 6.
    ('tx_links', 'dangling_inside_replaced_reference', '.. |x| replace:: `nope`_\n\nSee |x|_ and `other`_.\n'),
    # A target naming itself: circular, and in its own `refnames` entry.
    ('tx_links', 'indirect_self_reference', 'See `a`_.\n\n.. _a: a_\n'),
    # An anonymous indirect target: no name in the message, and the
    # anonymous reference (in `document.refids` since 440) is replaced.
    ('tx_links', 'anonymous_indirect_unknown', 'See `x`__.\n\n__ missing_\n'),
    # A second target referring where the first one does keeps the first
    # its name (`set_duplicate_name_id`, `nodes.py:1944-1951`: an INFO,
    # which never prints): the name still resolves.
    ('tx_links', 'duplicate_embedded_uri_reference', '`Python <https://python.org>`_ and `Python <https://python.org>`_ and Python_.\n'),
    ('tx_links', 'duplicate_external_same_refuri', 'See x_.\n\n.. _x: https://same\n.. _x: https://same\n'),
    ('tx_links', 'duplicate_indirect_same_refname', 'See x_.\n\n.. _x: y_\n.. _x: y_\n\n.. _y: https://y\n'),
    # Where an unknown name is reported: the reference's nearest stamped
    # ancestor (`get_source_line`) — a section title's section, a list
    # item's paragraphs, a table cell's, a block quote's, a definition
    # list's term and definition, a field body's, an admonition's, a line
    # block's line. (Not a field name, `:0:` in docutils, nor a line
    # block's later lines, each stamped with its own line: ledgered.)
    ('tx_links', 'dangling_reference_locations', 'Title a_\n========\n\n- item b_\n\n  more c_\n\n+------+\n| d_   |\n+------+\n\n    quote e_\n\nterm f_\n   def g_\n\n:field: body i_\n\n.. note:: note j_\n\n| line k_\n'),
    # Fix round 1: an explicit-markup or line-block nested parse stops at
    # a blank line (`Explicit.blank`, `LineBlock.blank` are
    # `invalid_input`, `states.py:2773,2803`), and a hyperlink target
    # (`until_blank=True`, `:2058-2059,2532-2533`) or an empty comment
    # (`:2427-2430`) does not read one: the top level eats the trailing
    # blank and ends one past the input. A comment's or a directive's block
    # does take trailing blanks, so it still ends with no line.
    ('tx_links', 'anonymous_mismatch_ends_with_target_and_blank', 'A `x`__.\n\n.. _t: https://x/\n\n'),
    ('tx_links', 'anonymous_mismatch_ends_with_anonymous_target_and_blank', 'A `x`__ and `y`__.\n\n__ https://x/\n\n'),
    ('tx_links', 'anonymous_mismatch_ends_with_line_block_and_blank', 'A `x`__.\n\n| one\n| two\n\n'),
    ('tx_links', 'anonymous_mismatch_ends_with_empty_comment_and_blank', 'A `x`__.\n\n..\n\n'),
    ('tx_links', 'anonymous_mismatch_ends_with_empty_comment', 'A `x`__.\n\n..\n'),
    ('tx_links', 'anonymous_mismatch_ends_with_comment_and_blank', 'A `x`__.\n\n.. c\n\n'),
    ('tx_links', 'anonymous_mismatch_section_ends_with_target_and_blank', 'A `x`__.\n\nT\n=\n\n.. _t: https://x/\n\n'),
    # Fix round 1 (Task 9): Substitutions' line-length error in a document
    # ending in a hyperlink target and a blank line — one past the input
    # (the target leaves the blank to the top level), as before Task 9.
    ('tx_subst', 'expansion_exceeds_line_length_limit_ending_in_a_target_and_blank', '.. |a| replace:: ' + 'x' * 1000 + '\n.. |b| replace:: ' + ' '.join(['|a|'] * 11) + '\n\nSee |b| here.\n\n.. _t: https://x/\n\n'),
    # ===== tx_footnotes (M2 wave 5, sub-project 1, Task 10) =====
    # Sphinx's CitationDefinitionTransform and CitationReferenceTransform
    # (619, `sphinx/domains/citation.py:133-177`), docutils' Footnotes (620,
    # `docutils/transforms/references.py:416-635`), Sphinx's
    # UnreferencedFootnotesDetector (622, `sphinx/transforms/__init__.py:
    # 288-324`) and FootnoteDocnameUpdater (700, `sphinx/builders/latex/
    # transforms.py:34-43`, registered for every builder). Formerly
    # excluded: every footnote is numbered, labelled and back-linked by
    # them, every citation reference becomes a `pending_xref`.
    ('tx_footnotes', 'auto_numbered', 'See [#]_ and [#]_.\n\n.. [#] One.\n.. [#] Two.\n'),
    # Labelled auto-numbered footnotes take their numbers in footnote
    # order, every reference to a label the same one; the unlabelled one
    # between them takes the next.
    ('tx_footnotes', 'auto_named', '[#b]_ [#a]_ [#a]_ [#]_\n\n.. [#a] A\n.. [#] auto\n.. [#b] B\n'),
    ('tx_footnotes', 'manual', '[1]_ [2]_ [1]_\n\n.. [1] one\n.. [2] two\n'),
    # Ten symbols, then each doubled (`**` for the eleventh).
    ('tx_footnotes', 'symbol', ' '.join(['[*]_'] * 12) + '\n\n' + ''.join('.. [*] s%d\n' % i for i in range(12))),
    # Auto numbers skip the names manual footnotes hold (1, 2), and the
    # unreferenced manual footnote warns.
    ('tx_footnotes', 'mixed_order', '[#]_ [1]_ [#]_ [#x]_\n\n.. [#] a\n.. [1] one\n.. [#] b\n.. [2] two\n.. [#x] x\n'),
    ('tx_footnotes', 'too_many_references', '[#]_ [#]_ [#]_\n\n.. [#] a\n.. [#] b\n'),
    # Manual footnotes first, then symbol, then auto-numbered (labelled
    # ones say `[#]` too), each in document order.
    ('tx_footnotes', 'unreferenced', '.. [1] one\n.. [#] auto\n.. [*] star\n.. [#lab] lab\n.. [2] two\n'),
    # `reftarget` keeps the label as written; the citation's backrefs come
    # from the references the 619 transform already took out of the tree
    # (docutils' `citation_refs` still holds them). `[Other]` stays an
    # unresolved `pending_xref` (resolution is a write-phase matter).
    ('tx_footnotes', 'citation_definition_and_reference', 'See [CIT]_ and [Other]_ and [cit]_.\n\n.. [CIT] Citation.\n'),
    # The `duplicate citation` warning is the citation domain's, decided
    # against the environment: a merge-time record (MERGE_TIME_RECORDS in
    # tests/sphinx_doctree_differential.rs).
    ('tx_footnotes', 'citation_duplicate', '[CIT]_\n\n.. [CIT] first\n.. [CIT] second\n'),
    # NBSP around the references (Python whitespace: the inline markup is
    # recognized) and non-ASCII labels (names lowercased, ids made ASCII).
    ('tx_footnotes', 'label_nbsp', 'See [#ä]_ and [ÄB]_ and [1]_.\n\n.. [#ä] Umlaut.\n.. [ÄB] Citation.\n.. [1] One two.\n'),
    # The overflow message's plural (`footnote` for one and none).
    ('tx_footnotes', 'too_many_references_one', '[#]_ [#]_\n\n.. [#] a\n'),
    ('tx_footnotes', 'too_many_references_none', '[#]_\n'),
    ('tx_footnotes', 'too_many_symbol_references', '[*]_ [*]_ [*]_\n\n.. [*] a\n'),
    # Overflowing references in a section title: located at the section.
    ('tx_footnotes', 'too_many_references_in_a_title', '- a [#]_\n\nT [#]_ [*]_\n===========\n\n.. [#] x\n'),
    # A labelled reference no footnote carries takes the next unlabelled
    # number — and keeps its `refname`, resolved: neither ExternalTargets
    # nor SphinxDanglingReferences touches it.
    ('tx_footnotes', 'unmatched_label_takes_a_number', 'See [#nope]_ and [#]_.\n\n.. [#] x\n\n.. _nope: https://x.example\n'),
    ('tx_footnotes', 'unmatched_label_is_not_dangling', 'See [#nope]_.\n\n.. [#] x\n'),
    # The number an auto footnote is named by is a name like any other:
    # SphinxDanglingReferences resolves `[1]_` to it (no backref).
    ('tx_footnotes', 'manual_reference_to_an_auto_number', 'See [1]_.\n\n.. [#] auto\n'),
    # Duplicate labels: no footnote is named, so nothing resolves; the
    # overflow message still spends its id.
    ('tx_footnotes', 'duplicate_labelled_auto', '[#a]_\n\n.. [#a] one\n.. [#a] two\n'),
    ('tx_footnotes', 'duplicate_manual', 'A [1]_ [1]_.\n\n.. [1] x\n.. [1] y\n'),
    ('tx_footnotes', 'auto_label_number_taken', 'See [1]_ [#1]_.\n\n.. [#1] auto one\n.. [1] manual\n'),
    ('tx_footnotes', 'labelled_and_unlabelled_overflow', 'See [#]_ [#lab]_ [#]_.\n\n.. [#lab] L\n.. [#] x\n'),
    # Footnotes inside footnotes, references inside footnotes and citations.
    ('tx_footnotes', 'nested', '.. [#] outer [#]_ [*]_\n\n   .. [#] inner [#]_\n   .. [*] star\n\nSee [#]_.\n'),
    ('tx_footnotes', 'references_inside_notes', 'See [#]_ and [CIT]_.\n\n.. [#] has [CIT]_ and [1]_\n.. [CIT] has [#]_ and [1]_\n.. [1] one\n.. [#] two\n'),
]


# Wave-4.5 exclusion ledger: py-domain candidate cases this corpus must NOT
# carry, each with the evidence for why. The assert in main() keeps CASES
# disjoint from this set; removing an entry requires re-probing the reason.
EXCLUDED = {
    # -- module content with sections (py.module_basic and
    #    py.duplicate_modules, the PropagateTargets-visible module shapes,
    #    were re-admitted in M2 wave 5 once the crate ran the target
    #    transforms: family tx_targets) --
    "py.module_content_and_sections": (
        "py:module content parses with allow_section_headings=True "
        "(nested sections unrepresentable in this parser's nested "
        "contexts, T6 deviation 4) [PY §1.6]; the module id moving onto "
        "the first content paragraph (PropagateTargets) is no longer a "
        "reason since M2 wave 5"
    ),
    # -- T6 documented divergence: retann ending in ')' --
    "py.function_greedy_retann": (
        "f(x) -> (int, str): py_sig_re swallows the parenthesized retann "
        "into the arglist; sphinx's def-wrapped grammar then treats the "
        "stray ')' as closing the def and KEEPS params [x], our arglist "
        "grammar rejects it (silent Syntax) and pseudo-parses — documented "
        "T6 deviation 2, node-shape divergence"
    ),
    # -- T5 documented conservative divergences (src/py/arglist.rs module
    #    docs): expression forms outside the task-3 unparser subset --
    "py.function_default_complex": (
        "f(x=1 + 2j): complex literals are outside the task-3 expression "
        "subset -> Err(Syntax) -> pseudo fallback renders the raw text "
        "where sphinx ast_unparse renders '1 + 2j' — TEXTUAL divergence "
        "(T5 report)"
    ),
    "py.function_default_exotic_exprs": (
        "lambda / comparison / slice / f-string / dict-unpack defaults: "
        "sphinx renders (or warns via NotImplementedError/ValueError) "
        "where our expression parser errs silently into the pseudo "
        "fallback — silent-vs-warn + shape divergence (T5 report)"
    ),
    # -- T3 documented conservative divergences (src/py/expr.rs) --
    "py.function_default_exotic_strings": (
        r"\N{...} escapes and lone-surrogate \u escapes in string "
        "defaults: unsupported by the task-3 unparser -> Err -> pseudo "
        "fallback (T3 report)"
    ),
    "py.function_sig_complexity_budget": (
        "expressions beyond the 200-node MAX_DEPTH complexity budget err "
        "into the fallback where CPython/sphinx succeed (T3 review fix 2)"
    ),
    # -- real sphinx crashes: nothing to record --
    "sx_std.confval_bad_type_markup": (
        ".. confval:: t + :type: *bad — the one-child system_message "
        "inside the generated field_list CRASHES sphinx 9.1.0 "
        "(DocFieldTransformer 'assert len(field) == 2'); no oracle output "
        "exists by definition (T7 report)"
    ),
}


class WarningRecorder(io.StringIO):
    """The app's warning stream, one entry per `write` — one per printed
    record (`logging.StreamHandler.emit` writes `msg + terminator` once)."""

    def __init__(self):
        super().__init__()
        self.writes = []

    def write(self, s):
        self.writes.append(s)
        return super().write(s)


def printed_records(app: SphinxTestApp, base: Path) -> list:
    """Drain the records printed since the last call, normalized."""
    records = []
    for text in app.warning.writes:
        assert text.endswith("\n"), f"a record without its terminator: {text!r}"
        text = text[:-1].replace(str(base / "index.rst"), SOURCE_TOKEN)
        assert str(base) not in text, f"srcdir path leaked into a warning:\n{text}"
        assert "\x1b" not in text, f"ANSI escape in a warning:\n{text!r}"
        records.append(text)
    app.warning.writes.clear()
    return records


def make_app(base: Path, conf: dict) -> SphinxTestApp:
    if base.exists():
        shutil.rmtree(base)
    base.mkdir(parents=True)
    (base / "conf.py").write_text(CONF_PY, encoding="utf-8")
    (base / "index.rst").write_text("Placeholder\n===========\n", encoding="utf-8")
    assert not (set(conf) & set(CONFOVERRIDES)), (
        f"per-case conf must not override the fixture base settings: {conf}"
    )
    return SphinxTestApp(
        buildername="dummy",
        srcdir=base,
        status=io.StringIO(),
        warning=WarningRecorder(),
        confoverrides={**CONFOVERRIDES, **conf},
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
    effective["source_date_epoch"] = SOURCE_DATE_EPOCH
    return effective


def case_parts(case):
    """A case tuple is (family, name, rst[, conf]); absent conf = defaults."""
    family, name, rst = case[0], case[1], case[2]
    conf = case[3] if len(case) == 4 else {}
    assert isinstance(conf, dict), f"{family}.{name}: conf must be a dict"
    return family, name, rst, conf


def main() -> int:
    names = [f"{c[0]}.{c[1]}" for c in CASES]
    assert len(names) == len(set(names)), "family-qualified case names must be unique"
    # Anti-truncation floors. Both the global floor and the per-family ones
    # below were set in wave 3 against a corpus a fraction of this size and
    # had gone dead (the wave-4 final panel filed the global >= 40 against
    # 314 cases; the family floors summed to 119). Wave-4.5 task 16 raises
    # them to ~85-90% of the committed corpus, the same bar task 14 applied
    # to the env fixture: enough headroom to reorganize a family, not enough
    # to delete one silently. Corpus policy is EXTEND-only, so a regen that
    # trips a floor means cases were lost, not that the floor is stale.
    assert len(CASES) >= 400, f"corpus degenerated: {len(CASES)} cases"

    for reason in EXCLUDED.values():
        assert reason.strip(), "every exclusion entry needs its reason"
    hit = set(names) & set(EXCLUDED)
    assert not hit, f"excluded cases must not join the corpus: {sorted(hit)}"

    floors = {
        "sx_plain": 140,
        "sx_admonitions": 34,
        "sx_body": 30,
        "sx_image": 8,
        "sx_directives": 46,
        "sx_roles": 19,
        "sx_std": 25,
        "py": 49,
        "pysig": 30,
        "pyconf": 30,
        "tx_filter": 1,
        "tx_targets": 16,
        "tx_subst": 24,
        "tx_links": 53,
        "tx_footnotes": 20,
    }
    counts: dict = {}
    for case in CASES:
        counts[case[0]] = counts.get(case[0], 0) + 1
    assert set(counts) == set(floors), f"unexpected families: {sorted(counts)}"
    for family, floor in floors.items():
        assert counts.get(family, 0) >= floor, (
            f"family {family}: {counts.get(family, 0)} < floor {floor}"
        )

    # Group cases by DISTINCT conf: one SphinxTestApp per group (mirrors the
    # [SIG] appendix probe scripts without spinning one app per case). The
    # default group ({}) always exists and provides the settings header.
    groups: dict = {}  # conf_key -> (conf, [(family, name, rst), ...])
    for case in CASES:
        family, name, rst, conf = case_parts(case)
        key = json.dumps(conf, sort_keys=True)
        groups.setdefault(key, (conf, []))[1].append((family, name, rst))
    assert "{}" in groups, "the default-conf group must exist"

    settings_record = None
    results: dict = {}  # "family.name" -> case record
    bad = []
    for key in sorted(groups, key=lambda k: (k != "{}", k)):
        conf, group_cases = groups[key]
        # resolve(): on macOS mkdtemp returns /var/... while Sphinx resolves
        # the srcdir to /private/var/...; path normalization must match.
        base = Path(tempfile.mkdtemp(prefix="sphinx_oracle_srcdir_")).resolve() / "src"
        with docutils_namespace(), patch_docutils(str(base)):
            app = make_app(base, conf)
            try:
                # The base settings assertions hold for EVERY app: per-case
                # conf keys never touch the pinned docutils settings.
                record = check_effective_settings(app, probe(app, base, "sanity\n"))
                if key == "{}":
                    settings_record = record

                for family, name, rst in group_cases:
                    printed_records(app, base)  # drop anything printed before
                    doctree = probe(app, base, rst)
                    warnings = printed_records(app, base)
                    stray = {n.tagname for n in doctree.findall()} - SUPPORTED_KINDS
                    if stray:
                        bad.append(f"{family}.{name}: unsupported kinds {sorted(stray)}")
                        continue
                    pseudo = normalize(doctree.pformat(), base)
                    assert pseudo.startswith(f'<document source="{SOURCE_TOKEN}">\n'), (
                        f"{family}.{name}: unexpected document start tag:\n{pseudo}"
                    )
                    record_case = {
                        "name": f"{family}.{name}",
                        "family": family,
                        "rst": rst,
                        "pseudo_xml": pseudo,
                        "warnings": warnings,
                    }
                    if conf:
                        record_case["conf"] = conf
                    results[f"{family}.{name}"] = record_case

                # In-process determinism check: a second pass over the group
                # must be byte-identical (catches cross-case env leakage).
                for family, name, rst in group_cases:
                    case_name = f"{family}.{name}"
                    if case_name not in results:
                        continue  # scope violation above
                    printed_records(app, base)
                    again = normalize(probe(app, base, rst).pformat(), base)
                    again_warnings = printed_records(app, base)
                    assert again_warnings == results[case_name]["warnings"], (
                        f"{case_name}: second parse printed different records "
                        f"(cross-case state leak?)\n--- first ---\n"
                        f"{results[case_name]['warnings']}\n--- second ---\n"
                        f"{again_warnings}"
                    )
                    assert again == results[case_name]["pseudo_xml"], (
                        f"{case_name}: second parse differs (cross-case state "
                        f"leak?)\n--- first ---\n{results[case_name]['pseudo_xml']}"
                        f"\n--- second ---\n{again}"
                    )
            finally:
                app.cleanup()
                shutil.rmtree(base.parent, ignore_errors=True)

    if bad:
        print("CORPUS SCOPE VIOLATIONS:", file=sys.stderr)
        for b in bad:
            print(f"  {b}", file=sys.stderr)
        return 1

    # Emit in CASES order regardless of the conf grouping above.
    out_cases = [results[name] for name in names]

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
        "conf_semantics": (
            "a case's optional 'conf' dict is confoverrides applied on top "
            "of the base settings above; absent = defaults. The consumer "
            "maps every key onto ParseOptions.py and errors on unmapped keys."
        ),
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
