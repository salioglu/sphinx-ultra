import sys
from pathlib import Path
from sphinx.application import Sphinx
here = Path(__file__).parent
app = Sphinx(str(here/'src3'), str(here/'src3'), str(here/'out4'), str(here/'dt4'), 'dummy', status=None, warning=sys.stderr, freshenv=True, confoverrides={'keep_warnings': True})
app.build()
print(app.env.get_doctree('index').pformat())
