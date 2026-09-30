import io, tempfile, pathlib
from sphinx.testing.util import SphinxTestApp
from sphinx.util.docutils import docutils_namespace, patch_docutils, _READER_TRANSFORMS
import docutils.parsers.rst
from sphinx.parsers import RSTParser
base = pathlib.Path(tempfile.mkdtemp())/"src"; base.mkdir()
(base/"conf.py").write_text("")
(base/"index.rst").write_text("T\n=\n")
with docutils_namespace(), patch_docutils(str(base)):
    app = SphinxTestApp(buildername="html", srcdir=base, status=io.StringIO(), warning=io.StringIO())
    ts = list(_READER_TRANSFORMS) + list(app.registry.get_transforms()) + list(RSTParser().get_transforms())
    print("READ TRANSFORMS (sorted by priority):")
    for t in sorted(ts, key=lambda t: (t.default_priority, t.__name__)):
        print(f"  {t.default_priority:4d} {t.__module__}.{t.__name__}")
    print("POST TRANSFORMS:")
    for t in sorted(app.registry.get_post_transforms(), key=lambda t: (t.default_priority, t.__name__)):
        print(f"  {t.default_priority:4d} {t.__module__}.{t.__name__}  builders={getattr(t,'builders',())} formats={getattr(t,'formats',())}")
    print("html_compact_lists", app.config.html_compact_lists)
    print("collectors:", [type(c).__name__ for c in app.registry.get_envcollectors()] if hasattr(app.registry,'get_envcollectors') else None)
    print("doctree-read listeners:", [ (l.handler.__qualname__, l.priority) for l in app.events.listeners.get('doctree-read', [])])
    print("env-merge-info listeners:", [ (l.handler.__qualname__, l.priority) for l in app.events.listeners.get('env-merge-info', [])])
    print("doctree-resolved listeners:", [ (l.handler.__qualname__, l.priority) for l in app.events.listeners.get('doctree-resolved', [])])
    print("env-updated listeners:", [ (l.handler.__qualname__, l.priority) for l in app.events.listeners.get('env-updated', [])])
    print("env-get-updated listeners:", [ (l.handler.__qualname__, l.priority) for l in app.events.listeners.get('env-get-updated', [])])
    print("env-purge-doc listeners:", [ (l.handler.__qualname__, l.priority) for l in app.events.listeners.get('env-purge-doc', [])])
    print("missing-reference listeners:", [ (l.handler.__qualname__, l.priority) for l in app.events.listeners.get('missing-reference', [])])
    app.cleanup()
