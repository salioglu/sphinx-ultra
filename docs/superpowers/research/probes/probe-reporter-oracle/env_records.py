"""Per-RECORD warning capture for env-corpus projects (vs the fixture's
line-split `warnings`). Usage: python env_records.py inc_basic [more...]"""
import importlib.util, io, json, shutil, sys, tempfile
from pathlib import Path
spec = importlib.util.spec_from_file_location("gef", "/home/user/sphinx-ultra/tools/gen_env_fixture.py")
gef = importlib.util.module_from_spec(spec); spec.loader.exec_module(gef)
from sphinx.testing.util import SphinxTestApp
from sphinx.util.docutils import docutils_namespace, patch_docutils

class Rec(io.StringIO):
    def __init__(self): super().__init__(); self.records = []
    def write(self, s): self.records.append(s); return super().write(s)

def records(entry):
    base = Path(tempfile.mkdtemp(prefix="env_oracle_srcdir_")).resolve() / "src"
    base.mkdir(parents=True)
    (base / "conf.py").write_text(gef.CONF_PY, encoding="utf-8")
    gef.write_project_files(base, entry["files"], entry.get("data_files", {}))
    rec = Rec()
    with docutils_namespace(), patch_docutils(str(base)):
        app = SphinxTestApp(buildername="dummy", srcdir=base, status=io.StringIO(), warning=rec,
                            confoverrides={**gef.BASE_CONFOVERRIDES, **entry.get("conf", {})})
        try:
            app.build()
            try: app.env.collect_relations()
            except RecursionError: pass
            out = [gef.normalize(r, base) for r in rec.records]
        finally:
            app.cleanup(); shutil.rmtree(base.parent, ignore_errors=True)
    return out

names = sys.argv[1:] or [p["name"] for p in gef.PROJECTS]
for p in gef.PROJECTS:
    if p["name"] in names:
        print(f"== {p['name']}")
        for r in records(p):
            print("   " + json.dumps(r))
