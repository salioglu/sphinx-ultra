import io, sys, shutil, tempfile
from pathlib import Path
from sphinx.util.console import nocolor
nocolor()
from sphinx.testing.util import SphinxTestApp
from sphinx.util.docutils import docutils_namespace, patch_docutils
FILES = {
 "index": "Index\n=====\n\n.. toctree::\n\n   a\n   b\n",
 "a": ":tocdepth: abc\n:author: Jane Doe\n:authors: A; B\n:version: 1.2\n:custom field: *x*\n:nocomments:\n\nA\n=\n\nx\n",
 "b": "B\n=\n\n:orphan:\n\ntext\n",
 "c": "C\n=\n\n:orphan:\n\ntext\n",
 "d": ".. comment\n\n:orphan:\n:tocdepth: 2\n\nD\n=\n",
}
base = Path(tempfile.mkdtemp()).resolve() / "src"; base.mkdir(parents=True)
(base/"conf.py").write_text("project='fixture'\nexclude_patterns=['_build']\n")
for d, s in FILES.items():
    p = base / (d + ".rst"); p.write_text(s)
resolved = {}
with docutils_namespace(), patch_docutils(str(base)):
    app = SphinxTestApp(buildername="dummy", srcdir=base, status=io.StringIO(), warning=io.StringIO())
    app.builder.write_doc = lambda d, t: resolved.__setitem__(d, t.pformat())
    app.build()
    print("WARNINGS:\n" + app.warning.getvalue().replace(str(base), "<p>"))
    print("metadata", {k: dict(v) for k, v in app.env.metadata.items()})
    for d in ("a", "c", "d"): print(resolved[d].replace(str(base), "<p>"))
    app.cleanup()
shutil.rmtree(base.parent, ignore_errors=True)
