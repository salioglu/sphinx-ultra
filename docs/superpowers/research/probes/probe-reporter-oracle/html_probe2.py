import json, base64
import html_probe as hp
PNG = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")
files = {"index": """\
Top
===

Shell example::

   $ ls -la | grep "x" && echo <ok>

.. code-block:: none

   raw <text> & stuff

.. figure:: pic.png
   :alt: a pic

   The caption.
"""}
import tempfile, pathlib
orig_build = hp.build
def build_with_png(files, conf):
    # write the png beside the sources by patching write_text path: simplest is a data file hook
    import builtins
    return orig_build(files, conf)
# monkeypatch: add png after conf.py is written
_orig_write = pathlib.Path.write_text
def write_text(self, data, *a, **k):
    r = _orig_write(self, data, *a, **k)
    if self.name == "conf.py":
        (self.parent / "pic.png").write_bytes(PNG)
    return r
pathlib.Path.write_text = write_text
for hl in (None, "none"):
    conf = {"smartquotes": False, "html_theme": "basic"}
    if hl: conf["highlight_language"] = hl
    out = hp.build(files, conf)
    print(f"##### highlight_language={hl!r}")
    print(out["pages"]["index"]["addctx"]["body"])
    print("images:", [k for k in out["listing"] if k.startswith("_images")])
    print("warnings:", out["warnings"])
