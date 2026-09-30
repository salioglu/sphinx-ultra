#!/usr/bin/env python3
"""Prototype for tools/gen_html_fixture.py (M2 wave 5 HTML-layer oracle).

Builds a tiny 2-document project with a REAL `sphinx-build -b html`
(SphinxTestApp(buildername='html') == the same Sphinx.build() a CLI run does)
under html_theme='basic', and captures, per page:

  * handle_page(pagename, addctx, templatename) arguments   [monkeypatch]
      -> for documents: body/toc/display_toc/title/prev/next/parents/
         rellinks/sourcename/meta/metatags/page_source_suffix/has_maths
      -> for genindex/py-modindex/search: their own addctx keys
  * the FULL merged template context at 'html-page-context'   [event]
      -> css/js file lists, sidebars, content_root, globaltoc output
  * the rendered file on disk (full page), plus the output tree listing,
    .buildinfo, objects.inv entries (decoded with Sphinx's own reader)
  * the warning stream, one record per handler write

Run:
  PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' \
      --with 'docutils==0.22.4' python html_probe.py [--full]
"""

import hashlib
import io
import json
import re
import shutil
import sys
import tempfile
from pathlib import Path

import docutils
import sphinx

assert sphinx.__version__ == "9.1.0", sphinx.__version__
assert docutils.__version__ == "0.22.4", docutils.__version__

from sphinx.util.console import nocolor  # noqa: E402

nocolor()

from sphinx.builders.html import StandaloneHTMLBuilder  # noqa: E402
from sphinx.testing.util import SphinxTestApp  # noqa: E402
from sphinx.util.docutils import docutils_namespace, patch_docutils  # noqa: E402
from sphinx.util.inventory import InventoryFile  # noqa: E402

TOKEN = "<project>"

CONF_PY = (
    "project = 'fixture'\n"
    "extensions = []\n"
    "master_doc = 'index'\n"
    "exclude_patterns = ['_build']\n"
)

FILES = {
    "index": """\
Welcome
=======

.. toctree::
   :maxdepth: 2

   second

Intro paragraph with *emphasis*, ``code`` and a :ref:`link <sec-label>`.

.. _local-target:

Local section
-------------

.. note::

   A note.

.. code-block:: python

   print("hi")

.. index:: single: welcome entry
""",
    "second": """\
Second page
===========

.. _sec-label:

Sub A
-----

Back to :doc:`index`.

Sub B
-----

.. py:function:: spam(x: int) -> str

   Does spam.
""",
}

# Keys of the per-document addctx (StandaloneHTMLBuilder.get_doc_context +
# write_doc's has_maths_elements). Everything here is plain data.
DOC_KEYS = [
    "title", "body", "toc", "display_toc", "prev", "next", "parents",
    "rellinks", "sourcename", "meta", "metatags", "page_source_suffix",
    "has_maths_elements",
]

# Context keys taken from the merged html-page-context ctx that a Rust
# comparison can use without re-deriving Sphinx internals.
CTX_KEYS = [
    "pagename", "content_root", "pageurl", "sidebars", "docstitle",
    "shorttitle", "project", "release", "version", "copyright",
    "last_updated", "show_source", "has_source", "show_sphinx",
    "sphinx_version", "file_suffix", "link_suffix", "use_opensearch",
    "language", "html5_doctype", "builder", "style", "logo_url",
    "favicon_url", "embedded", "show_copyright", "show_search_summary",
    "theme_nosidebar", "theme_globaltoc_maxdepth",
]


class RecordingIO(io.StringIO):
    """logging.StreamHandler.emit does one write per record."""

    def __init__(self):
        super().__init__()
        self.records = []

    def write(self, s):
        self.records.append(s)
        return super().write(s)


def jsonable(value):
    if isinstance(value, dict):
        return {str(k): jsonable(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [jsonable(v) for v in value]
    if isinstance(value, (str, int, float, bool)) or value is None:
        return value
    return f"<{type(value).__name__}>{value}"


def normalize(text, spellings):
    for s in spellings:
        text = text.replace(s, TOKEN)
    return text


def build(files, confoverrides):
    base = Path(tempfile.mkdtemp(prefix="html_oracle_")).resolve() / "src"
    base.mkdir(parents=True)
    (base / "conf.py").write_text(CONF_PY, encoding="utf-8")
    for docname, text in files.items():
        p = base / f"{docname}.rst"
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, encoding="utf-8")
    spellings = sorted({str(base), str(base.resolve())}, key=len, reverse=True)

    pages = {}
    warn = RecordingIO()

    orig_handle_page = StandaloneHTMLBuilder.handle_page

    def handle_page(self, pagename, addctx, templatename="page.html", *a, **kw):
        rec = pages.setdefault(pagename, {})
        rec["templatename"] = templatename
        if templatename == "page.html":
            rec["addctx"] = {k: jsonable(addctx.get(k)) for k in DOC_KEYS}
        else:
            # genindex / domainindex / search: keep every plain-data key
            rec["addctx"] = {
                k: jsonable(v) for k, v in addctx.items() if not callable(v)
            }
        return orig_handle_page(self, pagename, addctx, templatename, *a, **kw)

    def on_page_context(app, pagename, templatename, ctx, doctree):
        rec = pages.setdefault(pagename, {})
        rec["ctx"] = {k: jsonable(ctx.get(k)) for k in CTX_KEYS}
        rec["ctx"]["css_files"] = [
            [str(c.filename), dict(sorted(c.attributes.items())), c.priority]
            for c in ctx["css_files"]
        ]
        rec["ctx"]["script_files"] = [
            [str(j.filename), dict(sorted(j.attributes.items())), j.priority]
            for j in ctx["script_files"]
        ]
        # What basic/globaltoc.html would render (theme defaults: collapse
        # true, includehidden false, maxdepth '' -> -1 in _get_local_toctree).
        rec["ctx"]["globaltoc"] = ctx["toctree"](
            collapse=True, includehidden=False, maxdepth=-1
        )

    StandaloneHTMLBuilder.handle_page = handle_page
    try:
        with docutils_namespace(), patch_docutils(str(base)):
            app = SphinxTestApp(
                buildername="html",
                srcdir=base,
                status=io.StringIO(),
                warning=warn,
                confoverrides=dict(confoverrides),
            )
            try:
                app.connect("html-page-context", on_page_context)
                app.build()
                outdir = Path(app.outdir)
                listing = {}
                for f in sorted(outdir.rglob("*")):
                    if f.is_file():
                        rel = f.relative_to(outdir).as_posix()
                        listing[rel] = hashlib.sha256(f.read_bytes()).hexdigest()[:16]
                full = {}
                for pagename in pages:
                    path = outdir / f"{pagename}.html"
                    if path.is_file():
                        full[pagename] = normalize(
                            path.read_text(encoding="utf-8"), spellings
                        )
                buildinfo = (outdir / ".buildinfo").read_text(encoding="utf-8")
                with open(outdir / "objects.inv", "rb") as fh:
                    inv = InventoryFile.loads(fh.read(), uri="")
                inventory = sorted(
                    [objtype, name, item.project_name, item.project_version,
                     item.uri, item.display_name]
                    for objtype, entries in inv.data.items()
                    for name, item in entries.items()
                )
                # config names that feed the .buildinfo config hash
                html_conf = sorted(c.name for c in app.config.filter(frozenset({"html"})))
                sources = {
                    rel: (outdir / rel).read_text(encoding="utf-8")
                    for rel in listing if rel.startswith("_sources/")
                }
            finally:
                app.cleanup()
    finally:
        StandaloneHTMLBuilder.handle_page = orig_handle_page
        shutil.rmtree(base.parent, ignore_errors=True)

    warnings = [normalize(r, spellings) for r in warn.records]
    for text in [json.dumps(pages), json.dumps(full), *warnings]:
        for s in spellings:
            assert s not in text, f"srcdir leaked: {s}"
    return {
        "pages": {k: normalize_obj(v, spellings) for k, v in pages.items()},
        "full": full,
        "listing": listing,
        "buildinfo": buildinfo,
        "html_config_names": html_conf,
        "inventory": inventory,
        "sources": sources,
        "warnings": warnings,
    }


def normalize_obj(obj, spellings):
    if isinstance(obj, str):
        return normalize(obj, spellings)
    if isinstance(obj, list):
        return [normalize_obj(o, spellings) for o in obj]
    if isinstance(obj, dict):
        return {k: normalize_obj(v, spellings) for k, v in obj.items()}
    return obj


VOLATILE = [
    # asset checksums: CRC32 of the output static file (_file_checksum);
    # deterministic, but only equal across implementations once the static
    # files themselves are byte-identical.
    (re.compile(r"\?v=[0-9a-f]{8}"), "?v=<crc32>"),
]


def main():
    conf = {"smartquotes": False, "html_theme": "basic"}
    first = build(FILES, conf)
    second = build(FILES, conf)
    deterministic = json.dumps(first, sort_keys=True) == json.dumps(second, sort_keys=True)
    print(f"deterministic across two builds: {deterministic}")
    if not deterministic:
        for key in first:
            if json.dumps(first[key], sort_keys=True) != json.dumps(second[key], sort_keys=True):
                print(f"  differs: {key}")

    out = first
    print("\n=== output listing (sha256[:16]) ===")
    for rel, digest in out["listing"].items():
        print(f"  {rel}  {digest}")
    print("\n=== .buildinfo ===")
    print(out["buildinfo"], end="")
    print(f"\n=== html-category config names feeding the hash ({len(out['html_config_names'])}) ===")
    print("  " + ", ".join(out["html_config_names"]))
    print("\n=== objects.inv (decoded) ===")
    for row in out["inventory"]:
        print("  " + json.dumps(row))
    print("\n=== warnings (one record per write) ===")
    for w in out["warnings"]:
        print("  " + json.dumps(w))
    for pagename, rec in out["pages"].items():
        print(f"\n=== page {pagename!r} template={rec.get('templatename')} ===")
        print(json.dumps(rec.get("addctx"), indent=2, ensure_ascii=False))
        print("--- ctx (subset) ---")
        print(json.dumps(rec.get("ctx"), indent=2, ensure_ascii=False))
    if "--full" in sys.argv:
        for pagename in ("index", "second"):
            text = out["full"][pagename]
            for rx, repl in VOLATILE:
                text = rx.sub(repl, text)
            print(f"\n=== full page {pagename}.html (VOLATILE-normalized) ===")
            print(text)
    print("\n=== _sources ===")
    for rel, text in out["sources"].items():
        print(f"--- {rel} ({len(text)} chars, == source: {text == FILES[rel[len('_sources/'):-len('.rst.txt')]]})")


if __name__ == "__main__":
    main()
