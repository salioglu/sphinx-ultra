import os, minijinja, html
T='/home/user/sphinx-ultra/templates'
templates = {f: open(os.path.join(T,f)).read() for f in os.listdir(T) if f.endswith(('.html','.xml'))}
def pathto(target, resource=False):
    return f"_static/{target}" if resource else f"{target}.html"
env = minijinja.Environment(templates=templates, pycompat=False,
    auto_escape_callback=lambda n: 'html' if n.endswith(('.html','.xml')) else None,
    globals={'pathto': pathto, 'css_tag': lambda c: f'<link rel="stylesheet" href="{c}" type="text/css" />',
             'js_tag': lambda j: f'<script src="{j}"></script>', 'toctree': lambda *a, **k: '<div class="toctree-wrapper"></div>'},
    filters={'e': lambda s: html.escape(str(s), quote=False) if isinstance(s, str) else s})
ctx = dict(title='T', body='<p>hi &amp; bye</p>', docstitle='D', language='en', css_files=['_static/pygments.css'], script_files=['_static/doctools.js'],
           parents=[], display_toc=True, toc='<ul></ul>', show_source=True, has_source=True, sourcename='index.rst.txt', show_copyright=True, copyright='2026', show_sphinx=True, sphinx_version='0.5.0',
           genindexentries=[('A', [('alpha', [[], [], None])])], genindexcounts=[1], split_index=False, key='A', entries=[], count=0,
           indextitle='Python Module Index', content=[], collapse_index=False, version='1', file_suffix='.html', project='P')
for name in ['page.html', 'genindex.html', 'genindex-single.html', 'genindex-split.html', 'domainindex.html', 'search.html', 'opensearch.xml']:
    try:
        out = env.render_template(name, **ctx)
        body_line = [l for l in out.splitlines() if 'hi' in l]
        print(f'{name}: OK len={len(out)} body_line={body_line[:1]}')
    except minijinja.TemplateError as e:
        print(f'{name}: ERROR {e.kind}: {e.message}')
