#!/usr/bin/env python3
"""Generate tests/fixtures/inventories/ from Sphinx 9.1.0.

Regenerate with:

    uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' \
        python tools/gen_inventory_fixture.py

THE objects.inv ORACLE. Where tools/gen_env_fixture.py records `BuildEnvironment`
state, this generator records the on-disk `objects.inv` inventory Sphinx's own
HTML builder writes for a handful of tiny projects, plus a set of handcrafted
byte-level cases (a v1 inventory, malformed headers, an absolute-location
entry) that don't naturally arise from a real build. For every case the raw
`.inv` bytes are committed under tests/fixtures/inventories/, and this script
records EITHER the parsed table OR the exact error text Sphinx's own
`sphinx.util.inventory.InventoryFile.loads` produces for those same bytes --
never a hand-derived expectation.

Design verified empirically in this session against sphinx 9.1.0 / docutils
0.22.4 under the pinned uv invocation above, against
sphinx/util/inventory.py (InventoryFile.loads/_loads_v1/_loads_v2/dump) and
docs/superpowers/plans/2026-08-31-m2-wave4-research-spec-inventory-intersphinx.md
section 1 (exact byte framing, `$`-suffix expansion order, error texts).

Harness notes:
  - `buildername='html'`: the HTML builder's finish task is the only one that
    calls `InventoryFile.dump`, so a plain `app.build()` against a real
    two-to-three-document project is enough to get a byte-real `objects.inv`
    on disk at `app.outdir / 'objects.inv'` -- no monkeypatching needed.
  - Every SPHINX_PROJECTS entry is built to produce ZERO warnings (checked
    below): rich enough to exercise the quirks in its `covers` note, boring
    enough that Sphinx has nothing to complain about.
  - `UNSAFE_UTF8_CASE` names the one SPHINX_PROJECTS entry whose compressed
    zlib payload is asserted (generation fails otherwise) to contain both a
    bare 0x0D and a bare 0x0A byte AND to not be valid UTF-8 as a whole --
    the exact byte class the old lossy-UTF8-then-`.lines()` reader corrupted
    (`src/inventory.rs:69-73` pre-Task-11). Any sufficiently content-rich
    real inventory has this property (verified empirically); this is not a
    contrived encoding, just an assertion that our corpus keeps exhibiting it.
  - HANDCRAFTED_OK and HANDCRAFTED_ERROR bytes are written by this script,
    then fed to the *real* `InventoryFile.loads` to derive the committed
    expectation -- for HANDCRAFTED_ERROR cases specifically, the expectation
    is `str(exc)` from the real raised `ValueError`, so the exact text
    (including the `!r`-repr'd unknown-version case) can never drift from
    what Sphinx itself raises.
  - All parsed-table expectations (from both SPHINX_PROJECTS and
    HANDCRAFTED_OK) are generated with the SAME `uri` (URI_BASE, no trailing
    slash) so `posixpath.join` exercises the "insert separator" branch
    uniformly; HANDCRAFTED_OK additionally includes one entry with an
    *absolute* location (leading `/`) to exercise posixpath.join's
    absolute-location-overrides-base branch end to end.

CORPUS POLICY: SPHINX_PROJECTS covers what a real build produces (std labels
with `$`-compaction, std:doc entries, glossary terms with spaces/colons,
unicode names, dispname compaction to `-`, explicit differing dispnames,
header whitespace-run escaping, empty vs. non-empty version). HANDCRAFTED_OK
covers what a real build never produces but the reader must still accept (a
v1 inventory, an absolute-location entry). HANDCRAFTED_ERROR covers every
distinct ValueError text in inventory.py's four raise sites, through both
the v1 and v2 code paths where a site is shared. Never remove or rename an
existing case name; later tasks only extend.
"""

import io
import json
import shutil
import sys
import tempfile
import zlib
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

from sphinx.testing.util import SphinxTestApp  # noqa: E402
from sphinx.util.docutils import docutils_namespace, patch_docutils  # noqa: E402
from sphinx.util.inventory import InventoryFile  # noqa: E402

URI_BASE = "https://docs.example.org/v1"
BASE_CONFOVERRIDES = {"smartquotes": False}
CONF_PY_TEMPLATE = (
    "project = {project!r}\n"
    "extensions = []\n"
    "master_doc = 'index'\n"
    "exclude_patterns = ['_build']\n"
)

# ---------------------------------------------------------------------------
# SPHINX_PROJECTS: built via a real `SphinxTestApp(buildername='html')` +
# `app.build()`; the committed bytes are Sphinx's own `objects.inv` output.
# ---------------------------------------------------------------------------

SPHINX_PROJECTS = [
    {
        "name": "std_objects_and_docs",
        "project": "fixture",
        "conf": {},
        "covers": (
            "std:label $-compaction (multi-word explicit dispname AND "
            "title==id -> '-'), std:doc entries (dispname = doc title), "
            "glossary terms with an embedded space and a bare colon, "
            "py:module/py:function/py:class/py:method $-compaction, "
            "binary-safety oracle (see UNSAFE_UTF8_CASE below)."
        ),
        "files": {
            "index": """\
Index
=====

.. toctree::

   a
   b

.. _explicit-target:

Explicit Target Section
------------------------

Text under explicit target.
""",
            "a": """\
A
=

.. glossary::

   a term
      Definition one.

   a term including:colon
      Definition two.

.. py:module:: mymod

   Module docstring.

.. py:function:: greet(name)

   Greets somebody.
""",
            "b": """\
B
=

See :term:`a term` and :doc:`a`.

.. _example:

example
-------

Section whose title equals its id (dispname == fullname -> '-').
""",
        },
    },
    {
        "name": "unicode_names",
        "project": "unicode fixture",
        "conf": {},
        "covers": (
            "Non-ASCII fullname/dispname/project-name bytes throughout: a "
            "py:function/py:module whose accented name does NOT collapse "
            "to a `$`-compacted anchor (docutils ASCII-folds the anchor id "
            "but not the recorded name -- anchor no longer ends with "
            "fullname), a CJK label+title pair, a glossary term with "
            "accented characters in both term and body."
        ),
        "files": {
            "index": """\
Index
=====

.. toctree::

   a

.. _日本語ラベル:

日本語の見出し
--------------

Text with a Japanese heading and label.
""",
            "a": """\
A
=

.. py:function:: café(x)

   A unicode-named function.

.. py:module:: café_mod

   A unicode module.

.. glossary::

   Ünïcödé Term
      A definition with unicode in both term and body: héllo wörld.
""",
        },
    },
    {
        "name": "dispname_and_header_escape",
        "project": "My   Project",
        "conf": {"version": "1.0   beta"},
        "covers": (
            "Header whitespace-run escaping on both Project and Version "
            "(collapsed to single spaces; entry lines are NOT escaped), "
            "nested py:class/py:method both $-compacting, an explicit "
            "std:label title==id dispname compaction alongside a std:doc "
            "entry whose dispname differs from its docname."
        ),
        "files": {
            "index": """\
Index
=====

.. toctree::

   example

.. _example:

example
-------

Section whose title exactly equals its label id (dispname==fullname -> '-').

.. py:class:: Widget

   A widget class.

   .. py:method:: render()

      Render the widget.
""",
            "example": """\
Example Page
=============

Body text.
""",
        },
    },
    {
        "name": "empty_version_minimal",
        "project": "minimal",
        "conf": {},
        "covers": (
            "The default, un-set `version` config renders as a genuinely "
            "empty `# Version:` header line (blank, not '-' or absent) -- "
            "the smallest project in the corpus so this case reads clearly "
            "on its own."
        ),
        "files": {
            "index": """\
Index
=====

.. toctree::

   a

.. py:function:: ping()

   Ping the server.
""",
            "a": """\
A
=

.. _minimal-label:

Minimal Label Section
----------------------

Just a plain section for a std:label.
""",
        },
    },
]

# The SPHINX_PROJECTS entry whose compressed payload must contain a bare
# 0x0D, a bare 0x0A, and not be valid UTF-8 as a whole -- asserted below.
UNSAFE_UTF8_CASE = "std_objects_and_docs"

# ---------------------------------------------------------------------------
# HANDCRAFTED_OK: bytes this script authors directly (not from a Sphinx
# build), still validated by feeding them through the real InventoryFile.loads.
# ---------------------------------------------------------------------------

HANDCRAFTED_OK = [
    {
        "name": "v1_handcrafted",
        "covers": (
            "A version-1 inventory (any-whitespace split, 'mod' -> "
            "py:module + #module-{name} anchor, everything else -> "
            "py:{item_type} + #{name} anchor, display_name always '-')."
        ),
        "bytes": (
            b"# Sphinx inventory version 1\n"
            b"# Project: legacy project\n"
            b"# Version: 0.9\n"
            b"widget mod widget.html\n"
            b"widget.Gadget class widget.html\n"
        ),
    },
    {
        "name": "v2_absolute_location",
        "covers": (
            "posixpath.join's absolute-location-overrides-base branch, "
            "end to end through the real v2 line parser (a location "
            "starting with '/' replaces URI_BASE entirely rather than "
            "being joined onto it)."
        ),
        "bytes": (
            b"# Sphinx inventory version 2\n"
            b"# Project: absolute test\n"
            b"# Version: 1.0\n"
            b"# The remainder of this file is compressed using zlib.\n"
        )
        + zlib.compress(
            b"thing py:function 1 /abs/other-root/thing.html#thing Thing\n"
        ),
    },
    {
        "name": "v2_duplicate_py_module",
        "covers": (
            "py:module first-wins dedup (inventory.py:129-134): two "
            "entries share the same (type, name) = ('py:module', "
            "'dupmod') key -- the *first* one wins (a doubling bug in "
            "Sphinx <=1.1's own writer the reader still has to "
            "tolerate), not the last, unlike every other type/name "
            "collision (plain last-wins dict overwrite). The expected "
            "table below -- generated by real sphinx, not us -- pins "
            "that the surviving uri/display_name come from the FIRST "
            "line (first.html / 'First Entry'), not the second."
        ),
        "bytes": (
            b"# Sphinx inventory version 2\n"
            b"# Project: dup test\n"
            b"# Version: 1.0\n"
            b"# The remainder of this file is compressed using zlib.\n"
        )
        + zlib.compress(
            b"dupmod py:module 0 first.html#module-dupmod First Entry\n"
            b"dupmod py:module 0 second.html#module-dupmod Second Entry\n"
        ),
    },
]

# ---------------------------------------------------------------------------
# HANDCRAFTED_ERROR: every distinct ValueError text in inventory.py's four
# raise sites (`_loads_v1`/`_loads_v2`/`loads`), through both code paths
# where a message text is shared between v1 and v2.
# ---------------------------------------------------------------------------

HANDCRAFTED_ERROR = [
    {
        "name": "err_unknown_version",
        "covers": (
            "loads(): first line matches the version-N prefix but N isn't "
            "1 or 2 -> ValueError with a !r-repr'd version suffix."
        ),
        "bytes": (
            b"# Sphinx inventory version 5\n"
            b"# Project: p\n"
            b"# Version: 1\n"
            b"# The remainder of this file is compressed using zlib.\n"
        ),
    },
    {
        "name": "err_invalid_header",
        "covers": (
            "loads(): first line matches neither v1, v2, nor the "
            "version-N prefix at all -> generic invalid-header ValueError "
            "quoting the raw (decoded, unescaped) first line."
        ),
        "bytes": b"Not a Sphinx inventory header\nsome other content\n",
    },
    {
        "name": "err_not_compressed",
        "covers": (
            "_loads_v2(): header parses (4 `\\n`-delimited chunks exist) "
            "but the fourth (check) line doesn't contain the substring "
            "'zlib' -> not-compressed ValueError quoting that check line."
        ),
        "bytes": (
            b"# Sphinx inventory version 2\n"
            b"# Project: p\n"
            b"# Version: 1\n"
            b"# The remainder of this file is compressed using LZMA.\n"
            b"garbage tail that is never reached\n"
        ),
    },
    {
        "name": "err_missing_project_version_v1",
        "covers": (
            "_loads_v1(): fewer than 2 lines remain after the format "
            "line (no Version line at all) -> missing-project-or-version "
            "ValueError, v1 code path."
        ),
        "bytes": b"# Sphinx inventory version 1\n# Project: onlyname\n",
    },
    {
        "name": "err_missing_project_version_v2",
        "covers": (
            "_loads_v2(): `content.split(b'\\n', maxsplit=3)` yields "
            "fewer than 4 parts (no check-line/compressed-tail at all) "
            "-> missing-project-or-version ValueError, v2 code path -- "
            "same message text as the v1 case above, different raise site."
        ),
        "bytes": b"# Sphinx inventory version 2\n# Project: onlyname\n",
    },
]


def write_project_files(base: Path, files: dict) -> None:
    for docname, text in files.items():
        path = base / f"{docname}.rst"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")


def capture_raw_objects(env) -> dict:
    """Capture the exact pre-compaction `domain.get_objects()` records
    `InventoryFile.dump` itself iterates (`env.domains.sorted()` +
    `sorted(domain.get_objects())`), grouped by domain name, as plain JSON
    values (`dispname`/`docname` can be lazy i18n proxy objects -- `str()`-ed
    like `gen_env_fixture.py` does for `sectionname`). This is what lets the
    Rust writer round-trip test feed `InvObject`s carrying the SAME priority
    and pre-`$`-compaction anchor Sphinx's own writer used, so its output can
    be compared byte-exact-per-line against this case's committed `.inv`
    (not just structurally, via the lossy parsed `expect` table, which drops
    `priority` entirely -- `_InventoryItem` has no such field).
    """
    by_domain: dict[str, list] = {}
    for domain in env.domains.sorted():
        records = [
            {
                "name": fullname,
                "dispname": str(dispname),
                "objtype": objtype,
                "docname": docname,
                "anchor": anchor,
                "priority": prio,
            }
            for fullname, dispname, objtype, docname, anchor, prio in domain.get_objects()
        ]
        if records:
            by_domain[domain.name] = records
    return by_domain


def build_sphinx_project(entry: dict) -> tuple[bytes, dict]:
    """Build `entry` with a real HTML app; return its raw objects.inv bytes
    plus the raw pre-compaction object records behind them (see
    `capture_raw_objects`)."""
    base = Path(tempfile.mkdtemp(prefix="inv_oracle_srcdir_")).resolve() / "src"
    base.mkdir(parents=True)
    (base / "conf.py").write_text(
        CONF_PY_TEMPLATE.format(project=entry["project"]), encoding="utf-8"
    )
    write_project_files(base, entry["files"])
    outdir = base.parent / "out"

    confoverrides = {**BASE_CONFOVERRIDES, **entry.get("conf", {})}

    with docutils_namespace(), patch_docutils(str(base)):
        app = SphinxTestApp(
            buildername="html",
            srcdir=base,
            outdir=outdir,
            status=io.StringIO(),
            warning=io.StringIO(),
            confoverrides=dict(confoverrides),
        )
        try:
            app.build()
            warnings = app.warning.getvalue()
            assert not warnings.strip(), (
                f"project {entry['name']!r} produced unexpected warnings "
                f"(corpus projects must build clean):\n{warnings}"
            )
            raw = (Path(app.outdir) / "objects.inv").read_bytes()
            raw_objects = capture_raw_objects(app.env)
        finally:
            app.cleanup()
            shutil.rmtree(base.parent, ignore_errors=True)

    return raw, raw_objects


def inventory_to_table(inv) -> dict:
    """Convert a real `_Inventory` into the JSON-able {objtype: {name: {...}}} shape."""
    return {
        objtype: {
            name: {
                "project_name": item.project_name,
                "project_version": item.project_version,
                "uri": item.uri,
                "display_name": item.display_name,
            }
            for name, item in names.items()
        }
        for objtype, names in inv.data.items()
    }


def build_ok_case(
    name: str, covers: str, raw: bytes, *, source: str, raw_objects: dict | None = None
) -> dict:
    inv = InventoryFile.loads(raw, uri=URI_BASE)
    table = inventory_to_table(inv)
    # Every item in one inventory file shares the same (project_name,
    # project_version) header pair -- pull it from any single item so the
    # Rust writer round-trip test doesn't have to re-derive it.
    first_item = next(iter(next(iter(table.values())).values()))
    case = {
        "name": name,
        "kind": "ok",
        "source": source,
        "covers": covers,
        "inv_file": f"{name}.inv",
        "project": first_item["project_name"],
        "version": first_item["project_version"],
        "expect": table,
    }
    if raw_objects is not None:
        case["raw_objects"] = raw_objects
    return case


def build_error_case(name: str, covers: str, raw: bytes) -> dict:
    try:
        InventoryFile.loads(raw, uri=URI_BASE)
    except ValueError as exc:
        error_text = str(exc)
    else:
        raise AssertionError(
            f"handcrafted error case {name!r} did not raise ValueError"
        )
    return {
        "name": name,
        "kind": "error",
        "source": "handcrafted",
        "covers": covers,
        "inv_file": f"{name}.inv",
        "error": error_text,
    }


def generate_all() -> tuple[dict, dict[str, bytes]]:
    cases = []
    raw_bytes: dict[str, bytes] = {}
    unsafe_checked = False

    for entry in SPHINX_PROJECTS:
        raw, raw_objects = build_sphinx_project(entry)
        if entry["name"] == UNSAFE_UTF8_CASE:
            unsafe_checked = True
            _, _, _, compressed = raw.split(b"\n", 3)
            assert 0x0D in compressed and 0x0A in compressed, (
                f"{entry['name']!r} was supposed to exercise the "
                "binary-safety (bare CR/LF in the compressed tail) case "
                "but no longer does -- richen its corpus back up"
            )
            try:
                compressed.decode("utf-8")
            except UnicodeDecodeError:
                pass
            else:
                raise AssertionError(
                    f"{entry['name']!r} was supposed to exercise the "
                    "binary-safety (invalid-UTF-8 compressed tail) case "
                    "but its compressed payload is valid UTF-8 now -- "
                    "richen its corpus back up"
                )
        case = build_ok_case(
            entry["name"],
            entry["covers"],
            raw,
            source="sphinx_build",
            raw_objects=raw_objects,
        )
        cases.append(case)
        raw_bytes[entry["name"]] = raw

    assert unsafe_checked, f"UNSAFE_UTF8_CASE {UNSAFE_UTF8_CASE!r} not in SPHINX_PROJECTS"

    for entry in HANDCRAFTED_OK:
        case = build_ok_case(
            entry["name"], entry["covers"], entry["bytes"], source="handcrafted"
        )
        cases.append(case)
        raw_bytes[entry["name"]] = entry["bytes"]

    for entry in HANDCRAFTED_ERROR:
        case = build_error_case(entry["name"], entry["covers"], entry["bytes"])
        cases.append(case)
        raw_bytes[entry["name"]] = entry["bytes"]

    manifest = {
        "sphinx_version": sphinx.__version__,
        "docutils_version": docutils.__version__,
        "generator": "tools/gen_inventory_fixture.py",
        "uri_base": URI_BASE,
        "unsafe_utf8_case": UNSAFE_UTF8_CASE,
        "cases": cases,
    }
    return manifest, raw_bytes


def main() -> int:
    all_names = (
        [p["name"] for p in SPHINX_PROJECTS]
        + [p["name"] for p in HANDCRAFTED_OK]
        + [p["name"] for p in HANDCRAFTED_ERROR]
    )
    assert len(all_names) == len(set(all_names)), "case names must be unique"
    assert len(SPHINX_PROJECTS) >= 4, f"corpus degenerated: {len(SPHINX_PROJECTS)} projects"

    manifest, raw_bytes = generate_all()

    # In-process determinism check: a second full pass must be byte-identical
    # for both the manifest and every raw .inv file (SPHINX_PROJECTS re-runs
    # a fresh SphinxTestApp per project; HANDCRAFTED_* are static bytes).
    manifest_again, raw_bytes_again = generate_all()
    first_json = json.dumps(manifest, indent=2, sort_keys=True, ensure_ascii=False)
    second_json = json.dumps(manifest_again, indent=2, sort_keys=True, ensure_ascii=False)
    if first_json != second_json:
        print("DETERMINISM VIOLATION: two in-process manifest passes differ", file=sys.stderr)
        return 1
    if raw_bytes != raw_bytes_again:
        diffs = sorted(k for k in raw_bytes if raw_bytes[k] != raw_bytes_again.get(k))
        print(f"DETERMINISM VIOLATION: raw .inv bytes differ for {diffs}", file=sys.stderr)
        return 1

    out_dir = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "inventories"
    out_dir.mkdir(parents=True, exist_ok=True)
    # Clean up any previously-committed .inv files whose case was renamed/removed.
    keep = {f"{name}.inv" for name in raw_bytes}
    for existing in out_dir.glob("*.inv"):
        if existing.name not in keep:
            existing.unlink()

    for name, raw in raw_bytes.items():
        (out_dir / f"{name}.inv").write_bytes(raw)

    manifest_path = out_dir / "manifest.json"
    with open(manifest_path, "w", encoding="utf-8") as f:
        f.write(first_json)
        f.write("\n")

    print(
        f"wrote {manifest_path} + {len(raw_bytes)} .inv files: "
        f"{len(SPHINX_PROJECTS)} sphinx-built, {len(HANDCRAFTED_OK)} handcrafted-ok, "
        f"{len(HANDCRAFTED_ERROR)} handcrafted-error, "
        f"sphinx {sphinx.__version__}, docutils {docutils.__version__}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
