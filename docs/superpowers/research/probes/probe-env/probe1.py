import io, sys, shutil, tempfile, zlib, json
from pathlib import Path
from sphinx.util.console import nocolor
nocolor()
from sphinx.testing.util import SphinxTestApp
from sphinx.util.docutils import docutils_namespace, patch_docutils

FILES = {
 "index": "Index\n=====\n\n.. toctree::\n   :numbered:\n   :caption: Contents\n\n   a\n   sub/b\n   self\n   genindex\n   https://example.com/x\n\nSee :doc:`a` and :ref:`sec-sub` and :ref:`genindex`.\n\n.. index:: single: Alpha\n",
 "a": ":tocdepth: 1\n\nA\n=\n\n.. _sec-sub:\n\nSub\n---\n\nText. :ref:`back <sec-sub>` :doc:`index`\n\nDeep\n~~~~\n\n.. envvar:: HOME\n\n.. py:module:: pkg\n   :synopsis: A package.\n\n.. py:function:: f(x)\n\n.. index:: pair: bread; butter\n",
 "sub/b": "B\n=\n\nLink to :doc:`../a` and :ref:`sec-sub`.\n\n.. figure:: pic.png\n\n   Caption.\n\n.. toctree::\n\n   c\n",
 "sub/c": "C\n=\n\nC text.\n",
 "orph": ":orphan:\n\nNo section here, just text.\n",
}
base = Path(tempfile.mkdtemp()).resolve() / "src"
base.mkdir(parents=True)
(base/"conf.py").write_text("project='fixture'\nversion='1.0'\nrelease='1.0.0'\nexclude_patterns=['_build']\n")
for d, s in FILES.items():
    p = base / (d + ".rst"); p.parent.mkdir(parents=True, exist_ok=True); p.write_text(s)
(base/"sub"/"pic.png").write_bytes(b"\x89PNG\r\n\x1a\n")
resolved = {}
with docutils_namespace(), patch_docutils(str(base)):
    app = SphinxTestApp(buildername=sys.argv[1] if len(sys.argv)>1 else "html", srcdir=base, status=io.StringIO(), warning=io.StringIO(), confoverrides={})
    orig = app.builder.write_doc
    def cap(docname, doctree):
        resolved[docname] = doctree.pformat()
        return orig(docname, doctree)
    app.builder.write_doc = cap
    app.build()
    out = Path(app.outdir)
    print("WARNINGS:\n" + app.warning.getvalue().replace(str(base), "<p>"))
    for d in sorted(resolved):
        print("=== resolved", d); print(resolved[d].replace(str(base), "<p>"))
    env = app.env
    print("titles", {k: v.pformat() for k,v in env.titles.items()})
    print("longtitles same obj:", {k: env.titles[k] is env.longtitles[k] for k in env.titles})
    print("metadata", dict(env.metadata))
    print("relations", env.collect_relations())
    if (out/"objects.inv").exists():
        raw = (out/"objects.inv").read_bytes()
        head = raw.split(b"\n", 4)
        print("INV HEADER", head[:4]); print(zlib.decompress(head[4]).decode())
    for f in sorted(out.rglob("*")):
        if f.is_file() and "_static" not in f.parts: print("OUT", f.relative_to(out))
    for name in ["genindex.html", "py-modindex.html", "sub/c.html"]:
        if (out/name).exists():
            print("=====HTML", name); print((out/name).read_text())
    if (out/".buildinfo").exists(): print("BUILDINFO", (out/".buildinfo").read_text())
    app.cleanup()
shutil.rmtree(base.parent, ignore_errors=True)
