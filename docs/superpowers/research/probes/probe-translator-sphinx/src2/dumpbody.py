import os
def dump(app, pagename, templatename, context, doctree):
    if 'body' in context:
        d = os.path.join(app.outdir, '..', '_dump')
        os.makedirs(os.path.join(d, os.path.dirname(pagename)), exist_ok=True)
        with open(os.path.join(d, pagename + '.body'), 'w', encoding='utf-8') as f:
            f.write(context['body'])
def setup(app):
    app.connect('html-page-context', dump)
    return {'parallel_read_safe': True}
