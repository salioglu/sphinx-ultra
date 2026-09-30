import sys
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
app = Sphinx(str(here/'src2'), str(here/'src2'), str(here/'out3'), str(here/'dt3'), 'html', status=None, warning=sys.stderr, freshenv=True, confoverrides={'keep_warnings': True})
app.build()
# also dump read-phase pickled doctree
import pickle
env = app.env
print("=== READ-PHASE (pickled) ===")
print(env.get_doctree('index').pformat())
print("=== WRITER-TIME ===")
print(out['index'])
