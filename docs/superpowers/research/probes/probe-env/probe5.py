import io, shutil, tempfile
from pathlib import Path
from sphinx.util.console import nocolor
nocolor()
from sphinx.testing.util import SphinxTestApp
from sphinx.util.docutils import docutils_namespace, patch_docutils
FILES = {"index": "Index\n=====\n\n.. toctree::\n   :caption: My Caption\n   :name: tocname\n\n   T1 <a>\n   T2 <b>\n   c\n", "a": "A\n=\n", "b": "B\n=\n", "c": "C\n=\n"}
base = Path(tempfile.mkdtemp()).resolve() / "src"; base.mkdir(parents=True)
(base/"conf.py").write_text("project='fixture'\n")
for d, s in FILES.items(): (base / (d + ".rst")).write_text(s)
res = {}
with docutils_namespace(), patch_docutils(str(base)):
    app = SphinxTestApp(buildername="dummy", srcdir=base, status=io.StringIO(), warning=io.StringIO())
    app.builder.write_doc = lambda d, t: res.__setitem__(d, t.pformat())
    app.build()
    print(app.env.tocs['index'].pformat())
    print(res['index'].replace(str(base), '<p>'))
    print("labels", {k: v for k, v in app.env.domaindata['std']['labels'].items() if k == 'tocname'})
    app.cleanup()
shutil.rmtree(base.parent, ignore_errors=True)
