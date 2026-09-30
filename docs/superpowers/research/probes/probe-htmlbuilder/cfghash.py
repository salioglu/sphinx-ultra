import sys, hashlib
from pathlib import Path
from sphinx.application import Sphinx
from sphinx.util._serialise import stable_hash
src = Path(sys.argv[1]); out = Path(sys.argv[2]); builder = sys.argv[3] if len(sys.argv) > 3 else 'html'
app = Sphinx(src, src, out, out/'.doctrees', builder, status=None, warning=None, freshenv=True)
vals = {c.name: c.value for c in app.config.filter(frozenset({'html'}))}
for k in sorted(vals):
    print(f'{k!r}: {vals[k]!r}  (type {type(vals[k]).__name__})')
print('config hash', stable_hash(vals), 'builder', app.builder.build_info.config_hash)
print('tags', sorted(app.tags), stable_hash(sorted(app.tags)), app.builder.build_info.tags_hash)
# manual reimplementation check
def md5(s): return hashlib.md5(s.encode()).hexdigest()
print('md5 of tags list manual:', md5(str(sorted(md5(md5(t)) for t in sorted(app.tags)))))
print('md5 manual2:', md5(str(sorted(md5(t) for t in sorted(app.tags)))))
# registered config values & their rebuild
from collections import Counter
print(Counter(c.rebuild for c in app.config))
print('extensions', list(app.extensions))
