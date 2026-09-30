import json, os
def on_page(app, pagename, templatename, ctx, doctree):
    if pagename not in ('intro', 'genindex', 'py-modindex', 'search'): return
    out = {}
    for k in sorted(ctx):
        v = ctx[k]
        if callable(v): r = f'<callable {getattr(v, "__name__", type(v).__name__)}>'
        elif k in ('body', 'toc') : r = f'<str len {len(v)}>'
        elif k.startswith('theme_') : continue
        else:
            r = repr(v)
            if len(r) > 300: r = r[:300] + '...'
        out[k] = f'{type(v).__name__}: {r}'
    p = os.path.join(app.outdir, '..', f'ctx-{pagename}.txt')
    with open(p, 'w') as f:
        f.write(f'template={templatename}\n')
        for k, v in out.items(): f.write(f'{k} = {v}\n')
def setup(app):
    app.connect('html-page-context', on_page, priority=950)
    return {'parallel_read_safe': True}
