import harness
from sphinx.environment.adapters.toctree import document_toc, global_toctree_for_doc
files = {
 'index.rst': '''Index
=====

.. toctree::
   :caption: Main "Cap"
   :maxdepth: 2
   :numbered:

   a
   sub/b
   self
   https://example.com
   Ext title <https://example.org>
   genindex
   Custom <a>

.. toctree::
   :hidden:

   hidden

Local
-----

Deeper
~~~~~~
''',
 'a.rst': 'A\n=\n\nA1\n--\n\nA11\n~~~\n\nA2\n--\n',
 'sub/b.rst': 'B\n=\n\n.. toctree::\n   :titlesonly:\n\n   c\n\nB1\n--\n',
 'sub/c.rst': 'C\n=\n\nC1\n--\n\n.. only:: html\n\n   C2 html\n   -------\n\n.. only:: latex\n\n   C3 latex\n   --------\n',
 'hidden.rst': 'Hidden\n======\n\nH1\n--\n',
}
def extra(app, out):
    env = app.env
    b = app.builder
    out['doctoc_index'] = document_toc(env, 'index', b.tags).pformat()
    out['doctoc_c'] = document_toc(env, 'sub/c', b.tags).pformat()
    for doc in ('sub/b', 'sub/c', 'hidden'):
        for collapse in (True, False):
            t = global_toctree_for_doc(env, doc, b, tags=b.tags, collapse=collapse, includehidden=False, maxdepth=0)
            out[f'global_{doc}_{collapse}'] = t.pformat() if t is not None else None
    t = global_toctree_for_doc(env, 'sub/b', b, tags=b.tags, collapse=False, includehidden=True, maxdepth=1, titles_only=True)
    out['global_b_hidden_depth1_titlesonly'] = t.pformat()
    out['render_local_toc_index'] = b.render_partial(document_toc(env, 'index', b.tags))['fragment']
    out['render_global_b'] = b._get_local_toctree('sub/b', collapse=True)
    out['tocs_c'] = env.tocs['sub/c'].pformat()
    out['toc_secnumbers'] = env.toc_secnumbers
r = harness.build(files, conf={'smartquotes': False, 'html_theme': 'basic'}, extra=extra)
print(r['warnings'])
for d in ('index', 'sub/b'):
    print('=== resolved', d); print(r['resolved'][d])
for k in [k for k in r if k.startswith(('doctoc', 'global', 'render', 'tocs', 'toc_sec'))]:
    print('===', k); print(r[k])
import re
print('=== index body'); print(harness.body(r['html']['index.html']))
print('=== sub/b sidebar'); m = re.search(r'<div class="sphinxsidebar".*?</div>\s*</div>', r['html']['sub/b.html'], re.S); print(m.group(0) if m else None)
print('=== sub/b related'); m = re.search(r'<div class="related".*?</div>', r['html']['sub/b.html'], re.S); print(m.group(0) if m else None)
