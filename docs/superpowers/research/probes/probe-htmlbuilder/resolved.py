import sys
from pathlib import Path
from sphinx.application import Sphinx
src = Path(sys.argv[1]); out = Path(sys.argv[2]); b = sys.argv[3]
app = Sphinx(src, src, out, out/'.doctrees', b, status=None, warning=None, freshenv=True)
cap = {}
orig = app.builder.write_doc
def wd(docname, doctree):
    cap[docname] = doctree.pformat()
    return orig(docname, doctree)
app.builder.write_doc = wd
app.build()
import re
for d in sorted(cap):
    for line in cap[d].splitlines():
        if 'refuri' in line or 'toctree' in line or 'compact' in line:
            print(b, d, line.strip())
