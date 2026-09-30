#!/usr/bin/env python3
"""Generate tests/fixtures/env_differential.json from Sphinx 9.1.0.

Regenerate with:

    PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' \
        --with 'docutils==0.22.4' python tools/gen_env_fixture.py

PYTHONNOUSERSITE=1 is NOT optional: `uv run` keeps the user's site-packages
on sys.path, and a user-site Pygments there silently re-records every
`code:: python` case as tokenized output. Regenerating without the flag
produces spurious fixture churn.

THE ENVIRONMENT-LAYER ORACLE. Where tools/gen_sphinx_fixture.py records
per-SNIPPET read-phase pseudo-XML, this generator records per-PROJECT
`BuildEnvironment` state: the toctree graph, relations, section/figure
numbering, std-domain object/label registries, the index/genindex adapters,
and fully cross-reference-resolved doctrees. A "project" is a small
multi-document srcdir (dict of docname -> rst source + conf overrides),
built with a real `SphinxTestApp(buildername='dummy')` + `app.build()` --
exactly what a real `sphinx-build` does for its read + resolve phases, minus
writing output files.

ORACLE VENUE NOTE (wave 4.5): projects here are ALSO the oracle venue for
the file-inserting directives (`include`/`literalinclude`) -- the two
doctree fixtures are string corpora that cannot carry the aux files those
directives read, so their node shapes (`:literal:` with `:name:`/
`:number-lines:`, `:code:`, SEVERE error shapes) belong to this fixture's
inc_* projects (T14) plus unit/e2e tests.

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
    `warning_records` (wave 5) is the same stream one entry per RECORD --
    each write of Sphinx's warning handler, minus its line terminator --
    so a multi-line docutils message keeps its boundaries and blank lines;
    the generator asserts that splitting it reproduces `warnings`.
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
Since wave 4.5 (plan Scope-8) the CWD-RELATIVE spelling of the srcdir is
replaced too: docutils' `adapt_path` spells every path of *included* content
relative to the process cwd (node `source` attrs, circular-inclusion chain
bodies, SEVERE error texts, the double-parse warning duplicates), which is
environment-dependent in exactly the way the absolute path is. The consumer
(tests/env_differential.rs) applies the mirror normalization to sphinx-ultra's
own srcdir-relative spellings by collapsing `<project>/` on both sides of the
warning and resolved-pformat comparisons, making "srcdir-relative" the
canonical spelling for both.

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

# The `literalinclude` corpus's Python source, shipped through `data_files`
# (same geometry as tests/fixtures/literalinclude/example.py, which the unit
# tests use). Three properties are load-bearing and must survive edits:
#
#   * it PARSES (`ast.parse` clean). A file that tokenizes but does not parse
#     yields `:pyobject:` tags here while Sphinx's own analyzer warns instead
#     (wave-4.5 task 15 ruling) -- a divergence no oracle can express.
#   * indentation is spaces only, uniformly 4. The one ledgered TAG-changing
#     divergence is `:tab-width:` (!= 8) + `:pyobject:` + mixed indentation;
#     keeping tabs out of the file keeps that trap unreachable.
#   * `Foo.method` is a nested definition and `tail` follows the class, so
#     `:pyobject: Foo.method` exercises the nested-def path and its end
#     boundary is a real dedent rather than EOF.
EXAMPLE_PY = '''\
"""Example module."""

CONST = 1


def top(x):
    """Top function."""
    return x + 1


class Foo:
    """A class."""

    attr = 2

    def method(self):
        return self.attr


def tail():
    pass
'''

# ---------------------------------------------------------------------------
# Corpus: one project per axis. `conf` holds extra confoverrides merged over
# BASE_CONFOVERRIDES; `files` maps docname -> rst source (nested docnames
# like "sub/b" get written to sub/b.rst). An optional `data_files` map
# (relative path -> literal text) ships non-document members -- the .py /
# .inc / .png files the include and literalinclude projects read. Data
# files must NOT use the .rst/.md/.txt suffixes: sphinx-ultra's discovery
# is wider than Sphinx's default `source_suffix` (it also admits .md and
# .txt), so a .txt member would become a document on one side only and the
# resolved-document key sets would diverge by construction.
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
    # -----------------------------------------------------------------------
    # Wave 4.5: py-domain projects (plan Task 14). Documents that carry a
    # `py:module` directive are PropagateTargets-visible (plan Scope-3: the
    # module target's ids migrate onto the following node in Sphinx's tree,
    # a transform this crate defers to wave 5), so their resolved_pformat is
    # gap-tabled in the consumer while every other key — registration
    # records, modindex, tocs, warnings, xref-bearing sibling documents —
    # compares in full.
    # -----------------------------------------------------------------------
    {
        # module + class + method + functions registered in NON-alphabetical
        # order (zeta before alpha) so the registration-order semantics of
        # py_objects/py_modules are pinned; doc b resolves xrefs against
        # them, including the ambiguous fuzzy `.same` ref -> warning.
        "name": "py_basic",
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

.. py:module:: zmod

.. py:class:: Widget

   .. py:method:: render(x)

.. py:function:: zeta.same()

.. py:function:: alpha.same()

.. py:function:: helper(arg=1)
""",
            "b": """\
B
=

See :py:class:`zmod.Widget` and :py:meth:`~zmod.Widget.render` and
:py:func:`zmod.helper` and :py:mod:`zmod` and :py:func:`.same`.
""",
        },
    },
    {
        # Duplicate objects ACROSS documents (dupfn, dupmod: a then b,
        # last-wins in a's insertion slot) plus a duplicate module WITHIN
        # one document (b's second dupmod -> node id falls back to the
        # `module-0` serial) — [PY spec section 8 item 3] warning bytes and
        # the last-wins registry both land in the fixture.
        "name": "py_dup",
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

.. py:function:: dupfn()

.. py:class:: Keeper

.. py:module:: dupmod
""",
            "b": """\
B
=

.. py:function:: dupfn()

.. py:module:: dupmod

.. py:module:: dupmod
""",
        },
    },
    {
        # The [SIG A.2] toc-object-entries project, default config: the
        # class/method/function entries join env.tocs with the shared
        # anchorname counter and `skip_section_number` stamps.
        "name": "py_toc",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. toctree::

   mod
""",
            "mod": """\
Mod
===

.. py:module:: pkg.mymod

.. py:class:: MyClass

   Class body.

   .. py:method:: my_method(arg)

      Method body.

.. py:function:: my_func(x)

   Function body.
""",
        },
    },
    {
        # The same files under `toc_object_entries_show_parents = 'all'`
        # (an enum string, -D-expressible): every toc entry spells its full
        # dotted path.
        "name": "py_toc_parents",
        "conf": {"toc_object_entries_show_parents": "all"},
        "files": {
            "index": """\
Index
=====

.. toctree::

   mod
""",
            "mod": """\
Mod
===

.. py:module:: pkg.mymod

.. py:class:: MyClass

   Class body.

   .. py:method:: my_method(arg)

      Method body.

.. py:function:: my_func(x)

   Function body.
""",
        },
    },
    {
        # The [PY section 4] modindex_shapes project: dummy parent (orphan,
        # subtype 1 with empty fields), parent promotion (pkg -> subtype 1),
        # submodules carrying platform / synopsis / deprecated fields, and
        # collapse=False (3 submodules vs 2 top-levels: 5-2=3 < 2 is false).
        "name": "py_modindex",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. py:module:: pkg

.. py:module:: pkg.sub
   :synopsis: Sub synopsis.

.. py:module:: pkg.sub2
   :platform: Windows

.. py:module:: orphan.child

.. py:module:: zzz
   :deprecated:
""",
        },
    },
    {
        # The [PY section 4] modindex_common_prefix project, exercising the
        # array-conf harness extension: `pkg.` is stripped for sorting and
        # bucketing while display names keep it, and every module counts as
        # top-level -> collapse=True.
        "name": "py_modindex_prefix",
        "conf": {"modindex_common_prefix": ["pkg."]},
        "files": {
            "index": """\
Index
=====

.. py:module:: pkg.aaa

.. py:module:: pkg.bbb

.. py:module:: other
""",
        },
    },
    {
        # nitpicky mode: missing py refs warn in the generic non-std shape
        # with [ref.{typ}], while builtin_resolver silences `int` (no
        # warning, literal kept with no reference wrapper). Offline —
        # nitpicky only, no intersphinx.
        "name": "py_nitpicky",
        "conf": {"nitpicky": True},
        "files": {
            "index": """\
Index
=====

Ref :py:func:`missing_fn` and :py:class:`int` and :py:class:`Missing`.
""",
        },
    },
    # -----------------------------------------------------------------------
    # Wave 4.5: include / literalinclude projects (plan Task 14). These are
    # the oracle venue for the file-inserting directives' node shapes — the
    # doctree fixtures are string corpora that cannot carry aux files. Path
    # spellings ride the Scope-8 normalization: srcdir-relative is the
    # canonical form on both sides of the warning/resolved comparisons.
    # -----------------------------------------------------------------------
    {
        # In-tree message and node shapes, under `keep_warnings=True` so the
        # docutils reporter messages STAY in the resolved doctrees where both
        # sides can compare them byte-for-byte: nested include chains, the
        # srcdir-absolute form, a missing file (InputError SEVERE), a failed
        # `:start-after:` clip (Text not found SEVERE), a circular include
        # pair, `:literal:` with `:name:`/`:number-lines:`, the two
        # language-less `:code:` forms (plain, and `:number-lines:` with
        # docutils' width quirk), literalinclude `:pyobject:`/`:lineno-match:`,
        # a `:pyobject:` miss (the T15 not-found text), and a captioned
        # `:lines:`+`:emphasize-lines:` block. `sub/nested.rst` pins
        # §Scope-2a docname-relative resolution: `shared/frag.rst` includes
        # `frag2.rst`, which resolves against the CURRENT DOCUMENT's
        # directory — `sub/frag2.rst` (exists) when included from
        # `sub/nested`, `shared/frag2.rst` (missing -> SEVERE) when
        # `shared/frag` is parsed standalone.
        #
        # Every one of the project's warning records is a docutils reporter
        # message (`[docutils]`), printed at creation and — under
        # `keep_warnings` — kept in the resolved doctrees too, so the same
        # bytes are compared twice: as records and as `system_message`
        # nodes (the tree keeps the DirectiveError literal the record lacks).
        "name": "inc_basic",
        "conf": {"keep_warnings": True},
        "files": {
            "index": """\
Index
=====

.. toctree::

   a
   b
   sub/nested
""",
            "a": """\
A
=

.. include:: chain1.rst

.. include:: /abs_part.rst

.. include:: missing.rst

.. include:: clip_part.inc
   :start-after: nope-not-here

.. include:: circ_a.rst

.. literalinclude:: example.py
   :pyobject: not_there
""",
            "b": """\
B
=

.. include:: lit_part.inc
   :literal:
   :name: lit-block

.. include:: numbered.inc
   :literal:
   :number-lines:

.. include:: code_plain.inc
   :code:

.. include:: code_numbered.inc
   :code:
   :number-lines:

.. literalinclude:: example.py
   :pyobject: Foo.method
   :lineno-match:

.. literalinclude:: example.py
   :lines: 6-8
   :emphasize-lines: 2
   :caption: Example tail
""",
            "sub/nested": """\
Nested
======

.. include:: ../shared/frag.rst
""",
            "chain1": """\
Chain one.

.. include:: chain2.rst
""",
            "chain2": """\
Chain two.
""",
            "abs_part": """\
Absolute part.
""",
            "circ_a": """\
Circ A.

.. include:: circ_b.rst
""",
            "circ_b": """\
Circ B.

.. include:: circ_a.rst
""",
            "shared/frag": """\
Frag paragraph one.

.. include:: frag2.rst
""",
            "sub/frag2": """\
Frag2 paragraph.
""",
        },
        "data_files": {
            "clip_part.inc": "clip alpha\nclip beta\n",
            "lit_part.inc": "Literal alpha.\nLiteral beta.\n",
            # Ten lines in BOTH numbered members, so the two number-column
            # widths differ visibly: `:literal:` sizes the column from the
            # real line count (width 2, `11`), while `:code:` goes through
            # the `code` directive with `len(self.content) == 1` and sizes
            # it from `startline + 1` -- docutils' single-element quirk,
            # a ragged width-1 column running past `9`.
            "numbered.inc": (
                "num one\nnum two\nnum three\nnum four\nnum five\n"
                "num six\nnum seven\nnum eight\nnum nine\nnum ten\n"
            ),
            "code_plain.inc": "plain code line\nsecond code line\n",
            "code_numbered.inc": (
                "line one\nline two\nline three\nline four\nline five\n"
                "line six\nline seven\nline eight\nline nine\nline ten\n"
            ),
            "example.py": EXAMPLE_PY,
        },
    },
    {
        # The warning streams both sides CAN compare byte-for-byte: the
        # three logger-channel literalinclude warnings (`:lines:` out of
        # range, `:emphasize-lines:` out of range against the post-filter
        # count, `non-whitespace stripped by dedent`) with their
        # doc2path-doubled `.rst.rst` locations, plus the sphinx-channel
        # warnings an INCLUDED .rst fires under both spellings: `part.rst`
        # registers `partfn` via the include into `a` and again as its own
        # standalone document (-> duplicate object warning naming `a`), and
        # its dangling :ref: resolves — and warns — once per resolved
        # document. The included file's orphan warning is suppressed
        # (env.included consult), which this project also pins.
        "name": "inc_warn",
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

.. include:: part.rst

.. literalinclude:: snippet.inc
   :lines: 1-40

.. literalinclude:: snippet.inc
   :emphasize-lines: 9

.. literalinclude:: snippet.inc
   :dedent: 2
""",
            "part": """\
Part
----

.. py:function:: partfn()

See :ref:`missing-target`.
""",
        },
        "data_files": {
            "snippet.inc": "alpha\nbeta\ngamma\n",
        },
    },
    {
        # The [INC PROBE 6] project, COLD state: include + literalinclude +
        # image dependencies land in env.dependencies (absolute, normalized
        # to <project>/...), the included docname in env.included. The
        # warm-rebuild/outdated matrix stays in e2e and the harness's own
        # incremental tests — a fixture build is always cold.
        #
        # NOT here, deliberately: the standard-include (`<isogrk4.txt>`)
        # no-record rule. Sphinx's Include hands the `<name>` form to the
        # docutils base directive before reaching `note_included`, so
        # `env.included` stays empty (probe-confirmed) — but docutils' own
        # `record_dependencies` DOES take the file, and `note_dependency`
        # resolves that against the srcdir, so `env.dependencies` ends up
        # holding `<project>/../../…/site-packages/docutils/parsers/rst/
        # include/isogrk4.txt`: an interpreter-installation path no
        # normalization can canonicalize, which would make the committed
        # fixture machine-specific. That rule stays with the unit tests.
        "name": "inc_deps",
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

.. include:: part.rst

.. literalinclude:: example.py
   :lines: 1-2

.. image:: pic.png
""",
            "b": """\
B
=

no deps here
""",
            "part": """\
part para
""",
        },
        "data_files": {
            "example.py": EXAMPLE_PY,
            "pic.png": "not really a png\n",
        },
    },
    # -----------------------------------------------------------------------
    # Panel fix round B additions.
    # -----------------------------------------------------------------------
    {
        # [23] `:any:` resolution, the wave-4.5 deliverable that had no
        # oracle coverage: a py:func hit (bare and with `()`, which
        # `find_obj` strips), a py:mod hit, a py:data hit, a std label hit
        # (lowercased ref lookup), a doc hit (std's doc-first branch), the
        # winner's extended literal classes (`xref any py py-func`,
        # `std std-ref`, `doc doc doc`), a std/py ambiguity — a label and a
        # function both named `dup` — with its ` or `-joined `[ref.any]`
        # warning, and a dangling target (`:any:` is warn_dangling, so it
        # warns WITHOUT nitpicky). No `:module:` option and no py:module
        # scope around the refs: the two shapes the T11 report keeps on the
        # avoid-list (round A closed the py:module-None key edge in the
        # sphinx doctree corpus) are not needed to pin any of this.
        #
        # Layout: every reference lives in `a`, which therefore compares at
        # FULL strength; the definitions live in `b`, whose resolved tree
        # is Scope-3 propagation-visible (the `.. _dup:` label's ids move
        # onto its section, a KNOWN_RESOLVED_GAPS shape) — the module sits
        # LAST in `b`, the py_dup shape, so its target has nothing to
        # propagate onto, and the label's name differs from its section's
        # slug so the section's FIRST id (the toc anchor) still agrees.
        "name": "py_any",
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

Hits: :any:`f` and :any:`f()` and :any:`mod` and :any:`item`
and :any:`std-label` and :any:`index`.

Ambiguous: :any:`dup`. Dangling: :any:`nosuch_any`.
""",
            "b": """\
B
=

.. _dup:

Dup Section
-----------

.. py:function:: mod.f()

.. py:function:: mod.dup()

.. py:data:: mod.item

.. _std-label:

Only Label
----------

Text.

.. py:module:: mod
""",
        },
    },
    {
        # [4]/[5] warning locations for the xrefs the py directives
        # synthesize, under nitpicky: an annotation xref (parameter,
        # return, `:type:` on py:data) locates at its signature's own
        # file:line — the INCLUDED file's for a signature inside an include
        # — while a doc-field xref locates through docutils'
        # `get_source_line` ancestor walk (the synthesized nodes and the
        # desc/desc_content above them carry no provenance): the
        # enclosing section's underline, a `.. note::` line, or the
        # INCLUDER's section for a field written in an included file — and
        # NO location at all for a description that sits directly under
        # the document (the first `WARNING:` line below has no prefix). An
        # inline role in a field body keeps its own line. Single document,
        # no toctree, so every key compares at full strength.
        "name": "py_locations",
        "conf": {"nitpicky": True},
        "files": {
            "index": """\
.. py:function:: top(q: nosuch_top)

   :param nosuch_top_field q: q

Top
===

.. include:: part.inc

Sec
---

.. py:function:: g(y: alsomissing) -> retmissing

   :param nosuch_doc y: d
   :param k: see :py:class:`nosuch_inline`
   :rtype: nosuch_rt

.. py:data:: d
   :type: typemissing

.. note::

   .. py:function:: h(z)

      :param nosuch_note z: q
""",
        },
        "data_files": {
            "part.inc": (
                ".. py:function:: f(x: missingtype)\n"
                "\n"
                "   :param nosuch_inc z: q\n"
            ),
        },
    },
    {
        # [21] literal_blocks whose `language`/`force`/`linenos` are ALL
        # directive-set, in a document outside both exemption tables, so
        # the three attributes are compared at full strength somewhere in
        # the corpus (HighlightLanguageTransform stamps only what a
        # directive left unset: `code.py:81-86`). literalinclude with
        # `:language:` + `:linenos:`, with `:language:` + `:lineno-start:`
        # + `:force:`, and a code-block with `:linenos:`. Single document,
        # no toctree.
        "name": "inc_highlight",
        "conf": {},
        "files": {
            "index": """\
Index
=====

.. literalinclude:: example.py
   :language: python
   :linenos:
   :lines: 1-3

.. literalinclude:: example.py
   :language: text
   :lineno-start: 5
   :force:
   :lines: 4-5

.. code-block:: python
   :linenos:

   x = 1
""",
        },
        "data_files": {
            "example.py": EXAMPLE_PY,
        },
    },
    # -----------------------------------------------------------------------
    # Panel fix round D: the docutils target-marker name rule and Python's
    # `\s` in every name/target normalizer, at environment level. Single
    # document, no toctree. Every bare target is followed by a comment, so
    # `PropagateTargets` donates nothing and the resolved doctree compares
    # at full strength. What it pins:
    #   - `.. _ pad  lbl :` / `.. _ a b :` / `.. _ only comment :` are
    #     COMMENTS (`\.\.[ ]+_(?![ ]|$)`): no duplicate-target message
    #     beside the real `.. _pad  lbl:`, and `only comment` registers
    #     nothing, so both `:ref:` spellings of it warn `undefined label`.
    #   - `.. _a\x1fb:` is the label `a b` (docutils `str.split()`), reached
    #     by `:ref:`AB <a b>`` and by `:ref:`AB2 <a\x1fb>`` (sphinx `ws_re`).
    #   - `.. envvar:: FOO\x1fBAR` registers `FOO BAR` (`ws_re.sub(' ', sig)`),
    #     reached by both `:envvar:` spellings.
    #   - `.. program:: git\x1fadd` scopes `-x` under `git-add`
    #     (`ws_re.sub('-', …)`); `:option:`git\x1fadd -x`` folds the
    #     subcommand off on the \x1f (`ws_re.split(target, maxsplit=1)`) and
    #     resolves, as does the spelled-out `:option:`git-add -x``.
    # -----------------------------------------------------------------------
    {
        "name": "names_round_d",
        "conf": {},
        "files": {
            "index": """\
Round D names
=============

Labels: :ref:`Pad <pad lbl>`, :ref:`AB <a b>` and :ref:`AB2 <a\x1fb>`.

Dangling: :ref:`X <only comment>` and :ref:`only comment`.

Environment: :envvar:`FOO BAR` and :envvar:`FOO\x1fBAR`.

Option: :option:`git\x1fadd -x` and :option:`git-add -x`.

.. envvar:: FOO\x1fBAR

   Variable.

.. program:: git\x1fadd

.. option:: -x

   Option.

.. _pad  lbl:
.. _ pad  lbl :

.. _a\x1fb:
.. _ a b :

.. _ only comment :
""",
        },
    },
    # -----------------------------------------------------------------------
    # Wave 5 (sub-project 1, Task 4): the diagnostics stream, compared per
    # RECORD through `warning_records`. Each project below sets
    # `keep_warnings` so the reporter messages also stay in the resolved
    # doctrees, where both sides compare them node for node.
    # -----------------------------------------------------------------------
    {
        # One document (`second`) whose read prints every parse-time
        # channel, interleaved in creation order: docutils reporter records
        # (inline WARNINGs, a DirectiveError ERROR printed without its
        # literal, an option-parse ERROR printed WITH its literal and so
        # spanning a blank line), a toctree logger record, and the
        # registration duplicates Sphinx logs from inside the directives at
        # parse time (`note_object`: py function, std envvar, glossary term
        # — the term between its own inline parse and its definition's).
        # After the whole parse stream: `IndexDomain.process_doc`'s invalid
        # entry, then `StandardDomain.process_doc`'s duplicate label — the
        # SphinxDomains order. Documents are read sorted, so `first` (the
        # first object instances) and `index` (the first `shared` label;
        # its resolved doctree already diverges on its toctree) are read
        # before `second`. The label is a section's (`PropagateTargets`
        # shape) rather than a rubric's `:name:`: docutils leaves a rubric
        # without a line, which Sphinx prints as `x.rst:: WARNING: duplicate
        # label ...` — a separate node-location quirk, not this axis. The
        # section title differs from the label so the two names do not
        # collide (a duplicate implicit name would move the toc anchor).
        # Probe: docs/superpowers/research/probes/probe-reporter-oracle/p6.
        "name": "reporter_interleave",
        "conf": {"keep_warnings": True},
        "files": {
            "index": """\
Index
=====

.. toctree::

   first
   second

.. _shared:

Shared section
--------------
""",
            "first": """\
First
=====

.. py:function:: dup()

.. envvar:: DUPVAR

.. glossary::

   gterm
      First.
""",
            "second": """\
Second
======

Para *bad one.

.. py:function:: dup()

.. toctree::

   missing-doc

.. envvar:: DUPVAR

.. glossary::

   gterm
      Second *bad in the definition.

.. note::

.. note::
   :bogus: x

   Body.

.. index:: single:

.. _shared:

Shared section
--------------

Para *bad two.
""",
        },
    },
    {
        # A missing `include` target: docutils raises a SEVERE
        # DirectiveError, which Sphinx prints as `CRITICAL` (without the
        # literal the tree message carries) and counts as a warning.
        "name": "inc_missing",
        "conf": {"keep_warnings": True},
        "files": {
            "index": """\
Index
=====

.. include:: missing.rst

After.
""",
        },
    },
    {
        # Reporter records raised INSIDE included files, two levels deep,
        # between records of the files around them: each record names the
        # file (and line) it was raised in, and the stream stays in
        # creation order across the three sources.
        "name": "inc_nested_error",
        "conf": {"keep_warnings": True},
        "files": {
            "index": """\
Index
=====

Before *bad.

.. include:: outer.inc

After *bad.
""",
        },
        "data_files": {
            "outer.inc": "Outer para.\n\n.. include:: inner.inc\n\nOuter *bad.\n",
            "inner.inc": "Inner para.\n\n.. note::\n\nInner *bad.\n",
        },
    },
    # -----------------------------------------------------------------------
    # Wave 5 (sub-project 1, Task 6): `keep_warnings`, the level
    # FilterSystemMessages (`transforms/__init__.py:337-347`, priority 999)
    # filters `system_message` nodes below — 2 when on, 5 (everything) when
    # off. The same document under each setting, carrying one WARNING-level
    # message (an inline markup error, in the tree after its paragraph) and
    # one INFO-level message (a duplicate implicit section name, in the
    # second section after its title): the resolved doctree keeps only the
    # WARNING under `True`, and no `system_message` at all under `False`.
    # The printed stream is the same WARNING record under both (INFO never
    # prints; printing does not depend on the tree).
    # -----------------------------------------------------------------------
    {
        "name": "keep_warnings_true",
        "conf": {"keep_warnings": True},
        "files": {
            "index": """\
Index
=====

Para *bad.

Dup
---

x

Dup
---

y
""",
        },
    },
    {
        "name": "keep_warnings_false",
        "conf": {"keep_warnings": False},
        "files": {
            "index": """\
Index
=====

Para *bad.

Dup
---

x

Dup
---

y
""",
        },
    },
]


def write_project_files(base: Path, files: dict, data_files: dict) -> None:
    for docname, text in files.items():
        path = base / f"{docname}.rst"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    for relpath, text in data_files.items():
        path = base / relpath
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")


def srcdir_spellings(base: Path) -> list:
    """Every spelling of the srcdir that can leak into captured text.

    Absolute forms (raw + resolved) plus the CWD-RELATIVE forms (Scope-8):
    docutils' `adapt_path` (`utils.relative_path(None, path)`) spells the
    paths of included content relative to `os.getcwd()`, so an include
    project's warnings, chain bodies and node `source` attributes carry a
    `../../..`-style prefix down into the tmp srcdir. Longest-first so an
    overlapping pair (`/var/...` inside macOS's resolved `/private/var/...`)
    cannot leave a mangled half-replacement behind.
    """
    forms = {
        str(base),
        str(base.resolve()),
        os.path.relpath(str(base)),
        os.path.relpath(str(base.resolve())),
    }
    return sorted(forms, key=len, reverse=True)


def normalize(text: str, base: Path) -> str:
    for form in srcdir_spellings(base):
        text = text.replace(form, SOURCE_TOKEN)
    for form in srcdir_spellings(base):
        assert form not in text, f"srcdir path leaked into captured text:\n{text}"
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


def dump_py(env) -> tuple:
    """`domaindata['py']` as record lists, in REGISTRATION order.

    The dict insertion order IS oracle data: `PythonDomain.objects` /
    `.modules` iterate in registration order and the fuzzy resolution pass
    (`find_obj` searchmode 1) takes the first match, so these lists must not
    be sorted. Field names follow the `ObjectEntry` / `ModuleEntry`
    NamedTuples (`sphinx/domains/python/__init__.py`).
    """
    data = env.domaindata.get("py", {})
    py_objects = [
        {
            "name": name,
            "docname": entry.docname,
            "node_id": entry.node_id,
            "objtype": entry.objtype,
            "aliased": entry.aliased,
        }
        for name, entry in data.get("objects", {}).items()
    ]
    py_modules = [
        {
            "name": name,
            "docname": entry.docname,
            "node_id": entry.node_id,
            "synopsis": entry.synopsis,
            "platform": entry.platform,
            "deprecated": entry.deprecated,
        }
        for name, entry in data.get("modules", {}).items()
    ]
    return py_objects, py_modules


def dump_py_modindex(env) -> dict:
    """`PythonModuleIndex(py_domain).generate()` -> `(content, collapse)`,
    the exact tuples the py-modindex page is rendered from. Entries are the
    7-field `IndexEntry` NamedTuple (`sphinx/domains/_index.py`)."""
    from sphinx.domains.python import PythonModuleIndex

    content, collapse = PythonModuleIndex(env.get_domain("py")).generate()
    return {
        "collapse": collapse,
        "groups": [
            {
                "letter": letter,
                "entries": [
                    {
                        "name": entry.name,
                        "subtype": entry.subtype,
                        "docname": entry.docname,
                        "anchor": entry.anchor,
                        "extra": str(entry.extra),
                        "qualifier": str(entry.qualifier),
                        "descr": str(entry.descr),
                    }
                    for entry in entries
                ],
            }
            for letter, entries in content
        ],
    }


def dump_dependencies(env, base: Path) -> dict:
    """`env.dependencies` -- absolute `_StrPath`s under the srcdir,
    normalized to `<project>/...` and sorted. Only documents that actually
    have dependencies get an entry (Sphinx's defaultdict never holds an
    empty set after `clear_doc`)."""
    return {
        docname: sorted(normalize(str(dep), base) for dep in deps)
        for docname, deps in sorted(env.dependencies.items())
        if deps
    }


def dump_included(env) -> dict:
    """`env.included` -- docname -> the docnames it textually includes
    (`note_included`), values sorted for a deterministic fixture."""
    return {
        docname: sorted(str(doc) for doc in docs)
        for docname, docs in sorted(env.included.items())
        if docs
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


class RecordingStream(io.StringIO):
    """The warning stream, remembering each `write` as one record.

    Sphinx's warning handler writes one formatted record per call
    (`logging.StreamHandler.emit`: `stream.write(msg + terminator)`, through
    `SafeEncodingWriter.write`), so the writes ARE the records — multi-line
    ones (a reporter message carrying its literal block) included, with
    their blank lines, which the line-split `warnings` key cannot show.
    """

    def __init__(self):
        super().__init__()
        self.records = []

    def write(self, s):
        self.records.append(s)
        return super().write(s)


def build_project(entry: dict) -> dict:
    base = Path(tempfile.mkdtemp(prefix="env_oracle_srcdir_")).resolve() / "src"
    base.mkdir(parents=True)
    (base / "conf.py").write_text(CONF_PY, encoding="utf-8")
    write_project_files(base, entry["files"], entry.get("data_files", {}))

    confoverrides = {**BASE_CONFOVERRIDES, **entry.get("conf", {})}

    resolved_raw: dict = {}

    def capture_write_doc(docname, doctree):
        resolved_raw[docname] = doctree.pformat()

    warning_stream = RecordingStream()
    with docutils_namespace(), patch_docutils(str(base)):
        app = SphinxTestApp(
            buildername="dummy",
            srcdir=base,
            status=io.StringIO(),
            warning=warning_stream,
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

            # One entry per record, exactly as printed minus the handler's
            # line terminator. The line-split `warnings` above must be
            # these records split the same way, or a write was not a record.
            warning_records = []
            for record in warning_stream.records:
                assert record.endswith("\n"), f"unterminated record: {record!r}"
                warning_records.append(normalize(record[:-1], base))
            assert warnings == [
                line
                for record in warning_records
                for line in record.splitlines()
                if line.strip()
            ], "warning_records do not split into warnings"

            tocs_pformat = {
                docname: normalize(toc.pformat(), base)
                for docname, toc in env.tocs.items()
            }
            resolved_pformat = {
                docname: normalize(text, base)
                for docname, text in resolved_raw.items()
            }
            py_objects, py_modules = dump_py(env)

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
                "py_objects": py_objects,
                "py_modules": py_modules,
                "py_modindex": dump_py_modindex(env),
                "dependencies": dump_dependencies(env, base),
                "included": dump_included(env),
                "resolved_pformat": resolved_pformat,
                "warnings": warnings,
                "warning_records": warning_records,
            }
        finally:
            app.cleanup()
            shutil.rmtree(base.parent, ignore_errors=True)

    out = {
        "name": entry["name"],
        "conf": confoverrides,
        "files": entry["files"],
        "expect": expect,
    }
    if entry.get("data_files"):
        out["data_files"] = entry["data_files"]
    return out


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
    assert len(PROJECTS) >= 20, f"corpus degenerated: {len(PROJECTS)} projects"

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
