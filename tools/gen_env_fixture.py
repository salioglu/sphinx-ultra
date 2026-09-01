#!/usr/bin/env python3
"""Generate tests/fixtures/env_differential.json from Sphinx 9.1.0.

Regenerate with:

    uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' \
        python tools/gen_env_fixture.py

THE ENVIRONMENT-LAYER ORACLE. Where tools/gen_sphinx_fixture.py records
per-SNIPPET read-phase pseudo-XML, this generator records per-PROJECT
`BuildEnvironment` state: the toctree graph, relations, section/figure
numbering, std-domain object/label registries, the index/genindex adapters,
and fully cross-reference-resolved doctrees. A "project" is a small
multi-document srcdir (dict of docname -> rst source + conf overrides),
built with a real `SphinxTestApp(buildername='dummy')` + `app.build()` --
exactly what a real `sphinx-build` does for its read + resolve phases, minus
writing output files.

Design verified empirically in this session against sphinx 9.1.0 / docutils
0.22.4 under the pinned uv invocation above; see
docs/superpowers/plans/2026-08-31-m2-wave4-research-read-fixtures-oracles.md
section 2 (env attribute shapes, relations quirk, lazy-i18n label trap) and
docs/superpowers/plans/2026-08-31-m2-wave4-research-spec-sphinx-env-toctree-domains.md
section 8 (exact warning texts several corpus projects below are built to
trigger byte-identically).

Harness notes:
  - `DummyBuilder.write_doc` is a no-op and `get_target_uri` always returns
    `''`; nonetheless the base `Builder.write()` write loop (`_write_docname`,
    builders/__init__.py) unconditionally calls
    `env.get_and_resolve_doctree(docname, builder)` for every found document
    before handing the resolved doctree to `write_doc` -- i.e. a plain dummy
    `app.build()` already performs full post-transform + toctree resolution
    for every document, byte-identical to what an HTML build would resolve.
    This generator monkeypatches the *instance* `app.builder.write_doc` to
    capture that already-resolved doctree's `pformat()` text, so
    `resolved_pformat` reflects the real single resolution pass a build
    performs -- no second resolution pass, hence no duplicated warnings.
  - `env.collect_relations()` is NOT called automatically by DummyBuilder
    (only HTML-family builders call it, for prev/next rellinks), so this
    generator calls it explicitly after `app.build()` to populate `relations`
    -- this is also the only place `_traverse_toctree`'s self-referencing-
    toctree warning fires (environment/__init__.py:920-926), so
    `toctree_self_ref` below relies on this explicit call.
  - `warnings` is snapshotted from `app.warning.getvalue()` once, after
    `app.build()` + `env.collect_relations()` + `IndexEntries(...).create_index()`
    have all run, in that fixed order -- the full, non-duplicated warning
    text a real build-plus-relations-plus-genindex pass would produce.
  - confoverrides always include `{'smartquotes': False}` (Sphinx's default
    smartquotes rewriting is irrelevant noise for this corpus); per-project
    extras (numfig, numfig_secnum_depth, numfig_format, ...) come from each
    corpus entry's own `conf` dict and are recorded verbatim in the fixture's
    per-project `conf` field so a later Rust consumer can replay the exact
    same build configuration.
  - `sphinx.util.console.nocolor()` -- warning text must not carry ANSI.
  - Per-project isolation: a fresh tmp srcdir + fresh `SphinxTestApp` per
    project, wrapped in `docutils_namespace()` + `patch_docutils()` (copies
    tools/gen_sphinx_fixture.py's harness hygiene).

Normalization (the ONLY rewrite): every absolute path under the project's tmp
srcdir is replaced by the token `<project>` in every piece of captured text
(`warnings`, `tocs_pformat` values, `resolved_pformat` values) -- generation
fails (assertion) if any occurrence of the raw srcdir path survives. Both the
as-returned `mkdtemp()` path and its `.resolve()`d form are checked (macOS
resolves `/var/...` to `/private/var/...`; Sphinx internally uses the
resolved form, per the same gotcha documented in gen_sphinx_fixture.py).

Value shapes: Python `tuple`s (relations entries excepted, which are already
plain lists) are converted to JSON lists; `set`s (`files_to_rebuild` values)
become sorted lists; std-domain dict keys that are themselves tuples
(`objects`: `(objtype, name)`, `progoptions`: `(program_or_None, name)`) are
flattened into a sorted list of `{..key fields.., docname, labelid}` records
since JSON object keys must be strings. The three preseeded virtual std
labels (genindex/modindex/search) are KEPT in `std.labels`/`std.anonlabels`
(they are part of the real oracle contract); their `sectionname` is a lazy
i18n proxy object and is `str()`-ed like every other sectionname.

CORPUS POLICY: one axis per project (toctree nesting/glob/numbering/self-ref/
circular/multi-parent/orphan, numfig figure-table-code-block numbering incl.
numref format styles, std-domain program/option/envvar/confval registration,
glossary term resolution, index-entry/genindex grouping, :doc: resolution).
Never remove or rename an existing project name; later tasks only extend.
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

from sphinx.environment.adapters.indexentries import IndexEntries  # noqa: E402
from sphinx.testing.util import SphinxTestApp  # noqa: E402
from sphinx.util.docutils import docutils_namespace, patch_docutils  # noqa: E402

SOURCE_TOKEN = "<project>"

BASE_CONFOVERRIDES = {"smartquotes": False}

CONF_PY = (
    "project = 'fixture'\n"
    "extensions = []\n"
    "master_doc = 'index'\n"
    "exclude_patterns = ['_build']\n"
)

# ---------------------------------------------------------------------------
# Corpus: one project per axis. `conf` holds extra confoverrides merged over
# BASE_CONFOVERRIDES; `files` maps docname -> rst source (nested docnames
# like "sub/b" get written to sub/b.rst).
# ---------------------------------------------------------------------------

PROJECTS = [
    {
        "name": "toctree_nested",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. toctree::

   a
   b
""",
            "a": """\
A
=

.. toctree::

   a1
   a2
""",
            "a1": """\
A1
==

Leaf content for a1.
""",
            "a2": """\
A2
==

Leaf content for a2.
""",
            "b": """\
B
=

Leaf content for b.
""",
        },
    },
    {
        "name": "toctree_glob",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. toctree::
   :glob:

   pages/*
""",
            "pages/a": """\
Page A
======

Leaf content for page a.
""",
            "pages/b": """\
Page B
======

Leaf content for page b.
""",
        },
    },
    {
        "name": "toctree_numbered",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. toctree::
   :numbered:

   a
   b
""",
            "a": """\
A
=

Sub
---

Text under sub.
""",
            "b": """\
B
=

Leaf content for b.
""",
        },
    },
    {
        "name": "toctree_numbered_depth2",
        "conf": {"numfig": True, "numfig_secnum_depth": 2},
        "files": {
            "index": """\
Index
=====

.. toctree::
   :numbered:

   a
""",
            "a": """\
A
=

Sub
---

SubSub
~~~~~~

.. figure:: pic.png
   :name: fig-one

   First figure.

.. figure:: pic2.png
   :name: fig-two

   Second figure.
""",
        },
    },
    {
        "name": "toctree_self_ref",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. toctree::

   index
   a
""",
            "a": """\
A
=

Leaf content for a.
""",
        },
    },
    {
        "name": "toctree_circular",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. toctree::

   a
""",
            "a": """\
A
=

.. toctree::

   b
""",
            "b": """\
B
=

.. toctree::

   a
""",
        },
    },
    {
        "name": "toctree_multi_parent",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. toctree::

   a
   b
""",
            "a": """\
A
=

.. toctree::

   c
""",
            "b": """\
B
=

.. toctree::

   c
""",
            "c": """\
C
=

Leaf content for c, referenced from two parents.
""",
        },
    },
    {
        "name": "orphan_doc",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. toctree::

   included
""",
            "included": """\
Included
========

This document is properly included in the toctree.
""",
            "non_orphan": """\
Not Orphan
==========

This document is not included in any toctree and lacks the orphan marker.
""",
            "orphan": """\
:orphan:

Orphan
======

This document is not included in any toctree but is marked orphan.
""",
        },
    },
    {
        "name": "numfig_on",
        "conf": {
            "numfig": True,
            "numfig_format": {"figure": "Figure %s", "table": "Table {number}"},
        },
        "files": {
            "index": """\
Index
=====

.. toctree::

   a
   b
""",
            "a": """\
A
=

.. figure:: pic.png
   :name: fig-a

   The First Figure

.. list-table:: The First Table
   :name: tab-a
   :header-rows: 1

   * - Col1
     - Col2
   * - x
     - y

.. code-block:: python
   :name: code-a
   :caption: The First Listing

   x = 1
""",
            "b": """\
B
=

See :numref:`fig-a` for the default figure format.

See :numref:`tab-a` for the default table format.

See :numref:`Custom {name} number {number} <fig-a>` for an explicit new-style reference.

See :numref:`Old style %s <tab-a>` for an explicit old-style reference.

See :numref:`code-a` for the listing.
""",
        },
    },
    {
        "name": "numfig_off_numref",
        "conf": {"numfig": False},
        "files": {
            "index": """\
Index
=====

.. toctree::

   a
   b
""",
            "a": """\
A
=

.. figure:: pic.png
   :name: fig-a

   A Figure
""",
            "b": """\
B
=

See :numref:`fig-a` here.
""",
        },
    },
    {
        "name": "labels_dups",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. toctree::

   a
   b
""",
            "a": """\
A
=

.. _dup-label:

Section One
-----------

Text in section one.
""",
            "b": """\
B
=

.. _dup-label:

Section Two
-----------

Text in section two.
""",
        },
    },
    {
        "name": "glossary_terms",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. toctree::

   a
   b
""",
            "a": """\
A
=

.. glossary::

   environment
      A structure where information about all documents under the root is
      saved.

   template engine
      Renders templates into output files.
""",
            "b": """\
B
=

See the :term:`environment` term.

See the :term:`Environment` term (case-insensitive fallback).

See the :term:`nonexistent term` here.
""",
        },
    },
    {
        "name": "std_objects",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. toctree::

   a
   b
""",
            "a": """\
A
=

.. program:: myprog

.. option:: --verbose

   Enables verbose output.

.. option:: --quiet

   Enables quiet output.

.. program:: None

.. option:: --global-opt

   A global (unscoped) option.

.. envvar:: HOME_A

   Home directory variable.

.. confval:: my_setting

   A config value.

.. describe:: widget

   A generic described object.
""",
            "b": """\
B
=

Use :option:`myprog --verbose` for the scoped option.

Use :option:`--global-opt` for the unscoped fallback.

Use :option:`--missing-option` here.

See :envvar:`HOME_A` for details.

See :confval:`my_setting` for the setting.
""",
        },
    },
    {
        "name": "index_entries",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. toctree::

   a
""",
            "a": """\
A
=

.. index::
   single: Alpha
   pair: bread; butter
   triple: fast; car; red
   see: Widget; Gadget
   seealso: Foo; Bar
   ! Important
   _private
   42answer

Text with indexed content.
""",
        },
    },
    {
        "name": "doc_refs",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. toctree::

   a
   sub/b
   sub/c
""",
            "a": """\
A
=

See :doc:`/sub/b` for the absolute reference.

See :doc:`missing-doc` for the unknown reference.
""",
            "sub/b": """\
Sub B
=====

See :doc:`c` for the relative reference.

See :doc:`/a` for the absolute reference back to the root-level doc.
""",
            "sub/c": """\
Sub C
=====

Leaf content for sub/c.
""",
        },
    },
]


def write_project_files(base: Path, files: dict) -> None:
    for docname, text in files.items():
        path = base / f"{docname}.rst"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")


def normalize(text: str, base: Path) -> str:
    for form in {str(base), str(base.resolve())}:
        text = text.replace(form, SOURCE_TOKEN)
    assert str(base) not in text and str(base.resolve()) not in text, (
        f"srcdir path leaked into captured text:\n{text}"
    )
    return text


def dump_std(env) -> dict:
    data = env.domaindata.get("std", {})

    labels = {
        name: [docname, labelid, str(sectionname)]
        for name, (docname, labelid, sectionname) in data.get("labels", {}).items()
    }
    anonlabels = {
        name: list(value) for name, value in data.get("anonlabels", {}).items()
    }
    terms = {name: list(value) for name, value in data.get("terms", {}).items()}

    objects = sorted(
        (
            {
                "objtype": objtype,
                "name": name,
                "docname": docname,
                "labelid": labelid,
            }
            for (objtype, name), (docname, labelid) in data.get(
                "objects", {}
            ).items()
        ),
        key=lambda e: (e["objtype"], e["name"]),
    )
    progoptions = sorted(
        (
            {
                "program": program,
                "name": name,
                "docname": docname,
                "labelid": labelid,
            }
            for (program, name), (docname, labelid) in data.get(
                "progoptions", {}
            ).items()
        ),
        key=lambda e: (e["program"] or "", e["name"]),
    )

    return {
        "labels": labels,
        "anonlabels": anonlabels,
        "objects": objects,
        "progoptions": progoptions,
        "terms": terms,
    }


def dump_index_entries(env) -> dict:
    entries = env.domaindata.get("index", {}).get("entries", {})
    return {
        docname: [list(entry) for entry in doc_entries]
        for docname, doc_entries in entries.items()
    }


def dump_genindex(genindex) -> list:
    out = []
    for group_key, entries in genindex:
        entry_list = []
        for entry_name, (targets, subitems, category_key) in entries:
            entry_list.append(
                {
                    "name": entry_name,
                    "targets": [list(t) for t in targets],
                    "subitems": [
                        {"name": subname, "targets": [list(t) for t in subtargets]}
                        for subname, subtargets in subitems
                    ],
                    "category_key": category_key,
                }
            )
        out.append({"group": group_key, "entries": entry_list})
    return out


def build_project(entry: dict) -> dict:
    base = Path(tempfile.mkdtemp(prefix="env_oracle_srcdir_")).resolve() / "src"
    base.mkdir(parents=True)
    (base / "conf.py").write_text(CONF_PY, encoding="utf-8")
    write_project_files(base, entry["files"])

    confoverrides = {**BASE_CONFOVERRIDES, **entry.get("conf", {})}

    resolved_raw: dict = {}

    def capture_write_doc(docname, doctree):
        resolved_raw[docname] = doctree.pformat()

    with docutils_namespace(), patch_docutils(str(base)):
        app = SphinxTestApp(
            buildername="dummy",
            srcdir=base,
            status=io.StringIO(),
            warning=io.StringIO(),
            confoverrides=dict(confoverrides),
        )
        try:
            # Instance-attribute override: DummyBuilder.write_doc is a
            # no-op, but the base Builder.write() loop always resolves the
            # doctree (post-transforms + toctree resolution) before handing
            # it to write_doc -- capturing here is the ONE real resolution
            # pass a build performs, so warnings are not double-fired.
            app.builder.write_doc = capture_write_doc

            app.build()

            env = app.env

            # `BuildEnvironment.collect_relations()` -> `_traverse_toctree`
            # (environment/__init__.py) only guards against an *immediate*
            # self-parent (`parent == docname`); a genuine multi-doc mutual
            # cycle (A includes B, B includes A) has no "already visited"
            # check before recursing, so it recurses without bound and
            # raises RecursionError -- a real, verified sphinx 9.1.0
            # limitation for the toctree_circular project (confirmed: a
            # real `sphinx-build -b html` over the same two-doc mutual
            # toctree would hit the same crash computing rellinks). The
            # write-phase toctree *resolution* path (`_resolve_toctree` /
            # `_toctree_entry`, adapters/toctree.py) has a correct
            # depth-bounded cycle guard and already ran cleanly inside
            # `app.build()` above (see the "circular toctree references
            # detected" warning it emits). Recording `relations: null` for
            # this one project is the oracle's honest answer: the real
            # attribute is uncomputable for this construct.
            try:
                relations = env.collect_relations()
            except RecursionError:
                relations = None

            genindex = IndexEntries(env).create_index(app.builder)

            warnings_text = normalize(app.warning.getvalue(), base)
            warnings = [line for line in warnings_text.splitlines() if line.strip()]

            tocs_pformat = {
                docname: normalize(toc.pformat(), base)
                for docname, toc in env.tocs.items()
            }
            resolved_pformat = {
                docname: normalize(text, base)
                for docname, text in resolved_raw.items()
            }

            expect = {
                "toctree_includes": dict(env.toctree_includes),
                "files_to_rebuild": {
                    docname: sorted(containers)
                    for docname, containers in env.files_to_rebuild.items()
                },
                "relations": relations,
                "tocs_pformat": tocs_pformat,
                "toc_num_entries": dict(env.toc_num_entries),
                "toc_secnumbers": {
                    docname: {
                        anchor: list(num) for anchor, num in secnums.items()
                    }
                    for docname, secnums in env.toc_secnumbers.items()
                },
                "toc_fignumbers": {
                    docname: {
                        figtype: {
                            fig_id: list(num) for fig_id, num in fignums.items()
                        }
                        for figtype, fignums in by_type.items()
                    }
                    for docname, by_type in env.toc_fignumbers.items()
                },
                "std": dump_std(env),
                "index_entries": dump_index_entries(env),
                "genindex": dump_genindex(genindex),
                "resolved_pformat": resolved_pformat,
                "warnings": warnings,
            }
        finally:
            app.cleanup()
            shutil.rmtree(base.parent, ignore_errors=True)

    return {
        "name": entry["name"],
        "conf": confoverrides,
        "files": entry["files"],
        "expect": expect,
    }


def generate_all() -> dict:
    out_projects = [build_project(entry) for entry in PROJECTS]
    return {
        "sphinx_version": sphinx.__version__,
        "docutils_version": docutils.__version__,
        "generator": "tools/gen_env_fixture.py",
        "projects": out_projects,
    }


def main() -> int:
    names = [p["name"] for p in PROJECTS]
    assert len(names) == len(set(names)), "project names must be unique"
    assert len(PROJECTS) >= 12, f"corpus degenerated: {len(PROJECTS)} projects"

    fixture = generate_all()

    # In-process determinism check: a second full pass over the entire
    # corpus (fresh tmpdirs, fresh SphinxTestApps) must be byte-identical.
    again = generate_all()
    first_json = json.dumps(fixture, indent=2, sort_keys=True, ensure_ascii=False)
    second_json = json.dumps(again, indent=2, sort_keys=True, ensure_ascii=False)
    if first_json != second_json:
        print("DETERMINISM VIOLATION: two in-process passes differ", file=sys.stderr)
        return 1

    out_path = (
        Path(__file__).resolve().parent.parent
        / "tests"
        / "fixtures"
        / "env_differential.json"
    )
    with open(out_path, "w", encoding="utf-8") as f:
        f.write(first_json)
        f.write("\n")
    print(
        f"wrote {out_path}: {len(fixture['projects'])} projects, "
        f"sphinx {sphinx.__version__}, docutils {docutils.__version__}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
