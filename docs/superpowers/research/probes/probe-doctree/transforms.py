import sys
from pathlib import Path
from sphinx.application import Sphinx
here = Path(__file__).parent
app = Sphinx(str(here/'src'), str(here/'src'), str(here/'out2'), str(here/'dt2'), 'html', status=None, warning=None, freshenv=True)
from sphinx.io import SphinxStandaloneReader
from docutils.readers import standalone
print("== read transforms (registry) ==")
ts = list(app.registry.get_transforms())
import docutils.readers.standalone as st, docutils.parsers.rst as rst
allt = ts + list(standalone.Reader().get_transforms()) + list(rst.Parser().get_transforms())
for t in sorted(set(allt), key=lambda t: (t.default_priority, t.__module__, t.__name__)):
    print(t.default_priority, t.__module__ + '.' + t.__name__)
print("== post transforms ==")
for t in sorted(app.registry.get_post_transforms(), key=lambda t: (t.default_priority, t.__name__)):
    print(t.default_priority, t.__module__ + '.' + t.__name__, getattr(t,'builders',()), getattr(t,'formats',()))
