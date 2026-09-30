#!/usr/bin/env python3
"""Generate tests/fixtures/html_differential_<family>.json from Sphinx 9.1.0.

Regenerate with:

    PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' \
        --with 'docutils==0.22.4' --with 'pygments==2.21.0' \
        --with 'jinja2==3.1.6' --with 'markupsafe==3.0.3' \
        --with 'alabaster==1.0.0' --with 'imagesize==2.0.1' \
        --with 'snowballstemmer==3.1.1' --with 'babel==2.18.0' \
        --with 'sphinxcontrib-htmlhelp==2.1.0' \
        --with 'sphinxcontrib-qthelp==2.0.0' \
        python tools/gen_html_fixture.py

PYTHONNOUSERSITE=1 is NOT optional: `uv run` keeps the user's site-packages
on sys.path, and a user-site Pygments/Jinja2/Pillow there silently changes
page bytes (tokenized code, template whitespace, image sizes). Every package
that reaches a page, a static file or `.buildinfo` is pinned above and
asserted at runtime (`PINS`); Pillow must be ABSENT (docutils' `:scale:`
path and Sphinx's image helpers change behaviour when it is importable).
The two sphinxcontrib pins are there because htmlhelp/qthelp register
`rebuild='html'` config values that feed the `.buildinfo` config hash.

THE PAGE-LEVEL HTML ORACLE (M2 wave 5, design decision 7). Where
tools/gen_env_fixture.py records `BuildEnvironment` state from a `dummy`
build, this generator runs a REAL `-b html` / `-b dirhtml` build of each
corpus project and records the OUTPUT TREE a user would deploy:

  output_files   every file under the outdir: relpath -> {kind, sha256,
                 text}. `kind` is one of page / source / static / image /
                 download / extra / buildinfo / inventory / searchindex.
  pages          every file written through `handle_page` (documents,
                 genindex*, py-modindex, search, html_additional_pages,
                 `_static/opensearch.xml`), full bytes after the
                 normalizations below, as a LIST OF LINES
                 (`text.split('\\n')`, so `'\\n'.join(lines)` restores the
                 exact bytes -- pages never end in a newline; the list form
                 keeps fixture diffs reviewable line by line).
  page_context   per page, what `handle_page` was handed (addctx: title,
                 body, toc, display_toc, prev, next, parents, rellinks,
                 sourcename, meta, metatags, page_source_suffix,
                 has_maths_elements for documents; every plain-data key for
                 genindex/domainindex/search pages) plus a subset of the
                 merged `html-page-context` ctx (content_root, sidebars,
                 css/script files with attributes and priorities, ...). Pure
                 localization data: when a page diverges, the consumer can
                 say whether the body, the toc or the chrome moved.
  texts          (file level, deduplicated by sha256) the full text of every
                 TEMPLATED or GENERATED static file (`*_t` / `*.jinja`
                 renders such as basic.css, documentation_options.js,
                 language_data.js, alabaster.css, user `_t` files; and
                 pygments.css), as lines.
  sources        `_sources/*` text (byte copies of the inputs, possibly
                 under a different suffix).
  buildinfo      `.buildinfo` verbatim, plus `buildinfo_config`: for every
                 option whose value differs from the file's
                 `buildinfo_reference` table, the Python `repr()` and the
                 `stable_hash()` of the value AS HASHED (captured inside
                 `create_build_info`, before `init_css_files`/`init_js_files`
                 inject `priority`), so a mismatching config hash localizes
                 to one option. `buildinfo_modelable` is False when any
                 hashed leaf is not a str/int/bool/None (the crate models
                 Python `str()` only for those) -- such projects' hashes
                 are recorded but are not a reasonable byte target.
  inventory      objects.inv as its 4 header lines + the zlib-DECOMPRESSED
                 payload text (compressed bytes differ across zlib
                 implementations; decision 8).
  warnings       the warning stream as RECORDS (one per logging handler
                 write, trailing newline dropped) -- never line-split, so a
                 multi-line reporter record stays one record.

searchindex.js is M3: it is listed in output_files (sha only) and the
consumer never compares its bytes.

HARNESS. Each project is built with `sphinx.application.Sphinx` itself --
the class `sphinx-build` instantiates (`sphinx/cmd/build.py:build_main`),
inside the same `patch_docutils(confdir)` + `docutils_namespace()` pair --
NOT `SphinxTestApp`: `SphinxTestApp._init_builder`
(`sphinx/testing/util.py:198-206`) silently swaps the default `alabaster`
theme for `basic` unless `html_theme` is an override, which would make the
alabaster family (html_theme UNSET) record the wrong theme. The global state
`SphinxTestApp.cleanup` would reset is reset by hand before every build
(`sphinx.locale.translators` -- a German build otherwise leaks its
translator into every later English one --, the ModuleAnalyzer cache, and
the per-process `_file_checksum_inner` cache). Layout mirrors the Rust
consumer: `<tmp>/source` (srcdir == confdir, conf.py inside),
`<tmp>/build` (outdir; the doctree cache lives OUTSIDE it, at
`<tmp>/doctrees`, so the recorded tree has no `.doctrees/`), serial
(parallel=0), fresh env, `sphinx.util.console.nocolor()`.

Each project's configuration has two halves, both recorded:
  conf      confoverrides == `sphinx-build -D` values: BASE_CONFOVERRIDES
            (html_theme='basic', smartquotes=False,
            highlight_language='none', language='en') merged with the
            project's own scalars; a project may `unset` base keys (the
            alabaster family drops html_theme, the highlight family drops
            highlight_language). Only str/bool/int values and comma-free
            str lists -- what -D can carry.
  conf_py   the full conf.py written into the srcdir: CONF_PY plus the
            project's `conf_py` extra, for everything -D cannot express
            (dicts, tuples, css/js attribute tuples, html_sidebars, ...).
Per-project `env` (only SOURCE_DATE_EPOCH, see below) is set around the
build and restored.

DATES. SOURCE_DATE_EPOCH must be UNSET in the generator's environment
(asserted). Nothing date-dependent is recorded except in projects that set
it explicitly through their `env` field: `html_last_updated_fmt` and
`|today|` read it (`sphinx/util/i18n.py:271-280`, which also forces UTC),
and `correct_copyright_year` (`sphinx/config.py:711-739`) would rewrite a
copyright year equal to the current one -- so no copyright here ever names
a recent year. A project setting html_last_updated_fmt or using |today|
without a pinned `today`/SOURCE_DATE_EPOCH fails generation.

NORMALIZATIONS (the ONLY rewrites; each is applied to every recorded string
and asserted complete):
  1. `<project>` -- every spelling of the srcdir (absolute, resolved, and
     the cwd-relative forms docutils' adapt_path uses for included content,
     same rule as tools/gen_env_fixture.py). Appears in keep_warnings pages
     (system-message source paths) and warnings.
  2. `<outdir>` -- every spelling of the outdir; the tmp root itself must
     not survive either (the doctree dir is under it).
  3. `@@generator-credit@@` -- the footer's generator credit. By design
     (decision 5) sphinx-ultra does not claim "Created using Sphinx" on
     pages it built; the oracle replaces the exact credit text Sphinx
     renders -- basic's `{% trans %}Created using <a href=...>Sphinx</a>
     9.1.0.{% endtrans %}` rendered through the build's own template
     environment (so a translated credit is replaced too), and alabaster's
     literal `Powered by <a href=...>Sphinx 9.1.0</a>` -- with this token.
     Generation asserts each page carries exactly one credit when the
     page's `show_sphinx` is true and none otherwise. The consumer maps the
     crate's honest credit onto the same token. The `sphinx_version`
     context value itself is NOT normalized (it stays "9.1.0" on both
     sides; themes feature-gate on it).
Nothing else is volatile: two complete passes over the corpus (fresh tmp
dirs, fresh apps) must produce byte-identical JSON or nothing is written.
The `?v=` asset checksums (CRC32 of the output static file) are
deterministic and recorded as-is.

CORPUS POLICY. Families (one fixture file each):
  structural   every tools/gen_env_fixture.py project (imported, so that
               corpus's extensions flow in on regeneration) under -b html,
               minus EXCLUDED_ENV_PROJECTS (each with its reason)
  nodes        node-kind coverage pages (every construct in the translator
               research notes)
  toctree      toctree/navigation: captions, hidden, numbered, titlesonly,
               nesting, prev/next, parents, subdirectories
  indices      genindex (incl. split) and py-modindex (incl.
               modindex_common_prefix), index switches
  config       one html_* knob group per project
  dirhtml      the dirhtml builder
  highlight    highlight_language default + python/pycon/text blocks
  smartquotes  smartquotes=True in several languages
  alabaster    html_theme unset -> Sphinx's default theme
One axis per project. Never remove or rename a project; later tasks only
extend. Data files never use the .rst/.md/.txt suffixes (sphinx-ultra's
discovery is wider than Sphinx's default source_suffix; a .txt member would
become a document on one side only). Binary members (real PNGs, built here
with stored -- level 0 -- deflate so the bytes do not depend on the zlib
build) travel base64-encoded in `binary_files`.
"""

import base64
import copy
import hashlib
import importlib.metadata
import importlib.util
import io
import json
import os
import re
import shutil
import struct
import sys
import tempfile
import zlib
from pathlib import Path

PINS = {
    "sphinx": "9.1.0",
    "docutils": "0.22.4",
    "pygments": "2.21.0",
    "jinja2": "3.1.6",
    "markupsafe": "3.0.3",
    "alabaster": "1.0.0",
    "imagesize": "2.0.1",
    "snowballstemmer": "3.1.1",
    "babel": "2.18.0",
    "sphinxcontrib-htmlhelp": "2.1.0",
    "sphinxcontrib-qthelp": "2.0.0",
}

for _dist, _want in PINS.items():
    _got = importlib.metadata.version(_dist)
    assert _got == _want, (
        f"{_dist} {_got} != {_want}; regenerate with the pinned command in the "
        "module docstring"
    )
assert importlib.util.find_spec("PIL") is None, (
    "Pillow is importable: the HTML oracle must be generated WITHOUT it "
    "(image sizing and docutils' :scale: messages depend on it)"
)
assert "SOURCE_DATE_EPOCH" not in os.environ, (
    "SOURCE_DATE_EPOCH is set: unset it (projects that need a fixed date set "
    "it themselves through their `env` field)"
)
assert "DOCUTILSCONFIG" not in os.environ, "DOCUTILSCONFIG is set: unset it"
for _conf in (Path("docutils.conf"), Path.home() / ".docutils", Path("/etc/docutils.conf")):
    assert not _conf.exists(), (
        f"{_conf} exists: docutils reads it (read_config_files=True) and it would "
        "change page bytes"
    )

import docutils  # noqa: E402
import sphinx  # noqa: E402

assert sphinx.__version__ == PINS["sphinx"]
assert docutils.__version__ == PINS["docutils"]

from sphinx.util.console import nocolor  # noqa: E402

nocolor()  # warning text must not carry environment-dependent ANSI escapes

import sphinx.locale  # noqa: E402
import sphinx.pycode  # noqa: E402
from sphinx.application import Sphinx  # noqa: E402
from sphinx.builders.html import StandaloneHTMLBuilder  # noqa: E402
from sphinx.builders.html import _assets as html_assets  # noqa: E402
from sphinx.util._serialise import stable_hash  # noqa: E402
from sphinx.util.docutils import docutils_namespace, patch_docutils  # noqa: E402

TOOLS_DIR = Path(__file__).resolve().parent
FIXTURE_DIR = TOOLS_DIR.parent / "tests" / "fixtures"

sys.path.insert(0, str(TOOLS_DIR))
import gen_env_fixture  # noqa: E402  (the structural family's corpus)

SCHEMA_VERSION = 1
GENERATOR = "tools/gen_html_fixture.py"

PROJECT_TOKEN = "<project>"
OUTDIR_TOKEN = "<outdir>"
CREDIT_TOKEN = "@@generator-credit@@"

BASE_CONFOVERRIDES = {
    "html_theme": "basic",
    "smartquotes": False,
    "highlight_language": "none",
    "language": "en",
}

CONF_PY = (
    "project = 'fixture'\n"
    "extensions = []\n"
    "master_doc = 'index'\n"
    "exclude_patterns = ['_build']\n"
)

# basic/layout.html:204 -- rendered through each build's own template
# environment, so a translated build's credit is found too.
BASIC_CREDIT_TEMPLATE = (
    '{% trans sphinx_version=sphinx_version|e %}Created using '
    '<a href="https://www.sphinx-doc.org/">Sphinx</a> {{ sphinx_version }}.'
    "{% endtrans %}"
)
# Builtin themes whose layout empties `{% block footer %}`: their pages carry
# no credit at all (themes/epub/layout.html:13, themes/nonav/layout.html:14).
CREDITLESS_THEMES = {"epub", "nonav"}
# alabaster/layout.html:93 -- literal text, never translated.
ALABASTER_CREDIT = (
    'Powered by <a href="https://www.sphinx-doc.org/">Sphinx '
    f"{sphinx.__version__}</a>"
)

EXCLUDED_ENV_PROJECTS = {
    "toctree_circular": (
        "a genuine two-document toctree cycle: the html builder's "
        "prepare_writing -> env.collect_relations() -> _traverse_toctree "
        "recurses without bound (RecursionError) -- a real sphinx-build -b "
        "html of this project crashes, so there is no output tree to record "
        "(the env fixture records relations=null for the same reason)"
    ),
    "numfig_on": (
        "its numfig_format sets table to 'Table {number}', a spelling only "
        ":numref: text understands: the HTML writer's caption prefix is "
        "`prefix % '.'.join(numbers)` (sphinx/writers/html5.py:450), which "
        "raises TypeError('not all arguments converted during string "
        "formatting') -- a real sphinx-build -b html of this project crashes. "
        "The nodes family's `nodes_numfig` covers numbered captions with "
        "%s-style formats instead"
    ),
}

# Per-document addctx keys (StandaloneHTMLBuilder.get_doc_context +
# write_doc's has_maths_elements).
DOC_KEYS = [
    "title",
    "body",
    "toc",
    "display_toc",
    "prev",
    "next",
    "parents",
    "rellinks",
    "sourcename",
    "meta",
    "metatags",
    "page_source_suffix",
    "has_maths_elements",
]

# Merged-context keys worth localizing on (html-page-context).
CTX_KEYS = [
    "content_root",
    "pageurl",
    "sidebars",
    "docstitle",
    "shorttitle",
    "last_updated",
    "show_source",
    "has_source",
    "show_sphinx",
    "show_copyright",
    "copyright",
    "logo_url",
    "favicon_url",
    "use_opensearch",
    "file_suffix",
    "link_suffix",
    "language",
    "builder",
]


# ---------------------------------------------------------------------------
# Binary members: tiny real PNGs, stored (level 0) deflate so the bytes are
# independent of the zlib build.
# ---------------------------------------------------------------------------


def png(width: int, height: int, rgb: tuple) -> bytes:
    raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 0))
        + chunk(b"IEND", b"")
    )


PNG_RED_40x30 = png(40, 30, (200, 30, 30))
PNG_BLUE_16x16 = png(16, 16, (30, 30, 200))
PNG_GREEN_8x4 = png(8, 4, (30, 160, 30))

SVG_DIAGRAM = (
    '<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10" '
    'viewBox="0 0 20 10"><rect width="20" height="10" fill="#888"/></svg>\n'
)

EXAMPLE_PY = gen_env_fixture.EXAMPLE_PY

# ---------------------------------------------------------------------------
# Corpus. Each project: name, files {docname: rst}, and optionally
#   conf        {key: scalar}  extra -D overrides over BASE_CONFOVERRIDES
#   unset       [key]          BASE_CONFOVERRIDES keys to drop
#   conf_py     str            appended to CONF_PY
#   env         {var: value}   process environment around the build
#   builder     'html' (default) | 'dirhtml'
#   data_files  {relpath: text}
#   binary_files {relpath: bytes}
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# (b) nodes: every construct the translator research notes specify. Single
# documents unless a construct needs a second one (citations, :doc:, math on
# one page only). highlight_language stays 'none' (base), so `::` blocks are
# plain; explicit `code-block:: python` still tokenizes (Pygments 2.21.0).
# ---------------------------------------------------------------------------

NODES = [
    {
        "name": "nodes_inline",
        "files": {
            "index": """\
Inline markup
=============

Plain *emphasis*, **strong**, ``literal text``, `default role`,
:emphasis:`role emphasis`, :strong:`role strong`, :literal:`role literal`,
H\\ :sub:`2`\\ O and E = mc\\ :sup:`2`, :title-reference:`A Title`.

Sphinx roles: :abbr:`LIFO (last-in, first-out)`, :abbr:`CSS`,
:kbd:`Control-x Control-f`, :kbd:`Ctrl+Alt+Del`, :kbd:`-`,
:menuselection:`Start --> Programs --> App`, :guilabel:`&Cancel`,
:file:`/usr/lib/{name}.so`, :samp:`print({x})`, :command:`rm`,
:program:`sphinx-build`, :dfn:`definition`, :mailheader:`Content-Type`,
:makevar:`PATH`, :mimetype:`text/plain`, :newsgroup:`comp.lang.python`,
:regexp:`^a+$`, :manpage:`ls(1)`, :code:`x = 1 + 2`.

PEPs and RFCs: :pep:`8`, :pep:`8#id4`, :pep:`Style <8>`, :rfc:`2822`,
:rfc:`2822#section-3.4`.

Literal spacing: ``two  spaces`` and ``three   spaces`` and ``tab\there``.

Escapes: \\*not emphasis\\*, a backslash \\\\ and joined\\ text.

Characters that need escaping: & < > " ' @ and non-ASCII ©, —, …, é, 日本.

| Line block one
| Line block two
|     indented continuation
""",
        },
    },
    {
        "name": "nodes_lists",
        "files": {
            "index": """\
Lists
=====

Simple bullets:

* one
* two
* three

Non-simple bullets (two paragraphs in one item):

- first item

  second paragraph of the first item

- second item

Nested:

* outer

  * inner a
  * inner b

    1. deep one
    2. deep two

* outer again

Enumerations:

1. arabic
2. arabic

a. loweralpha
b. loweralpha

A. upperalpha
B. upperalpha

i. lowerroman
ii. lowerroman

I. upperroman
II. upperroman

(1) parens
(2) parens

1) rparen
2) rparen

3. starts at three
4. four

#. auto one
#. auto two

Definition list:

term
   Definition.

term with classifier : classifier one : classifier two
   Definition with classifiers.

*marked up* term
   First paragraph.

   Second paragraph.

Field list (after a paragraph, so not docinfo):

:Author: Somebody
:Version: 1.0
:Long field name: with a body
   that spans lines.

Option list:

-a            short option
-b FILE       short with argument
--long        long option
--input=FILE  long with argument
/V            DOS style

A list made non-simple by an index directive:

* item with an index entry

  .. index:: single: in-list entry

* plain item
""",
        },
    },
    {
        # Note `.. class:: two-classes another` below: under Sphinx the
        # default domain's `class` directive (py:class) shadows docutils'
        # `class`, so it renders a py:class description, not classes on the
        # next paragraph -- a real trap, kept deliberately (`rst-class` is
        # the docutils behaviour, exercised just above it).
        "name": "nodes_blocks",
        "files": {
            "index": """\
Block constructs
================

Paragraph before a literal block::

   literal line 1
     literal line 2 <&>

Expanded form:

::

   bare literal block

Quoted literal block:

::

> quoted line one
> quoted line two

.. parsed-literal::

   parsed **bold** and *emph* in a literal

.. parsed-literal::

   parsed literal without markup

>>> print("doctest block")
doctest block

Block quote:

   Quoted paragraph.

   -- Attribution Name

Nested quote:

   Level one.

      Level two.

----------

After the transition.

.. comment that renders nothing

.. epigraph::

   Epigraph text.

   -- Author

.. highlights::

   Highlighted text.

.. pull-quote::

   Pull quote text.

.. compound::

   Compound first paragraph.

   Compound second paragraph.

.. container:: custom-box other-class

   Inside a container.

.. topic:: Topic Title

   Topic body.

.. sidebar:: Sidebar Title
   :subtitle: Sidebar Subtitle

   Sidebar body.

.. rubric:: A Rubric

.. centered:: Centered text

.. hlist::
   :columns: 3

   * alpha
   * beta
   * gamma
   * delta
   * epsilon

.. rst-class:: special-paragraph

A paragraph with a class.

.. class:: two-classes another

Another paragraph with classes.

.. acks::

   * Alice
   * Bob
""",
        },
    },
    {
        "name": "nodes_tables",
        "files": {
            "index": """\
Tables
======

+------------+------------+-----------+
| Header 1   | Header 2   | Header 3  |
+============+============+===========+
| body row 1 | column 2   | column 3  |
+------------+------------+-----------+
| row 2      | Cells may span columns.|
+------------+------------+-----------+
| row 3      | Cells may  | - Cells   |
+------------+ span rows. | - contain |
| row 4      |            | - blocks. |
+------------+------------+-----------+

=====  =====  ======
  A      B    A or B
=====  =====  ======
False  False  False
True   False  True
False  True   True
True   True   True
=====  =====  ======

=====  =====
col 1  col 2
=====  =====
1      Second column of row 1.
2      Second column of row 2.
       Second line of paragraph.
3      - Second column of row 3.

       - Second item in bullet
         list (row 3, column 2).
\\      Row 4; column 1 will be empty.
=====  =====

.. table:: Captioned table
   :widths: 30 70
   :align: right
   :width: 80%
   :class: extra-table

   =====  =====
   key    value
   =====  =====
   a      1
   b      2
   =====  =====

.. table::
   :align: center
   :widths: auto

   ===  ===
   x    y
   ===  ===

.. list-table:: List table
   :header-rows: 1
   :stub-columns: 1
   :widths: 20 40 40
   :name: list-table-name

   * - Stub
     - Column A
     - Column B
   * - Row 1
     - a1
     - b1
   * - Row 2
     - a2
     -

.. csv-table:: CSV table
   :header: "Name", "Value", "Note"
   :widths: 15, 10, 30

   "alpha", 1, "first, with comma"
   "beta", 2, "second"
""",
        },
    },
    {
        "name": "nodes_admonitions",
        "files": {
            "index": """\
Admonitions
===========

.. attention:: Attention text.

.. caution:: Caution text.

.. danger:: Danger text.

   * with a list
   * of items

.. error:: Error text.

.. hint:: Hint text.

.. important:: Important text.

.. note:: Note text.

.. tip:: Tip text.

.. warning:: Warning text.

.. admonition:: Custom Title
   :class: custom-class

   Generic admonition body.

.. admonition:: Named Admonition
   :name: named-admonition

   Body.

.. note::
   :collapsible:

   Collapsed note.

.. warning::
   :collapsible: open

   Open collapsible warning.

.. seealso:: Short form see-also.

.. seealso::

   Long form see-also.

   Second paragraph.

.. versionadded:: 1.2

.. versionadded:: 1.3
   With an explanation.

.. versionchanged:: 2.0
   Changed behaviour.

   Second paragraph of the change.

.. deprecated:: 3.0
   Use something else.

.. versionremoved:: 4.0
   Removed entirely.
""",
        },
    },
    {
        "name": "nodes_footnotes",
        "files": {
            "index": """\
Footnotes and citations
=======================

.. toctree::

   other

Numbered [1]_, auto [#]_, labeled auto [#note]_, symbols [*]_ and [*]_,
the labeled one again [#note]_, and a citation [CIT2002]_ twice [CIT2002]_.

.. [1] Numbered footnote.
.. [#] Auto-numbered footnote.
.. [#note] Labeled auto-numbered footnote.
.. [*] First symbol footnote.
.. [*] Second symbol footnote.

.. [CIT2002] A citation.
""",
            "other": """\
Other
=====

The citation from another document: [CIT2002]_.
""",
        },
    },
    {
        "name": "nodes_links",
        "files": {
            "index": """\
Links
=====

.. toctree::

   other
   sub/page

External: `Python <https://www.python.org/>`_, anonymous `link`__,
embedded alias `Alias <named_>`_, named_, standalone https://example.org/path,
email user@example.org, mailto:other@example.org, an at-sign URL
https://example.com/@user.

__ https://docs.python.org/

.. _named: https://example.com/named

Internal: `Local Target`_, indirect `alias target`_, `Other Section`_.

.. _alias target: `Local Target`_

.. _Local Target:

Local Target Paragraph.

Other Section
-------------

Cross references: :ref:`label-here`, :ref:`Explicit <label-here>`,
:ref:`label-other`, :doc:`other`, :doc:`Custom text <other>`,
:doc:`/sub/page`, :any:`label-other`, :download:`the file <data.bin>`,
:download:`nested/data.bin`.

Dangling: :ref:`no-such-label`, :doc:`no-such-doc`.

.. _label-here:

Labeled Section
---------------

Text.
""",
            "other": """\
.. _label-other:

Other Page
==========

Back to :doc:`index` and :ref:`label-here`, relative :doc:`sub/page`.
""",
            "sub/page": """\
Sub Page
========

Up: :doc:`../other`, absolute :doc:`/index`, :ref:`label-other`,
:download:`../data.bin`.
""",
        },
        "data_files": {
            "data.bin": "payload\n",
            "nested/data.bin": "nested payload\n",
        },
    },
    {
        "name": "nodes_desc_py",
        "files": {
            "index": """\
Python objects
==============

.. py:module:: pkg.mod
   :synopsis: A module.

.. py:function:: func(a, b: int = 2, *args, c: str | None = None, **kwargs) -> bool

   A function.

   :param a: first
   :type a: str
   :param int b: second
   :returns: a flag
   :rtype: bool
   :raises ValueError: when bad

.. py:function:: posonly(a, /, b, *, c)
   :async:

   Positional-only and keyword-only parameters.

.. py:class:: Base

.. py:class:: Klass(Base, metaclass=Meta)

   A class.

   .. py:method:: method(self, x)

      A method.

   .. py:method:: smethod(x)
      :staticmethod:

   .. py:method:: cmethod(cls)
      :classmethod:

   .. py:method:: amethod()
      :abstractmethod:
      :async:

   .. py:attribute:: attr
      :type: int
      :value: 3

   .. py:property:: prop
      :type: str

.. py:exception:: CustomError

.. py:data:: CONSTANT
   :type: int
   :value: 42

.. py:decorator:: deco(arg)

.. py:function:: hidden()
   :no-index:

.. py:function:: noentry()
   :no-contents-entry:

.. py:currentmodule:: None

.. py:function:: toplevel()

   :var x: a variable
   :ivar y: an instance variable
   :cvar z: a class variable

References: :py:func:`pkg.mod.func`, :py:func:`~pkg.mod.func`,
:py:class:`pkg.mod.Klass`, :py:meth:`pkg.mod.Klass.method`,
:py:attr:`pkg.mod.Klass.attr`, :py:exc:`pkg.mod.CustomError`,
:py:data:`pkg.mod.CONSTANT`, :py:mod:`pkg.mod`, :py:obj:`toplevel`,
unresolved :py:func:`nowhere`.
""",
        },
    },
    {
        "name": "nodes_desc_std",
        "files": {
            "index": """\
Standard domain objects
=======================

.. program:: tool

.. option:: -f, --flag

   A flag.

.. option:: --output=FILE

   An option with a value.

.. option:: /V

   DOS style.

.. envvar:: TOOL_HOME

   An environment variable.

.. confval:: tool_setting
   :type: int
   :default: ``3``

   A configuration value.

.. describe:: opcode

   A generic description.

.. object:: thing

   A generic object.

.. productionlist::
   try_stmt: try1_stmt | try2_stmt
   try1_stmt: "try" ":" `suite`
            : ("except" [`expression` ["," `target`]] ":" `suite`)+

References: :option:`tool -f`, :option:`tool --output`, :envvar:`TOOL_HOME`,
:confval:`tool_setting`, :token:`try_stmt`.
""",
        },
    },
    {
        "name": "nodes_desc_other_domains",
        "files": {
            "index": """\
Other domains
=============

.. rst:directive:: .. mydirective:: arg

   A directive.

   .. rst:directive:option:: opt
      :type: text

.. rst:role:: myrole

.. js:module:: jsmod

.. js:function:: jsfunc(a, b)

   A JavaScript function.

.. js:class:: JsClass(x)

   .. js:method:: jsMethod()

   .. js:attribute:: jsAttr

.. js:data:: JS_DATA

.. c:function:: int c_func(const char *s, size_t n)

.. c:struct:: c_struct

   .. c:member:: int field

.. c:macro:: C_MACRO(x)

.. cpp:class:: template<typename T> Container

   .. cpp:function:: T get(int index) const

.. cpp:enum:: Colour

   .. cpp:enumerator:: Red

References: :rst:dir:`mydirective`, :rst:role:`myrole`, :js:func:`jsfunc`,
:c:func:`c_func`, :cpp:class:`Container`.
""",
        },
    },
    {
        "name": "nodes_images",
        "files": {
            "index": """\
Images
======

.. toctree::

   sub/page

.. image:: img/red.png

.. image:: img/red.png
   :alt: Red rectangle
   :width: 80px
   :height: 60px
   :align: center
   :class: framed

.. image:: img/red.png
   :scale: 50%
   :align: left

.. image:: img/blue.png
   :width: 50%
   :target: https://example.org/

.. image:: img/blue.png
   :width: 2em
   :target: `Figures`_

.. image:: img/red.png
   :scale: 200%
   :class: no-scaled-link

.. image:: other/red.png
   :name: same-basename

.. image:: img/diagram.*

.. image:: img/diagram.svg
   :alt: an svg

.. image:: https://example.org/remote.png
   :alt: remote image

Figures
-------

.. figure:: img/green.png
   :figwidth: 60%
   :align: right
   :figclass: fig-extra
   :alt: green

   The figure caption with *markup*.

   The legend paragraph.

.. figure:: img/blue.png
   :name: named-figure

   Named figure caption.

Inline |icon| substitution image.

.. |icon| image:: img/green.png
""",
            "sub/page": """\
Sub Page
========

.. image:: ../img/red.png

.. figure:: /img/blue.png

   Figure from a subdirectory.
""",
        },
        "data_files": {
            "img/diagram.svg": SVG_DIAGRAM,
        },
        "binary_files": {
            "img/red.png": PNG_RED_40x30,
            "img/blue.png": PNG_BLUE_16x16,
            "img/green.png": PNG_GREEN_8x4,
            "img/diagram.png": PNG_GREEN_8x4,
            "other/red.png": PNG_BLUE_16x16,
        },
    },
    {
        "name": "nodes_math",
        "files": {
            "index": """\
Math
====

.. toctree::

   nomath

Inline :math:`a^2 + b^2 = c^2` and :math:`\\alpha < \\beta`.

.. math:: e^{i\\pi} + 1 = 0
   :label: euler

.. math::

   a &= b \\\\
   c &= d

.. math::

   x = 1

   y = 2

.. math::
   :nowrap:

   \\begin{equation}
   z = 3
   \\end{equation}

.. math::
   :label: second
   :no-wrap:

   w = 4

See :eq:`euler` and :math:numref:`second`.
""",
            "nomath": """\
No math
=======

This page has no math, so no MathJax script.
""",
        },
    },
    {
        "name": "nodes_numfig",
        "conf": {"numfig": True},
        "conf_py": (
            "numfig_format = {'figure': 'Figure %s', 'table': 'Tab. %s', "
            "'code-block': 'Snippet %s', 'section': 'Sect. %s'}\n"
        ),
        "files": {
            "index": """\
Numbered figures
================

.. toctree::
   :numbered:

   chapter

.. figure:: img/red.png
   :name: fig-one

   First figure.

.. table:: First table
   :name: table-one

   ===  ===
   a    b
   ===  ===

.. code-block:: none
   :caption: First snippet
   :name: code-one

   code

References: :numref:`fig-one`, :numref:`table-one`, :numref:`code-one`,
:numref:`Custom {number} <fig-one>`, :numref:`Named {name} <fig-one>`,
:numref:`chapter-sec`, :numref:`fig-two`.
""",
            "chapter": """\
.. _chapter-sec:

Chapter
=======

.. figure:: img/red.png
   :name: fig-two

   Second figure.

Sub Section
-----------

.. figure:: img/red.png

   Unnamed figure, still numbered.
""",
        },
        "binary_files": {"img/red.png": PNG_RED_40x30},
    },
    {
        "name": "nodes_sections",
        "files": {
            "index": """\
Sections *with* ``markup``
==========================

Level 2
-------

Level 3
~~~~~~~

Level 4
^^^^^^^

Level 5
'''''''

Level 6
#######

Level 7
*******

Level 8
+++++++

Deep text.

Duplicate
---------

Duplicate
---------

Non-ASCII Éléments — 日本語
---------------------------

Text.
""",
        },
    },
    {
        "name": "nodes_contents",
        "files": {
            "index": """\
Contents directive
==================

.. contents:: On this page
   :local:
   :depth: 2
   :backlinks: top

First
-----

Nested
~~~~~~

Second
------

Text.
""",
        },
    },
    {
        "name": "nodes_glossary_index",
        "files": {
            "index": """\
Glossary and index
==================

.. glossary::

   environment
      A structure.

   source directory
   srcdir
      The root directory.

   Zeta
      Last term.

.. glossary::
   :sorted:

   beta term
      Sorted second.

   alpha term
      Sorted first.

Terms: :term:`environment`, :term:`srcdir`, :term:`the root <source directory>`,
:term:`Environment`, dangling :term:`missing term`.

.. index::
   single: single entry
   single: parent; child
   pair: pair; entry
   triple: one; two; three
   see: seeing; single entry
   seealso: see also; pair
   !single: main entry

.. index:: inline-directive-entry
   :name: index-target-name

Inline :index:`role entry` and :index:`with target <role target>`.
""",
        },
    },
    {
        "name": "nodes_substitutions",
        "conf": {"today": "January 1, 2000"},
        "conf_py": "version = '1.2'\nrelease = '1.2.3'\n",
        "files": {
            "index": """\
Substitutions
=============

Replace: |name|, trimmed |trim|, unicode |copy|, image |img|.

Defaults: version |version|, release |release|, today |today|.

.. |name| replace:: *replacement text*
.. |trim| unicode:: U+2014
   :trim:
.. |copy| unicode:: 0xA9 .. copyright sign
.. |img| image:: pic.png
   :alt: pic
""",
        },
        "binary_files": {"pic.png": PNG_GREEN_8x4},
    },
    {
        "name": "nodes_code",
        "files": {
            "index": """\
Code blocks
===========

.. code-block:: python
   :caption: A captioned block
   :name: captioned-code

   def f(x):
       return x + 1

.. code-block:: python
   :linenos:
   :lineno-start: 10
   :emphasize-lines: 2,3

   a = 1
   b = 2
   c = 3

.. code-block:: python
   :dedent: 4
   :class: extra-code

       indented = True

.. code-block:: text
   :force:

   plain <text> & more

.. code:: python

   docutils_code = "directive"

.. sourcecode:: python

   alias = "sourcecode"

.. literalinclude:: example.py
   :language: python
   :pyobject: Foo.method

.. literalinclude:: example.py
   :language: python
   :lines: 1-3
   :caption:

.. literalinclude:: example.py
   :language: python
   :start-after: CONST
   :end-before: class Foo
   :prepend: # prepended
   :append: # appended

.. literalinclude:: example.py
   :language: none
   :lines: 6-8
   :linenos:
   :lineno-match:
""",
        },
        "data_files": {"example.py": EXAMPLE_PY},
    },
    {
        "name": "nodes_raw_only_meta",
        "files": {
            "index": """\
Raw, only and meta
==================

.. meta::
   :description: A page description
   :keywords: sphinx, html

.. title:: Custom HTML Title

.. raw:: html

   <div class="raw-block">raw <b>html</b></div>

.. raw:: latex

   \\textbf{latex only}

.. only:: html

   Only in HTML builds.

.. only:: latex

   Only in LaTeX builds.

.. only:: html and not latex

   Tag expression.

.. tabularcolumns:: |l|r|

.. role:: custom
   :class: custom-role

Custom role :custom:`text`.

.. default-role:: literal

Default role now `literal`.
""",
        },
    },
    {
        "name": "nodes_system_messages",
        "conf": {"keep_warnings": True},
        "files": {
            "index": """\
System messages
===============

Inline *emphasis start without end.

Unknown role :nosuchrole:`text`.

.. nosuchdirective:: arg

Reference to `undefined target`_.

.. note::

.. _dup:

Dup target one.

.. _dup:

Dup target two.
""",
        },
    },
]

# ---------------------------------------------------------------------------
# (c) toctree / navigation: what the resolved toctree, the local toc, the
# relbars, prev/next and parents look like.
# ---------------------------------------------------------------------------


def _leaf(title: str, body: str = "Leaf text.") -> str:
    return f"{title}\n{'=' * len(title)}\n\n{body}\n"


TOCTREE = [
    {
        "name": "toc_captions_maxdepth",
        "files": {
            "index": """\
Home
====

.. toctree::
   :caption: First Part
   :maxdepth: 1
   :name: first-toc

   a
   b

.. toctree::
   :caption: Second *Part*
   :maxdepth: 3

   c
""",
            "a": """\
Page A
======

A One
-----

A One One
~~~~~~~~~

A Two
-----
""",
            "b": _leaf("Page B"),
            "c": """\
Page C
======

.. toctree::

   c1

C Section
---------
""",
            "c1": _leaf("Page C1"),
        },
    },
    {
        "name": "toc_hidden",
        "files": {
            "index": """\
Home
====

.. toctree::

   visible

.. toctree::
   :hidden:

   hidden
""",
            "visible": _leaf("Visible"),
            "hidden": _leaf("Hidden"),
        },
    },
    {
        "name": "toc_includehidden",
        "files": {
            "index": """\
Home
====

.. toctree::
   :includehidden:

   parent
""",
            "parent": """\
Parent
======

.. toctree::
   :hidden:

   child
""",
            "child": _leaf("Child"),
        },
    },
    {
        "name": "toc_numbered_multi",
        "files": {
            "index": """\
Home
====

.. toctree::
   :numbered:

   one
   two

.. toctree::
   :numbered: 1

   three
""",
            "one": """\
One
===

One Sub
-------

One Sub Sub
~~~~~~~~~~~
""",
            "two": """\
Two
===

.. toctree::

   two_child

Two Sub
-------
""",
            "two_child": _leaf("Two Child"),
            "three": """\
Three
=====

Three Sub
---------
""",
        },
    },
    {
        "name": "toc_titlesonly",
        "files": {
            "index": """\
Home
====

.. toctree::
   :titlesonly:

   a
   b
""",
            "a": """\
Page A
======

Hidden By Titlesonly
--------------------
""",
            "b": _leaf("Page B"),
        },
    },
    {
        "name": "toc_subdirs",
        "files": {
            "index": """\
Home
====

.. toctree::
   :maxdepth: 2

   guide/index
   api/index
""",
            "guide/index": """\
Guide
=====

.. toctree::

   install
   advanced/deep
""",
            "guide/install": _leaf("Install", "See :doc:`../api/index` and :doc:`/index`."),
            "guide/advanced/deep": _leaf("Deep Page", "Three levels down."),
            "api/index": """\
API
===

.. toctree::

   ../guide/install
""",
        },
    },
    {
        "name": "toc_glob_reversed",
        "files": {
            "index": """\
Home
====

.. toctree::
   :glob:
   :reversed:

   intro
   chapters/*
""",
            "intro": _leaf("Intro"),
            "chapters/ch1": _leaf("Chapter 1"),
            "chapters/ch2": _leaf("Chapter 2"),
            "chapters/ch3": _leaf("Chapter 3"),
        },
    },
    {
        "name": "toc_special_entries",
        "files": {
            "index": """\
Home
====

.. toctree::

   self
   Custom Title <a>
   Python <https://www.python.org/>
   https://example.org/bare
   a
   missing
""",
            "a": _leaf("Page A"),
        },
    },
    {
        "name": "toc_orphans_and_untitled",
        "files": {
            "index": """\
Home
====

.. toctree::

   untitled
   two_top
""",
            "untitled": "Just a paragraph, no title at all.\n",
            "two_top": """\
First Top
=========

Text.

Second Top
==========

Text.
""",
            "orphan": ":orphan:\n\nOrphan\n======\n\nNot in any toctree, on purpose.\n",
            "stray": _leaf("Stray", "Not in any toctree, by accident."),
        },
    },
    {
        "name": "toc_single_section_pages",
        "files": {
            "index": """\
Home
====

.. toctree::
   :maxdepth: 1

   flat
   deep
""",
            "flat": _leaf("Flat", "No subsections: display_toc is false here."),
            "deep": """\
Deep
====

Sub
---

Sub Sub
~~~~~~~
""",
        },
    },
]

# ---------------------------------------------------------------------------
# (d) genindex / py-modindex.
# ---------------------------------------------------------------------------

_GENINDEX_DOC = """\
Index Entries
=============

.. index::
   single: apple
   single: Apple; pie
   single: apple; tart
   pair: banana; split
   triple: cherry; date; elder
   see: fig; apple
   seealso: grape; banana
   !single: main apple
   single: _private
   single: @decorator
   single: "quoted"
   single: 1number
   single: Ärger
   single: élan
   single: Zebra
   single: zebra

.. py:module:: idxmod

.. py:function:: idxfunc()

.. py:class:: IdxClass

   .. py:method:: meth()

.. envvar:: IDX_VAR

.. program:: idxprog

.. option:: --idx-opt

.. glossary::

   idx term
      A term.

Text with :index:`inline entry`.
"""

INDICES = [
    {
        "name": "idx_genindex",
        "files": {
            "index": _GENINDEX_DOC
            + "\n.. toctree::\n\n   second\n",
            "second": """\
Second
======

.. index::
   single: apple
   !single: banana
   pair: second; entry
""",
        },
    },
    {
        "name": "idx_split",
        "conf": {"html_split_index": True},
        "files": {
            "index": _GENINDEX_DOC,
        },
    },
    {
        "name": "idx_modindex",
        "files": {
            "index": """\
Modules
=======

.. py:module:: pkg
   :synopsis: The package.

.. py:module:: pkg.sub
   :synopsis: A subpackage.
   :platform: Unix, Windows

.. py:module:: pkg.sub.leaf
   :deprecated:

.. py:module:: other
   :synopsis: Another top-level module.

.. py:module:: zeta

References :py:mod:`pkg`, :py:mod:`pkg.sub.leaf`.
""",
        },
    },
    {
        "name": "idx_modindex_prefix",
        "conf": {"modindex_common_prefix": ["pkg."]},
        "files": {
            "index": """\
Modules
=======

.. py:module:: pkg.alpha

.. py:module:: pkg.beta
   :synopsis: Beta.

.. py:module:: pkg.beta.inner

.. py:module:: standalone
""",
        },
    },
    {
        "name": "idx_disabled",
        "conf": {"html_use_index": False, "html_domain_indices": False},
        "files": {
            "index": """\
No indices
==========

.. py:module:: hidden_mod

.. index:: single: not shown
""",
        },
    },
    {
        "name": "idx_domain_indices_list",
        "conf_py": "html_domain_indices = ['py-modindex']\n",
        "files": {
            "index": """\
Listed domain index
===================

.. py:module:: listed_mod
""",
        },
    },
    {
        "name": "idx_empty",
        "files": {
            "index": _leaf("Nothing indexed"),
        },
    },
]

# ---------------------------------------------------------------------------
# (e) config knobs, one group per project. Every project has two documents
# (so prev/next, rellinks and cross-page links exercise the knob) unless the
# knob is page-local.
# ---------------------------------------------------------------------------

_TWO_DOCS = {
    "index": """\
Home
====

.. toctree::

   other

Intro paragraph linking :doc:`other` and :ref:`other-label`.

Home Section
------------

* simple
* list
""",
    "other": """\
.. _other-label:

Other
=====

Other Section
-------------

Text.
""",
}


def _two_docs(**extra) -> dict:
    files = dict(_TWO_DOCS)
    files.update(extra)
    return files


_TEMPLATE_LAYOUT = """\
{% extends "!layout.html" %}
{%- block extrahead %}
    <meta name="custom-head" content="{{ custom_var }}" />
{{ super() }}
{%- endblock %}
{%- block footer %}
{{ super() }}
    <p class="custom-footer">{{ custom_var }} / {{ pagename }}</p>
{%- endblock %}
"""

_TEMPLATE_EXTRA_PAGE = """\
{% extends "layout.html" %}
{%- block body %}
  <h1>Extra page</h1>
  <p>{{ custom_var }} on {{ pagename }}</p>
{%- endblock %}
"""

CONFIG = [
    {
        "name": "cfg_titles",
        "conf_py": (
            "project = 'Demo & \"Q\"'\n"
            "version = '2.0'\n"
            "release = '2.0.1'\n"
            "copyright = '2001, Tester <tester@example.org>'\n"
            "html_title = 'Custom <b>Title</b> & more'\n"
            "html_short_title = 'Short'\n"
        ),
        "files": _two_docs(),
    },
    {
        "name": "cfg_default_titles",
        "conf_py": "version = '3.1'\nrelease = '3.1.4'\ncopyright = '1999, Plain'\n",
        "files": _two_docs(),
    },
    {
        "name": "cfg_copyright_list",
        "conf_py": "copyright = ['2001, Alice <a>', '2002, Bob']\n",
        "files": _two_docs(),
    },
    {
        "name": "cfg_permalinks_off",
        "conf": {"html_permalinks": False},
        "files": _two_docs(),
    },
    {
        "name": "cfg_permalinks_icon",
        "conf": {"html_permalinks_icon": "§"},
        "files": _two_docs(
            other="""\
.. _other-label:

Other
=====

.. code-block:: none
   :caption: Captioned

   code

.. table:: Captioned table

   ===  ===
   a    b
   ===  ===
"""
        ),
    },
    {
        "name": "cfg_source_off",
        "conf": {"html_copy_source": False, "html_show_sourcelink": False},
        "files": _two_docs(),
    },
    {
        "name": "cfg_sourcelink_hidden",
        "conf": {"html_show_sourcelink": False},
        "files": _two_docs(),
    },
    {
        "name": "cfg_sourcelink_suffix",
        "conf": {"html_sourcelink_suffix": ".rst"},
        "files": _two_docs(),
    },
    {
        "name": "cfg_suffixes",
        "conf": {"html_file_suffix": ".xhtml", "html_link_suffix": ".htm"},
        "files": _two_docs(),
    },
    {
        "name": "cfg_secnumber_suffix",
        "conf": {"html_secnumber_suffix": ") "},
        "files": _two_docs(
            index="""\
Home
====

.. toctree::
   :numbered:

   other

Home Section
------------
"""
        ),
    },
    {
        "name": "cfg_compact_lists_off",
        "conf": {"html_compact_lists": False},
        "files": _two_docs(),
    },
    {
        "name": "cfg_scaled_image_link_off",
        "conf": {"html_scaled_image_link": False},
        "files": _two_docs(
            other="""\
.. _other-label:

Other
=====

.. image:: pic.png
   :scale: 50%

.. image:: pic.png
   :width: 20px
"""
        ),
        "binary_files": {"pic.png": PNG_RED_40x30},
    },
    {
        "name": "cfg_footer_off",
        "conf": {
            "html_show_copyright": False,
            "html_show_sphinx": False,
            "html_show_search_summary": False,
        },
        "conf_py": "copyright = '2001, Hidden'\n",
        "files": _two_docs(),
    },
    {
        "name": "cfg_baseurl",
        "conf": {"html_baseurl": "https://example.org/docs/"},
        "files": _two_docs(),
    },
    {
        "name": "cfg_assets",
        "conf_py": """\
html_static_path = ['_static']
html_css_files = [
    'custom.css',
    ('print.css', {'media': 'print'}),
    ('https://cdn.example.org/remote.css', {'priority': 100}),
    ('late.css', {'priority': 900, 'title': 'Late & "quoted"'}),
    'missing.css',
]
html_js_files = [
    'custom.js',
    ('module.js', {'type': 'module', 'defer': 'defer'}),
    ('https://cdn.example.org/remote.js', {'async': 'async', 'priority': 100}),
    ('', {'body': 'var inline = 1;'}),
]
""",
        "files": _two_docs(),
        "data_files": {
            "_static/custom.css": "body { color: #333; }\n",
            "_static/print.css": "@media print { .sphinxsidebar { display: none; } }\n",
            "_static/late.css": "/* late */\n",
            "_static/custom.js": "console.log('custom');\n",
            "_static/module.js": "export const x = 1;\n",
            "_static/tmpl.css_t": "/* {{ project }} {{ docstitle|e }} */\n.x { width: {{ 2 * 3 }}px; }\n",
            "_static/sub/nested.css": "/* nested static file */\n",
        },
        "binary_files": {"_static/img/icon.png": PNG_BLUE_16x16},
    },
    {
        "name": "cfg_html_style",
        "conf_py": "html_static_path = ['_static']\nhtml_style = 'custom-style.css'\n",
        "files": _two_docs(),
        "data_files": {"_static/custom-style.css": "@import url('basic.css');\n"},
    },
    {
        "name": "cfg_static_override",
        "conf_py": "html_static_path = ['_static']\n",
        "files": _two_docs(),
        "data_files": {"_static/basic.css": "/* user override of the theme stylesheet */\n"},
    },
    {
        "name": "cfg_static_path_missing",
        "conf_py": "html_static_path = ['_static']\nhtml_extra_path = ['_nope']\n",
        "files": _two_docs(),
    },
    {
        "name": "cfg_extra_path",
        "conf_py": "html_extra_path = ['_extra']\n",
        "files": _two_docs(),
        "data_files": {
            "_extra/robots.dat": "User-agent: *\n",
            "_extra/.htaccess": "Options -Indexes\n",
            "_extra/humans.json": '{"team": "fixture"}\n',
            "_extra/sub/keep.html": "<p>kept verbatim</p>\n",
        },
    },
    {
        "name": "cfg_templates",
        "conf_py": """\
templates_path = ['_templates']
html_context = {'custom_var': 'from html_context <&>'}
html_additional_pages = {'extra': 'extra.html'}
""",
        "files": _two_docs(),
        "data_files": {
            "_templates/layout.html": _TEMPLATE_LAYOUT,
            "_templates/extra.html": _TEMPLATE_EXTRA_PAGE,
        },
    },
    {
        "name": "cfg_sidebars",
        "conf_py": """\
html_sidebars = {
    '**': ['globaltoc.html', 'searchbox.html'],
    'other': [],
    'o*': ['relations.html'],
    'i*': ['searchbox.html'],
    'special': ['localtoc.html', 'sourcelink.html'],
}
""",
        "files": _two_docs(
            index="""\
Home
====

.. toctree::

   other
   special
""",
            special="""\
Special
=======

Special Sub
-----------
""",
        ),
    },
    {
        "name": "cfg_logo_favicon",
        "conf_py": "html_logo = 'img/logo.png'\nhtml_favicon = 'img/fav.png'\n",
        "files": _two_docs(),
        "binary_files": {
            "img/logo.png": PNG_RED_40x30,
            "img/fav.png": PNG_BLUE_16x16,
        },
    },
    {
        "name": "cfg_logo_favicon_urls",
        "conf": {
            "html_logo": "https://example.org/logo.png",
            "html_favicon": "https://example.org/favicon.ico",
        },
        "files": _two_docs(),
    },
    {
        "name": "cfg_logo_missing",
        "conf": {"html_logo": "nope.png", "html_favicon": "nope.ico"},
        "files": _two_docs(),
    },
    {
        # SOURCE_DATE_EPOCH 1700000000 = 2023-11-14 22:13:20 UTC. Sphinx
        # forces UTC when it is set (sphinx/util/i18n.py:271-280). The
        # copyright names 2020 so correct_copyright_year never rewrites it.
        "name": "cfg_last_updated",
        "conf": {"html_last_updated_fmt": "%b %d, %Y %H:%M:%S (%a %j)"},
        "conf_py": "copyright = '2020, Dated'\n",
        "env": {"SOURCE_DATE_EPOCH": "1700000000"},
        "files": _two_docs(
            other="""\
.. _other-label:

Other
=====

Built on |today|.
"""
        ),
    },
    {
        "name": "cfg_last_updated_default_fmt",
        "conf": {"html_last_updated_fmt": ""},
        "env": {"SOURCE_DATE_EPOCH": "946684800"},
        "files": _two_docs(),
    },
    {
        "name": "cfg_opensearch",
        "conf": {"html_use_opensearch": "https://example.org/docs"},
        "conf_py": "html_favicon = 'fav.png'\n",
        "files": _two_docs(),
        "binary_files": {"fav.png": PNG_BLUE_16x16},
    },
    {
        "name": "cfg_theme_options",
        "conf_py": """\
html_theme_options = {
    'sidebarwidth': '300px',
    'body_min_width': '0',
    'body_max_width': 'none',
    'navigation_with_keys': True,
    'enable_search_shortcuts': False,
    'bogus_option': 'x',
}
""",
        "files": _two_docs(),
    },
    {
        "name": "cfg_nosidebar",
        "conf_py": "html_theme_options = {'nosidebar': True}\n",
        "files": _two_docs(),
    },
    {
        "name": "cfg_globaltoc_options",
        "conf_py": """\
html_sidebars = {'**': ['globaltoc.html']}
html_theme_options = {
    'globaltoc_collapse': False,
    'globaltoc_includehidden': True,
    'globaltoc_maxdepth': 1,
}
""",
        "files": _two_docs(
            index="""\
Home
====

.. toctree::

   other

.. toctree::
   :hidden:

   hidden
""",
            hidden=_leaf("Hidden"),
        ),
    },
]

# ---------------------------------------------------------------------------
# (f) dirhtml: only get_target_uri/get_output_path differ from html
# (sphinx/builders/dirhtml.py:27-38) -- every URI, content_root and the
# output layout (`<doc>/index.html`) moves.
# ---------------------------------------------------------------------------

DIRHTML = [
    {
        "name": "dir_nav",
        "builder": "dirhtml",
        "files": {
            "index": """\
Home
====

.. toctree::
   :maxdepth: 2

   intro
   guide/index
   guide/install
   api/deep/page
""",
            "intro": _leaf("Intro", "See :doc:`guide/install` and :ref:`deep-label`."),
            "guide/index": _leaf("Guide", "Back to :doc:`/index`."),
            "guide/install": """\
Install
=======

Install Step
------------

Relative :doc:`index` and :doc:`../intro`.
""",
            "api/deep/page": """\
.. _deep-label:

Deep Page
=========

Up :doc:`../../intro`.
""",
        },
    },
    {
        "name": "dir_assets",
        "builder": "dirhtml",
        "files": {
            "index": """\
Assets
======

.. toctree::

   sub/page

.. image:: img/red.png
   :scale: 50%

:download:`payload <files/data.bin>`
""",
            "sub/page": """\
Sub Page
========

.. figure:: ../img/blue.png

   From a subdirectory.

:download:`../files/data.bin`
""",
        },
        "data_files": {"files/data.bin": "payload\n"},
        "binary_files": {
            "img/red.png": PNG_RED_40x30,
            "img/blue.png": PNG_BLUE_16x16,
        },
    },
    {
        "name": "dir_indices",
        "builder": "dirhtml",
        "files": {
            "index": """\
Indices
=======

.. toctree::

   other

.. py:module:: dirmod
   :synopsis: A module.

.. py:function:: dirfunc()

.. index:: single: dir entry
""",
            "other": """\
Other
=====

.. py:module:: dirmod.sub

See :py:func:`dirmod.dirfunc` and :py:mod:`dirmod`.
""",
        },
    },
    {
        "name": "dir_split_index",
        "builder": "dirhtml",
        "conf": {"html_split_index": True},
        "files": {"index": _GENINDEX_DOC},
    },
    {
        "name": "dir_numbered",
        "builder": "dirhtml",
        "conf": {"numfig": True},
        "files": {
            "index": """\
Home
====

.. toctree::
   :numbered:

   chapter/index

See :numref:`dir-fig`.
""",
            "chapter/index": """\
Chapter
=======

.. figure:: ../pic.png
   :name: dir-fig

   A figure.

Section
-------
""",
        },
        "binary_files": {"pic.png": PNG_GREEN_8x4},
    },
]

# ---------------------------------------------------------------------------
# (g) highlighting: Pygments 2.21.0 through Sphinx's PygmentsBridge.
# `hl_default*` drop the base highlight_language='none' so Sphinx's own
# default ('default': python, or pycon for '>>>' blocks, silent fallback to
# text) applies to `::` blocks.
# ---------------------------------------------------------------------------

_PY_SAMPLE = '''\
import os

@decorator(arg=1)
class Klass(Base):
    """Docstring with 'quotes' & <angle>."""

    attr: int = 0x1F

    async def method(self, *args, **kwargs) -> None:
        if args and not kwargs:
            return f"{self.attr!r:>10} {1_000.5e-3}"
        raise ValueError('bad')  # comment
'''

HIGHLIGHT = [
    {
        "name": "hl_default",
        "unset": ["highlight_language"],
        "files": {
            "index": """\
Default highlighting
====================

Valid Python::

   def f(x):
       return x * 2  # double

Console session::

   >>> 1 + 1
   2
   >>> print("hi")
   hi

Not Python at all::

   $ ls -la | grep "x" && echo <ok>

Doctest block:

>>> sorted({3, 1, 2})
[1, 2, 3]
""",
        },
    },
    {
        "name": "hl_default_literalinclude",
        "unset": ["highlight_language"],
        "files": {
            "index": """\
Default literalinclude
======================

.. literalinclude:: example.py

.. literalinclude:: example.py
   :pyobject: top
   :emphasize-lines: 2
""",
        },
        "data_files": {"example.py": EXAMPLE_PY},
    },
    {
        "name": "hl_explicit",
        "files": {
            "index": """\
Explicit languages
==================

.. code-block:: python

""" + "".join("   " + line + "\n" if line else "\n" for line in _PY_SAMPLE.splitlines()) + """
.. code-block:: python3

   print("python3 alias")

.. code-block:: py

   x = [1, 2, 3]

.. code-block:: pycon

   >>> x = 1
   >>> x + 1
   2
   >>> raise ValueError("boom")
   Traceback (most recent call last):
     File "<stdin>", line 1, in <module>
   ValueError: boom

.. code-block:: Python

   capitalised = "strips surrounding newlines"

.. code-block:: text

   plain text <&>

.. code-block:: none

   none <&>

.. code-block:: default

   def default_lang(): pass

.. code-block:: nosuchlexer

   unknown lexer name
""",
        },
    },
    {
        "name": "hl_directive",
        "files": {
            "index": """\
Highlight directive
===================

Before any directive::

   still none

.. highlight:: python

Now python::

   def g(): return 1

.. highlight:: python
   :linenothreshold: 3

Short::

   a = 1

Long enough for line numbers::

   a = 1
   b = 2
   c = 3
   d = 4

.. highlight:: none

Back to none::

   def not_highlighted(): pass
""",
        },
    },
    {
        "name": "hl_errors",
        "files": {
            "index": """\
Lexing errors
=============

.. code-block:: python

   $ this is not python ?

.. code-block:: pycon

   >>> ok = 1
   >>> $bad

.. code-block:: python
   :force:

   $ forced, no warning
""",
        },
    },
    {
        "name": "hl_linenos",
        "files": {
            "index": """\
Line numbers
============

.. code-block:: python
   :linenos:
   :emphasize-lines: 1,3

   first = 1
   second = 2
   third = 3

.. code-block:: python
   :linenos:
   :lineno-start: 98

   ninety_eight = 98
   ninety_nine = 99
   hundred = 100

.. code-block:: python
   :emphasize-lines: 2

   no_linenos = True
   emphasized = True
""",
        },
    },
    {
        "name": "hl_linenos_table",
        "conf": {"html_codeblock_linenos_style": "table"},
        "files": {
            "index": """\
Table line numbers
==================

.. code-block:: python
   :linenos:
   :emphasize-lines: 2

   a = 1
   b = 2
   c = 3

.. code-block:: none
   :linenos:
   :lineno-start: 7

   plain
   lines
""",
        },
    },
    {
        "name": "hl_pygments_style_sphinx",
        "conf": {"pygments_style": "sphinx"},
        "files": {
            "index": """\
Sphinx style
============

.. code-block:: python

   def styled(x):
       return "sphinx style"
""",
        },
    },
    {
        "name": "hl_pygments_style_friendly",
        "conf": {"pygments_style": "friendly"},
        "files": {
            "index": """\
Friendly style
==============

.. code-block:: python

   def styled(x):
       return "friendly style"
""",
        },
    },
    {
        "name": "hl_inline_code_role",
        "files": {
            "index": """\
Inline code role
================

.. role:: py(code)
   :language: python

.. role:: txt(code)
   :language: text

Inline :py:`def f(x): return x` and :txt:`plain` and :code:`no language`.
""",
        },
    },
]

# ---------------------------------------------------------------------------
# (h) smartquotes: SphinxSmartQuotes (priority 750) on, per language.
# ---------------------------------------------------------------------------

_SQ_BODY = """\
"Double" and 'single' quotes, it's an apostrophe, dashes -- and ---,
an ellipsis... and ``"literal" -- untouched``.

* "quoted" list item
* 'another' one

.. code-block:: none

   "code" -- untouched...

:kbd:`"kbd"` and :samp:`"samp" --`.
"""

SMARTQUOTES = [
    {
        "name": "sq_en",
        "conf": {"smartquotes": True},
        "files": {
            "index": f"""\
"Smart" Quotes -- Title
=======================

.. toctree::

   other

{_SQ_BODY}""",
            "other": """\
Other "Page" -- Next
====================

Text with "quotes".
""",
        },
    },
    {
        "name": "sq_classes",
        "conf": {"smartquotes": True},
        "files": {
            "index": """\
Per-node languages
==================

.. rst-class:: language-de

"Deutsch" mit 'einfachen' Anführungszeichen.

.. rst-class:: language-fr

"Français" avec 'guillemets'.

.. rst-class:: language-it

"Italiano" e 'singoli'.

.. rst-class:: language-ru

"Русский" и 'одиночные'.

.. rst-class:: language-ja

"日本語" は 'そのまま'.

.. container:: language-de-x-altquot

   "Alternative" German quotes.
""",
        },
    },
    {
        "name": "sq_action_q",
        "conf": {"smartquotes": True, "smartquotes_action": "q"},
        "files": {"index": f"Action q\n========\n\n{_SQ_BODY}"},
    },
    {
        "name": "sq_excluded_builder",
        "conf": {"smartquotes": True},
        "conf_py": "smartquotes_excludes = {'languages': [], 'builders': ['html']}\n",
        "files": {"index": f"Excluded\n========\n\n{_SQ_BODY}"},
    },
    {
        "name": "sq_lang_de",
        "conf": {"smartquotes": True, "language": "de"},
        "files": {
            "index": f"""\
Deutsch
=======

.. toctree::

   other

.. note:: Ein Hinweis.

{_SQ_BODY}""",
            "other": _leaf("Andere Seite", '"Zitat" hier.'),
        },
    },
    {
        "name": "sq_lang_fr",
        "conf": {"smartquotes": True, "language": "fr"},
        "files": {
            "index": f"""\
Français
========

.. toctree::

   other

.. warning:: Un avertissement.

{_SQ_BODY}""",
            "other": _leaf("Autre page", '"Citation" ici.'),
        },
    },
    {
        "name": "sq_lang_ja",
        "conf": {"smartquotes": True, "language": "ja"},
        "files": {
            "index": f"Japanese\n========\n\n{_SQ_BODY}",
        },
    },
]

# ---------------------------------------------------------------------------
# (i) alabaster: html_theme UNSET -> Sphinx's default theme (decision 5).
# ---------------------------------------------------------------------------

ALABASTER = [
    {
        "name": "alab_basic",
        "unset": ["html_theme"],
        "files": {
            "index": """\
Welcome
=======

.. toctree::
   :maxdepth: 2
   :caption: Contents

   a
   b

Intro text with ``code``.

.. code-block:: python

   def alabaster():
       return "default theme"
""",
            "a": """\
Page A
======

A Section
---------

.. note:: A note.
""",
            "b": _leaf("Page B"),
        },
    },
    {
        "name": "alab_options",
        "unset": ["html_theme"],
        "conf_py": """\
html_theme_options = {
    'description': 'A <b>described</b> project',
    'github_user': 'someone',
    'github_repo': 'something',
    'github_banner': True,
    'fixed_sidebar': True,
    'show_powered_by': False,
    'page_width': '1000px',
    'show_relbars': True,
    'extra_nav_links': {'Home page': 'https://example.org/'},
}
""",
        "files": _two_docs(),
    },
    {
        "name": "alab_logo_copyright",
        "unset": ["html_theme"],
        "conf_py": (
            "html_logo = 'logo.png'\n"
            "copyright = ['2001, Alice <a>', '2002, Bob']\n"
            "html_show_sourcelink = True\n"
        ),
        "files": _two_docs(),
        "binary_files": {"logo.png": PNG_RED_40x30},
    },
    {
        "name": "alab_indices",
        "unset": ["html_theme"],
        "files": {
            "index": """\
Indexed
=======

.. py:module:: alabmod
   :synopsis: Under alabaster.

.. py:function:: alabfunc()

.. index:: single: alabaster entry
""",
        },
    },
    {
        "name": "alab_footer_off",
        "unset": ["html_theme"],
        "conf": {"html_show_sphinx": False, "html_show_copyright": False},
        "files": _two_docs(),
    },
]

# ---------------------------------------------------------------------------
# themes: the other builtin themes (decision 5: "the same engine renders"
# them). One small project each, html_theme set explicitly.
# ---------------------------------------------------------------------------

_THEME_DOCS = {
    "index": """\
Theme Home
==========

.. toctree::

   other

.. code-block:: python

   x = 1

.. note:: A note.
""",
    "other": """\
Other
=====

Section
-------

Text.
""",
}

THEMES = [
    {"name": f"theme_{theme}", "conf": {"html_theme": theme}, "files": _THEME_DOCS}
    for theme in (
        "agogo",
        "bizstyle",
        "classic",
        "default",
        "epub",
        "haiku",
        "nature",
        "nonav",
        "pyramid",
        "scrolls",
        "sphinxdoc",
        "traditional",
    )
]

FAMILIES = {
    "nodes": NODES,
    "toctree": TOCTREE,
    "indices": INDICES,
    "config": CONFIG,
    "dirhtml": DIRHTML,
    "highlight": HIGHLIGHT,
    "smartquotes": SMARTQUOTES,
    "alabaster": ALABASTER,
    "themes": THEMES,
}

# ---------------------------------------------------------------------------
# Harness
# ---------------------------------------------------------------------------


class RecordingIO(io.StringIO):
    """`logging.StreamHandler.emit` writes `msg + terminator` in ONE call
    (through Sphinx's SafeEncodingWriter, also one call), so each write is
    one warning record."""

    def __init__(self):
        super().__init__()
        self.records = []

    def write(self, s):
        self.records.append(s)
        return super().write(s)


def spellings(path: Path) -> list:
    """Absolute (raw + resolved) and cwd-relative spellings of `path`
    (tools/gen_env_fixture.py:srcdir_spellings, Scope-8)."""
    return [
        str(path),
        str(path.resolve()),
        os.path.relpath(str(path)),
        os.path.relpath(str(path.resolve())),
    ]


class Normalizer:
    def __init__(self, srcdir: Path, outdir: Path, tmp: Path):
        forms = [(form, PROJECT_TOKEN) for form in spellings(srcdir)]
        forms += [(form, OUTDIR_TOKEN) for form in spellings(outdir)]
        # Longest first: an overlapping pair (macOS /var inside /private/var)
        # cannot leave a mangled half-replacement behind.
        self.forms = sorted(set(forms), key=lambda f: len(f[0]), reverse=True)
        self.leak_checks = sorted(
            {form for form, _ in self.forms} | set(spellings(tmp)), key=len, reverse=True
        )

    def __call__(self, text: str) -> str:
        for form, token in self.forms:
            text = text.replace(form, token)
        for form in self.leak_checks:
            # A cwd-relative spelling can be as short as "." -- only check
            # forms that could not occur by accident.
            if len(form) > 3:
                assert form not in text, f"tmp path {form!r} leaked into:\n{text[:2000]}"
        return text

    def obj(self, value):
        if isinstance(value, str):
            return self(value)
        if isinstance(value, list):
            return [self.obj(v) for v in value]
        if isinstance(value, dict):
            return {self(k): self.obj(v) for k, v in value.items()}
        return value


def jsonable(value):
    """Plain JSON data for a context value. Lazy translation proxies and
    markupsafe strings become their text; NamedTuples/tuples become lists;
    Sphinx's asset objects are handled by the caller."""
    if isinstance(value, dict):
        return {str(k): jsonable(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [jsonable(v) for v in value]
    if value is None or isinstance(value, (bool, int, float)):
        return value
    if isinstance(value, str):
        return str(value)
    if isinstance(value, sphinx.locale._TranslationProxy):
        return str(value)
    text = f"<{type(value).__name__}>{value}"
    assert " at 0x" not in text, f"non-deterministic repr in context: {text}"
    return text


def asset_record(asset) -> list:
    return [str(asset.filename), jsonable(dict(sorted(asset.attributes.items()))), asset.priority]


def leaf_types(value, out: set) -> None:
    if isinstance(value, dict):
        for k, v in value.items():
            leaf_types(k, out)
            leaf_types(v, out)
    elif isinstance(value, (list, tuple, set, frozenset)):
        for v in value:
            leaf_types(v, out)
    else:
        out.add(type(value).__name__)


MODELABLE_LEAVES = {"str", "int", "bool", "NoneType"}


def conf_of(entry: dict) -> dict:
    conf = {k: v for k, v in BASE_CONFOVERRIDES.items() if k not in entry.get("unset", ())}
    for key in entry.get("unset", ()):
        assert key in BASE_CONFOVERRIDES, f"{entry['name']}: unset of non-base key {key}"
    conf.update(entry.get("conf", {}))
    for key, value in conf.items():
        # -D carries a scalar, a comma-free str list (comma-joined), or one
        # dict key at a time (`-D numfig_format.figure=...`, which Sphinx
        # merges like the typed dict the env corpus passes).
        ok = (
            isinstance(value, (str, bool, int))
            or (isinstance(value, list) and all(isinstance(v, str) and "," not in v for v in value))
            or (isinstance(value, dict) and all(isinstance(v, (str, bool, int)) for v in value.values()))
        )
        assert ok, f"{entry['name']}: conf {key}={value!r} is not -D-expressible; use conf_py"
    return conf


def check_date_policy(entry: dict, conf: dict, conf_py: str) -> None:
    dated = "html_last_updated_fmt" in conf or "html_last_updated_fmt" in conf_py
    uses_today = any("|today|" in text for text in entry["files"].values())
    pinned_today = "today" in conf or re.search(r"^today\s*=", conf_py, re.M)
    has_epoch = "SOURCE_DATE_EPOCH" in entry.get("env", {})
    assert not dated or has_epoch, f"{entry['name']}: html_last_updated_fmt needs SOURCE_DATE_EPOCH"
    assert not uses_today or pinned_today or has_epoch, f"{entry['name']}: |today| needs a pin"


def write_members(srcdir: Path, entry: dict, conf_py: str) -> None:
    srcdir.mkdir(parents=True)
    (srcdir / "conf.py").write_text(conf_py, encoding="utf-8")
    for docname, text in entry["files"].items():
        path = srcdir / f"{docname}.rst"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    for relpath, text in entry.get("data_files", {}).items():
        assert not relpath.endswith((".rst", ".md", ".txt")), (
            f"{entry['name']}: data file {relpath} has a document suffix"
        )
        path = srcdir / relpath
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    for relpath, data in entry.get("binary_files", {}).items():
        path = srcdir / relpath
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)


def templated_statics(app) -> set:
    """Output paths of every static file Sphinx RENDERS (`*_t` / `*.jinja`,
    sphinx/util/fileutil.py:_template_basename) from the theme chain and
    html_static_path, plus the generated Pygments stylesheets."""
    out = {"_static/pygments.css", "_static/pygments_dark.css"}
    dirs = [Path(d) / "static" for d in app.builder.theme.get_theme_dirs()]
    dirs += [Path(app.confdir, p) for p in app.config.html_static_path]
    for base in dirs:
        if not base.is_dir():
            continue
        for f in base.rglob("*"):
            if not f.is_file():
                continue
            name = f.name.lower()
            rel = f.relative_to(base)
            if name.endswith("_t"):
                out.add(f"_static/{rel.as_posix()[:-2]}")
            elif name.endswith(".jinja"):
                out.add(f"_static/{rel.as_posix()[:-6]}")
    return out


def classify(rel: str, pages: dict) -> str:
    if rel in pages:
        return "page"
    if rel.startswith("_sources/"):
        return "source"
    if rel == ".buildinfo":
        return "buildinfo"
    if rel == "objects.inv":
        return "inventory"
    if rel == "searchindex.js":
        return "searchindex"
    if rel.startswith("_static/"):
        return "static"
    if rel.startswith("_images/"):
        return "image"
    if rel.startswith("_downloads/"):
        return "download"
    return "extra"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def build_project(entry: dict, texts: dict) -> dict:
    name = entry["name"]
    builder_name = entry.get("builder", "html")
    conf = conf_of(entry)
    conf_py = CONF_PY + entry.get("conf_py", "")
    check_date_policy(entry, conf, conf_py)

    tmp = Path(tempfile.mkdtemp(prefix="html_oracle_")).resolve()
    srcdir = tmp / "source"
    outdir = tmp / "build"
    doctreedir = tmp / "doctrees"
    write_members(srcdir, entry, conf_py)
    norm = Normalizer(srcdir, outdir, tmp)

    records: dict = {}
    current = [None]
    buildinfo_capture: dict = {}
    warn = RecordingIO()

    orig_create_build_info = StandaloneHTMLBuilder.create_build_info

    def create_build_info(self):
        # The values exactly as BuildInfo hashes them: BEFORE init_css_files/
        # init_js_files setdefault('priority', 800) on the attr dicts.
        values = {c.name: c.value for c in self.config.filter(frozenset({"html"}))}
        leaves: set = set()
        for value in values.values():
            leaf_types(value, leaves)
        buildinfo_capture["values"] = {
            key: {"repr": repr(value), "hash": stable_hash(value)}
            for key, value in sorted(values.items())
        }
        buildinfo_capture["config_hash"] = stable_hash(values)
        buildinfo_capture["tags_hash"] = stable_hash(sorted(self.tags))
        buildinfo_capture["leaves"] = sorted(leaves)
        return orig_create_build_info(self)

    saved_env = {key: os.environ.get(key) for key in entry.get("env", {})}
    saved_path = sys.path.copy()
    sphinx.locale.translators.clear()
    sphinx.pycode.ModuleAnalyzer.cache.clear()
    html_assets._file_checksum_inner.cache_clear()
    StandaloneHTMLBuilder.create_build_info = create_build_info
    os.environ.update(entry.get("env", {}))
    try:
        with patch_docutils(srcdir), docutils_namespace():
            app = Sphinx(
                srcdir=srcdir,
                confdir=srcdir,
                outdir=outdir,
                doctreedir=doctreedir,
                buildername=builder_name,
                confoverrides=copy.deepcopy(conf),
                status=io.StringIO(),
                warning=warn,
                freshenv=True,
                parallel=0,
            )
            orig_handle_page = app.builder.handle_page

            def handle_page(pagename, addctx, templatename="page.html", *args, **kwargs):
                outfilename = kwargs.get("outfilename")
                path = Path(outfilename) if outfilename else app.builder.get_output_path(pagename)
                rel = Path(path).resolve().relative_to(outdir).as_posix()
                assert rel not in records, f"{name}: {rel} written twice"
                if templatename == "page.html":
                    addrec = {k: jsonable(addctx.get(k)) for k in DOC_KEYS}
                else:
                    addrec = {
                        k: jsonable(v) for k, v in sorted(addctx.items()) if not callable(v)
                    }
                rec = {"pagename": pagename, "template": templatename, "addctx": addrec}
                records[rel] = rec
                current[0] = rec
                try:
                    return orig_handle_page(pagename, addctx, templatename, *args, **kwargs)
                finally:
                    current[0] = None

            app.builder.handle_page = handle_page

            def on_page_context(app_, pagename, templatename, ctx, doctree):
                rec = current[0]
                assert rec is not None and rec["pagename"] == pagename, pagename
                # Never call ctx['toctree']() here: it re-runs _resolve_toctree
                # and would duplicate warnings in the recorded stream.
                sub = {k: jsonable(ctx[k]) for k in CTX_KEYS if k in ctx}
                sub["css_files"] = [asset_record(c) for c in ctx.get("css_files", [])]
                sub["script_files"] = [asset_record(j) for j in ctx.get("script_files", [])]
                rec["ctx"] = sub

            app.connect("html-page-context", on_page_context)

            credits = [
                app.builder.templates.render_string(
                    BASIC_CREDIT_TEMPLATE, {"sphinx_version": sphinx.__version__}
                ),
                ALABASTER_CREDIT,
            ]
            app.build(False, [])
            templated = templated_statics(app)
            theme_name = app.config.html_theme
    finally:
        StandaloneHTMLBuilder.create_build_info = orig_create_build_info
        for key, value in saved_env.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        sys.path[:] = saved_path

    try:
        expect = collect_outputs(
            name, outdir, records, templated, credits, theme_name, norm, texts
        )
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    # .buildinfo self-check: Sphinx's own hash of the captured values is the
    # file's config line (proves the capture point is the hashing point).
    lines = expect["buildinfo"].split("\n")
    assert lines[2] == f"config: {buildinfo_capture['config_hash']}", (name, lines)
    assert lines[3] == f"tags: {buildinfo_capture['tags_hash']}", (name, lines)

    expect["warnings"] = []
    for record in warn.records:
        assert record.endswith("\n"), (name, record)
        expect["warnings"].append(norm(record[:-1]))

    out = {
        "name": name,
        "builder": builder_name,
        "conf": conf,
        "conf_py": conf_py,
        "files": entry["files"],
        "expect": expect,
        "_buildinfo_values": buildinfo_capture["values"],
        "_buildinfo_leaves": buildinfo_capture["leaves"],
    }
    if entry.get("unset"):
        out["unset"] = list(entry["unset"])
    if entry.get("env"):
        out["env"] = dict(entry["env"])
    if entry.get("data_files"):
        out["data_files"] = entry["data_files"]
    if entry.get("binary_files"):
        out["binary_files"] = {
            rel: base64.b64encode(data).decode("ascii")
            for rel, data in sorted(entry["binary_files"].items())
        }
    return out


def normalize_credit(rel: str, text: str, show_sphinx: bool, credits: list) -> str:
    found = sum(text.count(c) for c in credits)
    if show_sphinx:
        assert found == 1, f"{rel}: expected exactly one generator credit, found {found}"
    else:
        assert found == 0, f"{rel}: show_sphinx is off but a credit was found"
    for credit in credits:
        text = text.replace(credit, CREDIT_TOKEN)
    return text


def collect_outputs(name, outdir, records, templated, credits, theme_name, norm, texts) -> dict:
    output_files = {}
    pages = {}
    sources = {}
    buildinfo = None
    inventory = None
    for path in sorted(p for p in outdir.rglob("*") if p.is_file()):
        rel = path.relative_to(outdir).as_posix()
        raw = path.read_bytes()
        kind = classify(rel, records)
        entry = {"kind": kind}
        if kind == "page":
            ctx = records[rel].get("ctx")
            assert ctx is not None, f"{name}: {rel} never reached html-page-context"
            # opensearch.xml is the one handle_page template without the
            # layout footer; the epub and nonav themes empty the footer
            # block (themes/epub/layout.html:13, themes/nonav/layout.html:14).
            has_footer = (
                records[rel]["template"] != "opensearch.xml"
                and theme_name not in CREDITLESS_THEMES
            )
            text = normalize_credit(
                rel, norm(raw.decode("utf-8")), has_footer and bool(ctx["show_sphinx"]), credits
            )
            assert CREDIT_TOKEN not in raw.decode("utf-8")
            pages[rel] = text.split("\n")
            entry["sha256"] = sha256(text.encode("utf-8"))
            entry["text"] = True
        elif kind == "source":
            text = raw.decode("utf-8")
            assert norm(text) == text, f"{name}: {rel} carries a tmp path"
            sources[rel] = text
            entry["sha256"] = sha256(raw)
            entry["text"] = True
        elif kind == "buildinfo":
            buildinfo = raw.decode("utf-8")
            entry["sha256"] = sha256(raw)
            entry["text"] = True
        elif kind == "inventory":
            head = b""
            rest = raw
            for _ in range(4):
                line, _sep, rest = rest.partition(b"\n")
                head += line + b"\n"
            inventory = {
                "header": head.decode("utf-8").split("\n")[:4],
                "payload": zlib.decompress(rest).decode("utf-8"),
            }
            entry["sha256"] = sha256(raw)
            entry["text"] = False
        elif kind == "searchindex":
            # M3. Presence only: the index tokenizes page text, and a
            # keep_warnings page's absolute source path puts the tmp dir's
            # random name into `terms` -- the bytes are not normalizable.
            pass
        elif kind == "static" and rel in templated:
            text = raw.decode("utf-8")
            assert norm(text) == text, f"{name}: {rel} carries a tmp path"
            digest = sha256(raw)
            texts[digest] = text.split("\n")
            entry["sha256"] = digest
            entry["text"] = True
        else:
            entry["sha256"] = sha256(raw)
            entry["text"] = False
        output_files[rel] = entry

    assert buildinfo is not None, f"{name}: no .buildinfo"
    assert inventory is not None, f"{name}: no objects.inv"
    missing = set(records) - set(output_files)
    assert not missing, f"{name}: handle_page wrote files that are not on disk: {missing}"
    page_context = {}
    for rel, rec in sorted(records.items()):
        page_context[rel] = norm.obj(
            {
                "pagename": rec["pagename"],
                "template": rec["template"],
                "addctx": rec["addctx"],
                "ctx": rec["ctx"],
            }
        )
    return {
        "output_files": output_files,
        "pages": pages,
        "page_context": page_context,
        "sources": sources,
        "buildinfo": buildinfo,
        "inventory": inventory,
    }


# The build whose hashed option values are the file-level
# `buildinfo_reference`: one document, BASE_CONFOVERRIDES + CONF_PY and
# nothing else, i.e. every html-rebuild option at its default as a plain
# project sees it. Not recorded as a project.
REFERENCE_ENTRY = {
    "name": "_buildinfo_reference",
    "files": {"index": "Reference\n=========\n"},
}


def attach_buildinfo_tables(family_projects: list, reference: dict) -> None:
    """Replace each project's captured per-option values by its delta
    against the file-level `buildinfo_reference`."""
    for project in family_projects:
        values = project.pop("_buildinfo_values")
        leaves = project.pop("_buildinfo_leaves")
        delta = {
            key: value
            for key, value in values.items()
            if reference.get(key) != value
        }
        missing = sorted(set(reference) - set(values))
        assert not missing, f"{project['name']}: options vanished from the hash: {missing}"
        project["expect"]["buildinfo_config"] = delta
        unmodelable = sorted(set(leaves) - MODELABLE_LEAVES)
        project["expect"]["buildinfo_modelable"] = not unmodelable
        if unmodelable:
            project["expect"]["buildinfo_unmodelable_leaves"] = unmodelable


def generate_family(family: str, entries: list) -> dict:
    reference = build_project(REFERENCE_ENTRY, {})["_buildinfo_values"]
    texts: dict = {}
    projects = []
    for entry in entries:
        try:
            project = build_project(entry, texts)
        except Exception as exc:
            raise RuntimeError(f"project {entry['name']!r} failed: {exc}") from exc
        project["family"] = family
        projects.append(project)
    attach_buildinfo_tables(projects, reference)
    return {
        "schema_version": SCHEMA_VERSION,
        "generator": GENERATOR,
        "family": family,
        "sphinx_version": sphinx.__version__,
        "docutils_version": docutils.__version__,
        "pins": dict(PINS),
        "pillow": "absent",
        "base_conf": dict(BASE_CONFOVERRIDES),
        "tokens": {
            "project": PROJECT_TOKEN,
            "outdir": OUTDIR_TOKEN,
            "generator_credit": CREDIT_TOKEN,
        },
        "buildinfo_reference": reference,
        "texts": dict(sorted(texts.items())),
        "projects": projects,
    }


def family_entries() -> dict:
    structural = []
    for entry in gen_env_fixture.PROJECTS:
        if entry["name"] in EXCLUDED_ENV_PROJECTS:
            continue
        ported = {
            "name": f"env_{entry['name']}",
            "conf": dict(entry.get("conf", {})),
            "files": entry["files"],
        }
        if entry.get("data_files"):
            ported["data_files"] = entry["data_files"]
        structural.append(ported)
    for excluded in EXCLUDED_ENV_PROJECTS:
        assert any(e["name"] == excluded for e in gen_env_fixture.PROJECTS), (
            f"EXCLUDED_ENV_PROJECTS names {excluded!r}, which the env corpus no longer has"
        )
    return {"structural": structural, **FAMILIES}


def first_difference(a, b, where: str):
    """Yield the path of the first point where two JSON values differ."""
    if type(a) is not type(b):
        yield f"{where} (type {type(a).__name__} vs {type(b).__name__})"
    elif isinstance(a, dict):
        for key in sorted(set(a) | set(b)):
            if key not in a or key not in b:
                yield f"{where}.{key} (present on one side only)"
                return
            if a[key] != b[key]:
                yield from first_difference(a[key], b[key], f"{where}.{key}")
                return
    elif isinstance(a, list):
        if len(a) != len(b):
            yield f"{where} (length {len(a)} vs {len(b)})"
        for i, (x, y) in enumerate(zip(a, b)):
            if x != y:
                yield from first_difference(x, y, f"{where}[{i}]")
                return
    elif a != b:
        yield f"{where}: {str(a)[:300]!r} vs {str(b)[:300]!r}"


def main() -> int:
    families = family_entries()
    names = [e["name"] for entries in families.values() for e in entries]
    dupes = sorted({n for n in names if names.count(n) > 1})
    assert not dupes, f"project names must be unique: {dupes}"
    only = set(sys.argv[1:])

    outputs = {}
    for family, entries in families.items():
        if only and family not in only:
            continue
        first = generate_family(family, entries)
        again = generate_family(family, entries)
        first_json = json.dumps(first, indent=1, sort_keys=True, ensure_ascii=False)
        again_json = json.dumps(again, indent=1, sort_keys=True, ensure_ascii=False)
        if first_json != again_json:
            print(f"DETERMINISM VIOLATION in family {family}: two passes differ", file=sys.stderr)
            for where in first_difference(first, again, family):
                print(f"  first difference at {where}", file=sys.stderr)
            return 1
        outputs[family] = first_json

    for family, text in outputs.items():
        path = FIXTURE_DIR / f"html_differential_{family}.json"
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)
            f.write("\n")
        count = len(families[family])
        size = len(text.encode("utf-8")) + 1
        print(f"wrote {path.relative_to(FIXTURE_DIR.parent.parent)}: {count} projects, {size} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
