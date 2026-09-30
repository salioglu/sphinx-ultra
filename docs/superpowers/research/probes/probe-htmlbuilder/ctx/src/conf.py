project = 'Probe'
copyright = '2026, Tester'
author = 'Tester'
release = '1.0'
html_theme = 'basic'

def _dump(app, pagename, templatename, context, doctree):
    import pprint
    with open(app.outdir.parent / 'ctxdump.txt', 'a') as f:
        f.write(f'===== {pagename} template={templatename} event_arg={type(doctree).__name__}\n')
        for k in sorted(context):
            v = context[k]
            if callable(v) and not isinstance(v, (list, dict)):
                r = f'<callable {getattr(v, "__name__", type(v).__name__)}>'
            elif k in ('body',):
                r = repr(v[:80]) + '...'
            else:
                r = repr(v)
                if len(r) > 300: r = r[:300] + '...'
            f.write(f'{k}: {r}\n')

def setup(app):
    app.connect('html-page-context', _dump, priority=900)
