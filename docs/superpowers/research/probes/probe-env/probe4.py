import io, sys, shutil, tempfile
from pathlib import Path
from sphinx.util.console import nocolor
nocolor()
from sphinx.testing.util import SphinxTestApp
from sphinx.util.docutils import docutils_namespace, patch_docutils
FILES = {
 "index": "Index\n=====\n\n.. toctree::\n   :titlesonly:\n\n   Custom Title <a>\n   b\n\n.. toctree::\n   :maxdepth: 1\n\n   c\n\nSub of index\n------------\n\n.. only:: html\n\n   .. toctree::\n\n      d\n",
 "a": "A\n=\n\nA1\n--\n\n.. toctree::\n\n   a_child\n",
 "a_child": "Achild\n======\n\nx\n",
 "b": "B\n=\n\nB1\n--\n\nB11\n~~~\n",
 "c": "C\n=\n\nC1\n--\n",
 "d": "D\n=\n\n.. only:: html\n\n   D-html-sub\n   ----------\n",
}
base = Path(tempfile.mkdtemp()).resolve() / "src"; base.mkdir(parents=True)
(base/"conf.py").write_text("project='fixture'\nexclude_patterns=['_build']\n")
for d, s in FILES.items():
    p = base / (d + ".rst"); p.write_text(s)
resolved = {}
with docutils_namespace(), patch_docutils(str(base)):
    app = SphinxTestApp(buildername=sys.argv[1], srcdir=base, status=io.StringIO(), warning=io.StringIO())
    orig = app.builder.write_doc
    def cap(d, t):
        resolved[d] = t.pformat()
        if sys.argv[1] != 'dummy': orig(d, t)
    app.builder.write_doc = cap
    app.build()
    print("WARNINGS:\n" + app.warning.getvalue().replace(str(base), "<p>"))
    print("TOC d:", app.env.tocs['d'].pformat())
    print("TOC index:", app.env.tocs['index'].pformat())
    for d in ("index", "a"): print(resolved[d].replace(str(base), "<p>"))
    from sphinx.environment.adapters.toctree import global_toctree_for_doc, document_toc
    g = global_toctree_for_doc(app.env, 'b', app.builder, tags=app.builder.tags, collapse=True, includehidden=False)
    print("GLOBAL for b (collapse):", g.pformat() if g is not None else None)
    print("LOCAL for b:", document_toc(app.env, 'b', app.builder.tags).pformat())
    app.cleanup()
shutil.rmtree(base.parent, ignore_errors=True)
