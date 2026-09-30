project = "Probe2"
html_compact_lists = False
html_theme = 'basic'
extensions = []
keep_warnings = True
import json, os
def _dump(app, pagename, templatename, ctx, doctree):
    if doctree is None: return
    d = os.path.join(app.outdir, '..', 'bodies')
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, pagename.replace('/', '__') + '.html'), 'w') as f:
        f.write(ctx.get('body', ''))
    with open(os.path.join(d, pagename.replace('/', '__') + '.meta'), 'w') as f:
        f.write(ctx.get('metatags', ''))
def setup(app):
    app.connect('html-page-context', _dump)
