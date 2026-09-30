import sys
from pathlib import Path
from sphinx.application import Sphinx
here = Path(__file__).parent
app = Sphinx(str(here/'src4'), str(here/'src4'), str(here/'out5'), str(here/'dt5'), 'dummy', status=None, warning=sys.stderr, freshenv=True)
app.build()
print(app.env.get_doctree('index').pformat())
