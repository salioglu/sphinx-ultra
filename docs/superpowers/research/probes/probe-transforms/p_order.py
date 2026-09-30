import io, tempfile, pathlib
from sphinx.testing.util import SphinxTestApp
from sphinx.util.docutils import docutils_namespace, patch_docutils
import sphinx.util.docutils as sud
base = pathlib.Path(tempfile.mkdtemp())/"src"; base.mkdir()
(base/"conf.py").write_text("")
(base/"index.rst").write_text("T\n=\n\ntext\n")
orig = sud._parse_str_to_doctree
applied = []
def wrapped(*a, **k):
    doc = orig(*a, **k)
    applied.extend(doc.transformer.applied)
    return doc
with docutils_namespace(), patch_docutils(str(base)):
    app = SphinxTestApp(buildername="html", srcdir=base, status=io.StringIO(), warning=io.StringIO())
    import sphinx.builders
    sphinx.builders._parse_str_to_doctree = wrapped
    app.build()
    for pr, cls, pending, kw in applied:
        print(pr, f"{cls.__module__}.{cls.__name__}")
    # post transforms actual order
    from sphinx.transforms import SphinxTransformer
    doctree = app.env.get_doctree('index')
    t = SphinxTransformer(doctree); t.set_environment(app.env)
    t.add_transforms(app.env._registry.get_post_transforms())
    t.transforms.sort()
    print('POST (sorted order):')
    for pr, cls, pending, kw in t.transforms:
        print(pr, f"{cls.__module__}.{cls.__name__}")
    app.cleanup()
