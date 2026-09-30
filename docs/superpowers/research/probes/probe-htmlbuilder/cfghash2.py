import sys
from pathlib import Path
from sphinx.application import Sphinx
from sphinx.util._serialise import stable_hash
import copy
src = Path(sys.argv[1]); out = Path(sys.argv[2])
import sphinx.builders.html as H
orig_init = H.StandaloneHTMLBuilder.create_build_info
def patched(self):
    vals = {c.name: copy.deepcopy(c.value) for c in self.config.filter(frozenset({'html'}))}
    print('at create_build_info: css', vals['html_css_files'], 'js', vals['html_js_files'])
    print('hash', stable_hash(vals))
    return orig_init(self)
H.StandaloneHTMLBuilder.create_build_info = patched
app = Sphinx(src, src, out, out/'.doctrees', 'html', status=None, warning=None, freshenv=True)
print('final', app.builder.build_info.config_hash)
