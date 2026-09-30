import hashlib, io, tempfile, shutil
from pathlib import Path
from sphinx.testing.util import SphinxTestApp
from sphinx.util._serialise import stable_hash
from sphinx.util.docutils import docutils_namespace, patch_docutils
md5 = lambda s: hashlib.md5(s.encode()).hexdigest()
base = Path(tempfile.mkdtemp()).resolve() / "src"; base.mkdir(parents=True)
(base / "conf.py").write_text("project = 'fixture'\nextensions = []\n")
(base / "index.rst").write_text("T\n=\n")
with docutils_namespace(), patch_docutils(str(base)):
    app = SphinxTestApp(buildername="html", srcdir=base, status=io.StringIO(), warning=io.StringIO(),
                        confoverrides={"html_theme": "basic"})
    app.build()
    print("tags:", sorted(app.tags))
    manual_tags = md5(str(sorted(md5(t) for t in sorted(app.tags))))
    print("tags hash (stable_hash):", stable_hash(sorted(app.tags)), "manual:", manual_tags)
    values = {c.name: c.value for c in app.config.filter(frozenset({"html"}))}
    def py_hash(obj):  # hand port of stable_hash, to prove the recipe
        if isinstance(obj, dict):
            obj = sorted(map(py_hash, obj.items()))
        if isinstance(obj, (list, tuple, set, frozenset)):
            obj = sorted(map(py_hash, obj))
        return md5(str(obj))
    print("config hash:", stable_hash(values), "manual:", py_hash(values))
    for k, v in sorted(values.items()):
        print(f"   {k} = {v!r}")
    print((Path(app.outdir) / ".buildinfo").read_text())
    app.cleanup()
shutil.rmtree(base.parent)
