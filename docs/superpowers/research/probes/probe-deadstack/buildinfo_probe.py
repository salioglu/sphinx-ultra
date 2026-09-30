import sys, os
from sphinx.application import Sphinx
from sphinx.util._serialise import stable_hash
from sphinx.builders.html._build_info import BuildInfo
app = Sphinx('proj', 'proj', 'out/bi', 'out/bi/.doctrees', 'html', status=None, warning=None, freshenv=True)
vals = {c.name: c.value for c in app.config.filter(frozenset({'html'}))}
for k in sorted(vals): print(repr(k), '=', repr(vals[k]))
print('TAGS', sorted(app.builder.tags))
bi = BuildInfo(app.config, app.builder.tags, frozenset({'html'}))
print('config_hash', bi.config_hash, 'tags_hash', bi.tags_hash)
# show stable_hash structure for one item
print('item example', stable_hash(('html_theme','alabaster')))
import hashlib
md5=lambda s: hashlib.md5(s.encode()).hexdigest()
def h(o):
    if isinstance(o, dict):
        o = sorted(h(kv) for kv in o.items())
    if isinstance(o, (list, tuple, set, frozenset)):
        o = sorted(h(x) for x in o)
    return md5(str(o))
print('manual config_hash', h(vals))
