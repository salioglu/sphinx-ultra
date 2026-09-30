"""Tiny probe harness: build a project with a real Sphinx HTML builder.
usage: import harness; harness.build(files, conf={}, builder='html', show=[docnames]) """
import io, tempfile, pathlib, shutil, sys, re
from sphinx.testing.util import SphinxTestApp
from sphinx.util.docutils import docutils_namespace, patch_docutils
from sphinx.util.console import nocolor
nocolor()

def build(files, conf=None, builder='html', show=None, show_html=None, conf_py='', keep=False, extra=None):
    base = pathlib.Path(tempfile.mkdtemp(prefix='probe_')).resolve() / 'src'
    base.mkdir(parents=True)
    (base / 'conf.py').write_text(conf_py)
    for name, text in files.items():
        p = base / name
        p.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(text, bytes):
            p.write_bytes(text)
        else:
            p.write_text(text)
    resolved = {}
    out = {}
    with docutils_namespace(), patch_docutils(str(base)):
        app = SphinxTestApp(buildername=builder, srcdir=base, status=io.StringIO(), warning=io.StringIO(), confoverrides=dict(conf or {}))
        orig = app.builder.write_doc
        def capture(docname, doctree):
            resolved[docname] = doctree.pformat()
            return orig(docname, doctree)
        app.builder.write_doc = capture
        try:
            app.build()
            out['warnings'] = app.warning.getvalue().replace(str(base), '<src>')
            out['resolved'] = {k: v.replace(str(base), '<src>') for k, v in resolved.items()}
            out['env'] = app.env
            if extra: extra(app, out)
            outdir = app.outdir
            out['html'] = {}
            for p in sorted(pathlib.Path(outdir).rglob('*.html')):
                out['html'][str(p.relative_to(outdir))] = p.read_text()
            out['files'] = sorted(str(p.relative_to(outdir)) for p in pathlib.Path(outdir).rglob('*') if p.is_file())
        finally:
            app.cleanup()
            if not keep:
                shutil.rmtree(base.parent, ignore_errors=True)
    return out

def body(html):
    m = re.search(r'<div class="body" role="main">(.*?)</div>\s*</div>\s*</div>\s*<div class="sphinxsidebar"', html, re.S)
    return m.group(1) if m else html
