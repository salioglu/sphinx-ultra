import io, sys, shutil, tempfile
from pathlib import Path
from sphinx.util.console import nocolor
nocolor()
from sphinx.testing.util import SphinxTestApp
from sphinx.util.docutils import docutils_namespace, patch_docutils
FILES = {
 "index": "Index\n=====\n\n.. toctree::\n   :maxdepth: 1\n\n   notitle\n   a\n\n.. toctree::\n   :hidden:\n\n   h\n",
 "notitle": "Just text, no title.\n",
 "a": "A\n=\n\nS1\n--\n\nx\n",
 "h": "Hidden\n======\n\nS2\n--\n\nx\n",
}
theme = sys.argv[2] if len(sys.argv) > 2 else None
base = Path(tempfile.mkdtemp()).resolve() / "src"; base.mkdir(parents=True)
conf = "project='fixture'\nexclude_patterns=['_build']\n"
if theme: conf += f"html_theme={theme!r}\n"
(base/"conf.py").write_text(conf)
for d, s in FILES.items():
    p = base / (d + ".rst"); p.parent.mkdir(parents=True, exist_ok=True); p.write_text(s)
resolved = {}
with docutils_namespace(), patch_docutils(str(base)):
    app = SphinxTestApp(buildername=sys.argv[1], srcdir=base, status=io.StringIO(), warning=io.StringIO())
    orig = app.builder.write_doc
    def cap(docname, doctree):
        resolved[docname] = doctree.pformat(); return orig(docname, doctree)
    app.builder.write_doc = cap
    app.build()
    print("THEME", app.config.html_theme if hasattr(app.config,'html_theme') else None)
    print("WARNINGS:\n" + app.warning.getvalue().replace(str(base), "<p>"))
    print(resolved.get("index","").replace(str(base), "<p>"))
    out = Path(app.outdir)
    if (out/"a.html").exists():
        t=(out/"a.html").read_text(); i=t.find('sphinxsidebarwrapper'); print(t[i-100:i+2500])
    app.cleanup()
shutil.rmtree(base.parent, ignore_errors=True)
