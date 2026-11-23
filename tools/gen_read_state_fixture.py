#!/usr/bin/env python3
"""Generate tests/fixtures/read_state_differential.json from Sphinx 9.1.0.

Regenerate with the locked core oracle profile:

    PYTHONNOUSERSITE=1 uv run --locked --python 3.12 \
        --project tools/oracle_profiles/core python tools/gen_read_state_fixture.py

THE READ-STATE ORACLE (wave 5a; research report "read document state" §11).
Where tools/gen_sphinx_fixture.py records one snippet's post-read doctree,
this generator also records the docutils `document` state a read leaves
behind, which the doctree does not show: every id `set_id` hands out (with
the transform that asked for it and whether the node it names survives), the
node registries right after the parse, the final state-machine position and
the read's warning stream. Each case is one `index.rst` snippet, read by
tools/gen_sphinx_fixture.py's harness (`probe`: `_parse_str_to_doctree` with
`env.settings` and the registry transforms, one `docutils_namespace()` per
read) under that fixture's settings (`keep_warnings = True`,
`smartquotes = False`), with three pieces of in-process instrumentation:

  - `document.set_id` is wrapped: each call on a node without ids is one
    `id_trace` event `[phase, kind, id, attached]` (the read-state report's
    `probe_registries.py` rule), in call order. `phase` is "parse" during the
    parse, otherwise the running transform's
    `f"{cls.__module__}.{cls.__qualname__}"`, which
    `docutils.transforms.Transformer.apply_transforms` is replaced to record
    (a copy of docutils 0.22.4's loop, `docutils/transforms/__init__.py:181-196`,
    that names each transform before applying it). `kind` is the node's
    tagname. `attached` is computed once the read is over, on the post-read
    doctree: whether the node `document.ids[id]` names is still in the tree,
    by a parent-chain walk that checks at each step that the node is among
    its parent's children (`replace_self` leaves a replaced node's `parent`
    pointing into the tree).
  - Sphinx's `RSTParser.parse` is wrapped to snapshot, the moment the parse
    returns and before any transform runs, the node registries
    (`registries_post_parse`) and `reporter.get_source_and_line(None)`, the
    top-level state machine's final position, where docutils locates a
    message that has neither a node nor a line (`final_position`).
  - The app's warning stream is emptied before each read and captured after
    it (`warnings`).

The registries are spelled the way the Rust side's
`DocumentState::registries_dump` spells them: an entry is
`[tagname, ids, attached]`; `autofootnotes`, `autofootnote_refs`,
`symbol_footnotes`, `symbol_footnote_refs`, `footnotes`, `citations` and
`indirect_targets` are lists of entries in registration order;
`footnote_refs`, `citation_refs` and `refnames` are `[key, [entry, ...]]`
pairs, keys in first-registration order; `substitution_defs` is
`[name, entry]` and `substitution_names` `[normalized name, name]` pairs, in
dict order; `names` is `[name, id or null, explicit]` in `nametypes` order.

Normalization (the ONLY rewrite): the temp srcdir's absolute path ->
`<SRCDIR>`, the HTML oracle's own token, in every recorded string; generation
fails if any spelling of the srcdir survives. `translation_progress` is kept.

Cases are appended, never inserted or renamed. Multi-document cases live in
tools/gen_env_fixture.py (`state.label_order`, `state.final_position_include`).
"""

import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

# The harness asserts the pinned sphinx and docutils versions and turns
# colour off on import.
import gen_sphinx_fixture as harness  # noqa: E402

import docutils  # noqa: E402
import sphinx  # noqa: E402
from docutils import nodes  # noqa: E402
from docutils.transforms import Transformer  # noqa: E402
from sphinx.parsers import RSTParser  # noqa: E402
from sphinx.util.docutils import docutils_namespace, patch_docutils  # noqa: E402

SRCDIR_TOKEN = "<SRCDIR>"
PARSE_PHASE = "parse"

# The anonymous reference every `state.final_position.*` case opens with:
# with no anonymous target, AnonymousHyperlinks (440) reports the mismatch
# with neither a node nor a line, so docutils locates the message at the
# final state-machine position, which the ending below decides
# ([DS §6.2]).
FINAL_POSITION_PREFIX = "Para `x`__.\n\n"

# (ending name, the construct the document ends with). The endings of
# src/rst/mod.rs's `final_position_follows_the_state_machine` (each probed on
# docutils 0.22.4), less the empty and blank-only documents, plus the
# report's further constructs ([DS §6.2]).
FINAL_POSITION_ENDINGS = [
    ("paragraph", "Last para.\n"),
    ("paragraph_trailing_blanks", "Para\n\n\n"),
    ("literal_expected", "Para::\n"),
    ("literal_expected_blank", "Para::\n\n"),
    ("literal_block", "Para::\n\n  code\n"),
    ("quoted_literal", "Para::\n\n> a\n"),
    ("quoted_literal_blank", "Para::\n\n> a\n\n"),
    ("section_title", "Title\n=====\n"),
    ("section_overline", "=====\nTitle\n=====\n"),
    ("transition", "Para.\n\n----\n"),
    ("block_quote", "  quote\n"),
    ("doctest_block", ">>> x\n"),
    ("grid_table", "+---+\n| a |\n+---+\n"),
    ("simple_table", "=  =\na  b\n=  =\n"),
    ("line_block_single", "| a\n"),
    ("line_block", "| a\n| b\n"),
    ("line_block_blank", "| a\n| b\n\n"),
    ("bullet_list", "- a\n\n\n"),
    ("enumerated_list", "1. a\n"),
    ("definition_list", "term\n  def\n\n"),
    ("field_list", ":f: v\n\n"),
    ("option_list", "-a  opt\n"),
    ("section_ends_in_list", "Title\n=====\n\n- a\n- b\n"),
    ("directive", ".. note:: x\n\n\n"),
    ("comment", ".. comment\n\n"),
    ("footnote", ".. [1] fn\n\n"),
    ("substitution_definition", ".. |s| replace:: x\n\n"),
    ("empty_comment", "..\n"),
    ("empty_comment_blank", "..\n\n"),
    ("target", "Para.\n\n.. _x:\n"),
    ("target_trailing_blanks", "Para.\n\n.. _x:\n\n\n"),
    ("external_target_continued", ".. _x: http://x\n   cont\n\n"),
    ("anonymous_target", "__ http://x\n\n"),
    ("directive_then_target", ".. note:: x\n.. _a:\n\n"),
    ("paragraph_after_target", "Para.\n\n.. _x:\n\nPara2\n"),
    ("image", ".. image:: pic.png\n"),
    ("citation", ".. [CIT] c\n"),
    ("nested_section_paragraph", "A\n=\n\nB\n-\n\npara\n"),
    ("nested_section_list", "A\n=\n\nB\n-\n\n- a\n"),
    ("title_then_directive", "Title\n=====\n\n.. note:: x\n"),
    ("list_item_paragraph", "- a\n\n  para\n"),
    ("block_quote_ending_in_list", "  quote\n\n  - item\n"),
]

# (context, input): one unknown reference `bad_` in each context, so
# SphinxDanglingReferences (850) reports `Unknown target name: "bad".` at
# the docutils location of the reference ([DS §6.1]: a node in inline
# position takes its first located ancestor's location).
LOCATION_CONTEXTS = [
    ("paragraph_continuation", "Para line one\nline two\nand bad_ here.\n"),
    (
        "grid_table_cell",
        "+------+------+\n| a    | b    |\n+======+======+\n| c    | bad_ |\n"
        "+------+------+\n",
    ),
    (
        "simple_table_cell",
        "====  ====\na     b\n====  ====\nc     bad_\n====  ====\n",
    ),
    ("bullet_item", "- one\n- two bad_\n"),
    ("enumerated_item", "1. one\n2. two bad_\n"),
    ("definition", "term\n   definition bad_\n"),
    ("classifier", "term : classifier bad_\n   definition\n"),
    ("field_body", ":field: body bad_\n"),
    ("option_description", "-a  option bad_\n"),
    ("block_quote", "Para.\n\n   quoted bad_\n"),
    ("attribution", "   Quote.\n\n   -- Author bad_\n"),
    ("line_block", "| line one\n| line bad_\n"),
    ("note_body", ".. note::\n\n   Body bad_\n"),
    ("section_title", "Title bad_\n==========\n\nText.\n"),
    ("rubric", ".. rubric:: Rubric bad_\n"),
    ("topic_title", ".. topic:: Topic bad_\n\n   Body.\n"),
    ("sidebar_title", ".. sidebar:: Side bad_\n\n   Body.\n"),
    ("admonition_title", ".. admonition:: Adm bad_\n\n   Body.\n"),
    ("figure_caption", ".. figure:: pic.png\n\n   Caption bad_\n"),
    ("image_target", ".. image:: pic.png\n   :target: bad_\n"),
    (
        "image_target_mid",
        "Para before.\n\n.. image:: pic.png\n   :target: bad_\n\nPara after.\n",
    ),
    ("substitution_definition", "Use |s| here.\n\n.. |s| replace:: bad_\n"),
    ("footnote_body", "See [1]_.\n\n.. [1] Note bad_\n"),
    ("citation_body", "See [CIT]_.\n\n.. [CIT] Cite bad_\n"),
    ("table_title", ".. table:: Cap bad_\n\n   =  =\n   a  b\n   =  =\n"),
    ("list_table_cell", ".. list-table::\n\n   * - a\n     - bad_\n"),
    ("glossary_definition", ".. glossary::\n\n   term\n      Definition bad_\n"),
]

# The named cases of [DS §11], each pinning one fact about the document
# state ([DS §4.3, §5]), then the endings and contexts above.
CASES = [
    # [DS §5.4] (= docutils-0242): ten ids, three survive.
    ("state.loose_ids_chain", "A |sub| B |sub2|_ C |sub3|__ D.\n"),
    # [DS §5.3] (= docutils-0364): a loose message makes the link `#id2`.
    (
        "state.dangling_footnote_ref_in_title",
        "Title [1]_ Here\n===============\n\nbody\n",
    ),
    # [DS §5.5] (= docutils-0236): `ids="<new> <old>"` versus `ids="<old>"`.
    ("state.problematic_id_shapes", "Refs [1]_ [#]_ [#label]_ [*]_ end.\n"),
    # [DS §4.3 (d)-(e)], [DS §4.4 (1)]: registrations in discarded subtrees.
    (
        "state.rejected_subst_autofootnote",
        ".. |f| replace:: note [#]_\n\nText [#]_.\n\n.. [#] the footnote\n",
    ),
    ("state.rejected_subst_indirect_target", ".. |f| replace:: `x <a_>`_\n\nText.\n"),
    ("state.rejected_subst_plain_target", ".. |f| replace:: _`x`\n\nText x_.\n"),
    (
        "state.rejected_table_refs",
        ".. table:: Cap [#]_\n\n   not a table [#]_\n\nText [#]_.\n\n"
        ".. [#] a\n.. [#] b\n",
    ),
    (
        "state.rejected_list_table_refs",
        ".. list-table::\n\n   not a list [#]_\n\nText [#]_.\n\n.. [#] a\n",
    ),
    # [DS §4.3 (a)-(c)]: footnote numbering follows registration order.
    (
        "state.docfields_footnote_order",
        ".. py:function:: f(a, b)\n\n"
        "   :param a: first [#]_\n"
        "   :returns: result [#]_\n"
        "   :param b: second [#]_\n\n"
        ".. [#] one\n.. [#] two\n.. [#] three\n",
    ),
    (
        "state.glossary_sorted_footnote_order",
        ".. glossary::\n   :sorted:\n\n"
        "   zeta\n      Last letter [#]_.\n\n"
        "   alpha\n      First letter [#]_.\n\n"
        ".. [#] z note\n.. [#] a note\n",
    ),
    (
        "state.docinfo_abstract_footnote_order",
        ":Abstract: abstract text [#]_\n:Author: someone [#]_\n\nBody.\n\n"
        ".. [#] abstract note\n.. [#] author note\n",
    ),
    # [DS §4.3 (f)]: substitution copies append to `refnames`.
    (
        "state.subst_copy_refnames_order",
        "One |s| two |s|.\n\n.. _indirect: missing_\n\n.. |s| replace:: indirect_\n",
    ),
    # [DS §4.4 (3)]: an embedded alias registers its target first.
    (
        "state.alias_target_before_reference",
        "See `text <alias_>`_ and alias_ and text_.\n",
    ),
    # [DS §4.3 (g)]: citation backrefs come from detached references.
    ("state.citation_backrefs", "Cite [CIT]_ and [CIT]_.\n\n.. [CIT] The citation.\n"),
    # [DS §4.3 (h)]: unreferenced footnotes warn per list.
    (
        "state.unreferenced_order",
        ".. [#] auto one\n.. [*] sym\n.. [1] manual\n.. [#b] auto labeled\n",
    ),
    # [DS §2.2]: pending transforms run after the statics of their priority.
    (
        "state.pending_serials",
        "Doc\n===\n\n"
        ".. contents::\n\n"
        ".. sectnum::\n\n"
        ".. rst-class:: special\n\n"
        "Sec A [#]_\n----------\n\n"
        "See `Python <https://python.org>`_ and Docs_ and "
        "`anon <https://anon.example>`__.\n\n"
        ".. _Docs: https://docs.example\n\n"
        "Sec B\n-----\n\n"
        ".. [#] note\n\n"
        ".. target-notes::\n",
    ),
    # [DS §7.5 (2)]: ClassAttribute targets the next sibling, a section.
    ("state.rst_class_before_section", ".. rst-class:: special\n\nSec\n===\n\nText.\n"),
    # [DS §5.6]: SortIds moves any `id`-prefixed first id.
    ("state.sortids_startswith", ".. _mylabel:\n\nIdentity\n========\n\nText.\n"),
    # [DS §7.5 (1)]: the `refname` branch of `set_duplicate_name_id`.
    (
        "state.dup_indirect_same_refname",
        "Text a_.\n\n.. _a: x_\n.. _a: x_\n.. _x: https://x/\n",
    ),
    # The three footnote-reference kinds, with their footnotes: `[#n]_` is
    # filed in `autofootnote_refs`, `footnote_refs` and `refnames`, `[*]_`
    # in `symbol_footnote_refs`, `[1]_` in `footnote_refs` and `refnames`.
    (
        "state.footnote_ref_kinds",
        "See [#n]_, [*]_ and [1]_.\n\n.. [#n] named auto\n.. [*] symbol\n.. [1] manual\n",
    ),
]
CASES += [
    (f"state.final_position.{name}", FINAL_POSITION_PREFIX + ending)
    for name, ending in FINAL_POSITION_ENDINGS
]
CASES += [(f"state.location.{context}", rst) for context, rst in LOCATION_CONTEXTS]


# ---------------------------------------------------------------------------
# Instrumentation
# ---------------------------------------------------------------------------


class Recorder:
    """What the instrumentation collects for the read in progress."""

    def __init__(self):
        self.active = False
        self.phase = PARSE_PHASE
        self.events = []
        self.post_parse = None

    def start(self):
        self.active = True
        self.phase = PARSE_PHASE
        self.events = []
        self.post_parse = None

    def stop(self):
        self.active = False


RECORDER = Recorder()

_original_set_id = nodes.document.set_id
_original_parse = RSTParser.parse


def traced_set_id(self, node, msgnode=None, suggested_prefix=""):
    fresh = not node["ids"]
    new_id = _original_set_id(self, node, msgnode, suggested_prefix)
    if fresh and RECORDER.active:
        RECORDER.events.append([RECORDER.phase, node.tagname, new_id])
    return new_id


def traced_apply_transforms(self):
    """docutils 0.22.4's `Transformer.apply_transforms`
    (`docutils/transforms/__init__.py:181-196`), naming each transform as the
    trace's phase before applying it."""
    self.document.reporter.attach_observer(self.document.note_transform_message)
    while self.transforms:
        if not self.sorted:
            # Unsorted initially, and whenever a transform is added
            # (transforms may add other transforms).
            self.transforms.sort(reverse=True)
            self.sorted = True
        priority, transform_class, pending, kwargs = self.transforms.pop()
        RECORDER.phase = f"{transform_class.__module__}.{transform_class.__qualname__}"
        transform = transform_class(self.document, startnode=pending)
        transform.apply(**kwargs)
        self.applied.append((priority, transform_class, pending, kwargs))
    self.document.reporter.detach_observer(self.document.note_transform_message)


def parse_and_snapshot(self, inputstring, document):
    _original_parse(self, inputstring, document)
    if RECORDER.active and RECORDER.post_parse is None:
        source, line = document.reporter.get_source_and_line(None)
        RECORDER.post_parse = (registries_dump(document), [source, line])


nodes.document.set_id = traced_set_id
Transformer.apply_transforms = traced_apply_transforms
RSTParser.parse = parse_and_snapshot


def attached(document, node) -> bool:
    """Whether `node` is in the tree under `document`: every step of its
    parent chain is a child of the next, up to the document."""
    while node is not document:
        parent = node.parent
        if parent is None or not any(child is node for child in parent.children):
            return False
        node = parent
    return True


def entry(document, node) -> list:
    return [node.tagname, list(node["ids"]), attached(document, node)]


def registries_dump(document) -> dict:
    def listed(registry):
        return [entry(document, node) for node in registry]

    def keyed(registry):
        return [[key, listed(registered)] for key, registered in registry.items()]

    return {
        "autofootnotes": listed(document.autofootnotes),
        "autofootnote_refs": listed(document.autofootnote_refs),
        "symbol_footnotes": listed(document.symbol_footnotes),
        "symbol_footnote_refs": listed(document.symbol_footnote_refs),
        "footnotes": listed(document.footnotes),
        "footnote_refs": keyed(document.footnote_refs),
        "citations": listed(document.citations),
        "citation_refs": keyed(document.citation_refs),
        "refnames": keyed(document.refnames),
        "indirect_targets": listed(document.indirect_targets),
        "substitution_defs": [
            [name, entry(document, node)]
            for name, node in document.substitution_defs.items()
        ],
        "substitution_names": [
            [normalized, name] for normalized, name in document.substitution_names.items()
        ],
        "names": [
            [name, document.nameids.get(name), bool(explicit)]
            for name, explicit in document.nametypes.items()
        ],
    }


# ---------------------------------------------------------------------------
# Reading the cases
# ---------------------------------------------------------------------------


def normalize(value, base: Path):
    """`base` -> `<SRCDIR>` in every string of `value`."""
    if isinstance(value, str):
        text = value.replace(str(base), SRCDIR_TOKEN)
        for spelling in {str(base), str(base.parent)}:
            assert spelling not in text, f"srcdir path leaked:\n{text}"
        return text
    if isinstance(value, list):
        return [normalize(item, base) for item in value]
    if isinstance(value, dict):
        return {key: normalize(item, base) for key, item in value.items()}
    return value


def warning_lines(stream: str) -> list:
    """The warning stream's lines: each record is its text plus `\\n`, so a
    multi-line record spans several lines and the stream ends in `\\n`."""
    if not stream:
        return []
    assert stream.endswith("\n"), f"unterminated warning stream: {stream!r}"
    return stream[:-1].split("\n")


def read_case(app, base: Path, rst: str) -> dict:
    """One read through the harness, with the instrumentation on."""
    app.warning.seek(0)
    app.warning.truncate()
    RECORDER.start()
    try:
        document = harness.probe(app, base, rst)
    finally:
        RECORDER.stop()
    assert RECORDER.post_parse is not None, "the parse never returned"
    registries, final_position = RECORDER.post_parse
    trace = [
        [phase, kind, node_id, attached(document, document.ids[node_id])]
        for phase, kind, node_id in RECORDER.events
    ]
    return normalize(
        {
            "pseudo_xml": document.pformat(),
            "warnings": warning_lines(app.warning.getvalue()),
            "id_trace": trace,
            "registries_post_parse": registries,
            "final_position": final_position,
        },
        base,
    )


def reference_crash_inputs() -> dict:
    """The input of every docutils snippet the HTML oracle's core ledger
    marks `reference-crash`, by case id: inputs that crash Sphinx 9.1.0,
    which Ultra does not model and no fixture records (wave 5a, decision 13)."""
    _, crashes = harness.html_oracle_docutils_cases()
    return {
        (harness.HTML_ORACLE_ROOT / "inputs" / harness.HTML_ORACLE_SOURCE_SET / case / "index.rst")
        .read_text(encoding="utf-8"): case
        for case in crashes
    }


def main() -> int:
    names = [name for name, _ in CASES]
    assert len(names) == len(set(names)), "case names must be unique"
    assert all(name.startswith("state.") for name in names), "names start with state."
    crashes = reference_crash_inputs()
    cases = []
    for name, rst in CASES:
        if rst in crashes:
            print(f"skipped {name}: the input of reference-crash case {crashes[rst]}")
        else:
            cases.append((name, rst))

    base = Path(tempfile.mkdtemp(prefix="read_state_srcdir_")).resolve() / "src"
    results = []
    with docutils_namespace(), patch_docutils(str(base)):
        app = harness.make_app(base, {})
        try:
            settings = harness.check_effective_settings(
                app, harness.probe(app, base, "sanity\n")
            )
            for name, rst in cases:
                record = read_case(app, base, rst)
                assert record["pseudo_xml"].startswith(
                    f'<document source="{SRCDIR_TOKEN}/index.rst" '
                    f"{harness.TRANSLATION_PROGRESS}>\n"
                ), f"{name}: unexpected document start tag:\n{record['pseudo_xml']}"
                results.append({"name": name, "rst": rst, **record})
            # In-process determinism check: a second pass over every case
            # must record the same state (catches cross-case leakage).
            for case in results:
                again = read_case(app, base, case["rst"])
                first = {key: case[key] for key in again}
                assert again == first, (
                    f"{case['name']}: second read differs (cross-case state leak?)"
                    f"\n--- first ---\n{first}\n--- second ---\n{again}"
                )
        finally:
            app.cleanup()

    fixture = {
        "docutils_version": docutils.__version__,
        "sphinx_version": sphinx.__version__,
        "generator": "tools/gen_read_state_fixture.py",
        "harness": (
            "tools/gen_sphinx_fixture.py's probe (sphinx.util.docutils."
            "_parse_str_to_doctree with env.settings + registry transforms, one "
            "docutils_namespace() per read), instrumented: document.set_id "
            "(id_trace), Transformer.apply_transforms (the trace's phase), "
            "RSTParser.parse (registries_post_parse, final_position)"
        ),
        "settings": settings,
        "normalizations": [f"srcdir absolute path -> {SRCDIR_TOKEN}"],
        "cases": results,
    }
    out_path = (
        Path(__file__).resolve().parent.parent
        / "tests"
        / "fixtures"
        / "read_state_differential.json"
    )
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(fixture, f, indent=2, sort_keys=True, ensure_ascii=False)
        f.write("\n")
    print(
        f"wrote {out_path}: {len(results)} cases, "
        f"sphinx {sphinx.__version__}, docutils {docutils.__version__}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
