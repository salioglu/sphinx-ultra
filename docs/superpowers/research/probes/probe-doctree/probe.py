import sys, os
from pathlib import Path
from sphinx.application import Sphinx
from sphinx.builders.html import StandaloneHTMLBuilder
out = {}
orig = StandaloneHTMLBuilder.write_doc
def write_doc(self, docname, doctree):
    out[docname] = doctree.pformat()
    return orig(self, docname, doctree)
StandaloneHTMLBuilder.write_doc = write_doc
here = Path(__file__).parent
app = Sphinx(str(here/'src'), str(here/'src'), str(here/'out'), str(here/'dt'), 'html', status=None, warning=sys.stderr, freshenv=True, confoverrides={'keep_warnings': True})
app.build()
for d, t in out.items():
    (here/f'{d}.writer.pformat').write_text(t)
