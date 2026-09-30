"""Experiment: is a post-parse doctree walk enough to reproduce Sphinx's
reporter (docutils system_message) warning stream?

For every case in tools/gen_sphinx_fixture.py's corpus, parse it through the
exact fixture harness (keep_warnings=True, so level>=2 messages stay in-tree)
and compare:

  STREAM: the records Sphinx's WarningStreamHandler actually emitted during
          the parse (one handler write == one record), in emission order.
  TREE:   level>=2 system_message nodes found by `doctree.findall`, rendered
          the way LoggingReporter/WarningStream would render them.

Reports cases where the [docutils] records differ in multiset or in order,
and cases where logger (non-docutils) records interleave with them.

Run:
  PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' \
      --with 'docutils==0.22.4' python stream_vs_tree.py
"""

import importlib.util
import io
import json
import shutil
import sys
import tempfile
from pathlib import Path

REPO = Path("/home/user/sphinx-ultra")
spec = importlib.util.spec_from_file_location(
    "gsf", REPO / "tools" / "gen_sphinx_fixture.py"
)
gsf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gsf)

from docutils import nodes  # noqa: E402
from sphinx.util.docutils import docutils_namespace, patch_docutils  # noqa: E402

PREFIX = {2: "WARNING: ", 3: "ERROR: ", 4: "CRITICAL: "}


class RecordingIO(io.StringIO):
    """StreamHandler.emit does ONE write per record (msg + terminator)."""

    def __init__(self):
        super().__init__()
        self.records = []

    def write(self, s):
        self.records.append(s)
        return super().write(s)


def render_tree_message(msg):
    # docutils system_message.astext(): "%s:%s: (%s/%s) %s" -- then
    # WarningStream strips the "(TYPE/N) " prefix and rstrips.
    body = nodes.Element.astext(msg).rstrip()
    line = msg.get("line", "")
    return f"{msg['source']}:{line}: {PREFIX[msg['level']]}{body} [docutils]\n"


def main():
    base = Path(tempfile.mkdtemp(prefix="svt_")).resolve() / "src"
    groups = {}
    for case in gsf.CASES:
        family, name, rst, conf = gsf.case_parts(case)
        groups.setdefault(json.dumps(conf, sort_keys=True), (conf, []))[1].append(
            (family, name, rst)
        )

    stats = {"cases": 0, "with_docutils_records": 0, "order_or_set_diff": 0,
             "interleaved_with_logger": 0, "logger_only": 0}
    diffs = []
    interleaved = []
    for key in sorted(groups):
        conf, cases = groups[key]
        with docutils_namespace(), patch_docutils(str(base)):
            app = gsf.make_app(base, conf)
            warn = RecordingIO()
            # re-point the warning handler at our recording stream
            import logging
            from sphinx.util.logging import WarningStreamHandler, SafeEncodingWriter
            for h in logging.getLogger("sphinx").handlers:
                if isinstance(h, WarningStreamHandler):
                    h.setStream(SafeEncodingWriter(warn))
            try:
                for family, name, rst in cases:
                    stats["cases"] += 1
                    warn.records.clear()
                    doctree = gsf.probe(app, base, rst)
                    stream = [r.replace(str(base / "index.rst"), "<snippet>")
                              for r in warn.records]
                    tree = [render_tree_message(m).replace(str(base / "index.rst"), "<snippet>")
                            for m in doctree.findall(nodes.system_message)
                            if m["level"] >= 2]
                    dstream = [r for r in stream if r.rstrip().endswith("[docutils]")]
                    if dstream:
                        stats["with_docutils_records"] += 1
                    if len(dstream) != len(stream):
                        kinds = ["D" if r.rstrip().endswith("[docutils]") else "L" for r in stream]
                        if "D" in kinds:
                            stats["interleaved_with_logger"] += 1
                            interleaved.append((f"{family}.{name}", "".join(kinds), stream))
                        else:
                            stats["logger_only"] += 1
                    if dstream != tree:
                        stats["order_or_set_diff"] += 1
                        diffs.append((f"{family}.{name}", rst, dstream, tree))
            finally:
                app.cleanup()
        shutil.rmtree(base.parent, ignore_errors=True)
        base = Path(tempfile.mkdtemp(prefix="svt_")).resolve() / "src"

    print(json.dumps(stats, indent=2))
    print(f"\n=== {len(diffs)} cases where STREAM != TREE ===")
    for name, rst, s, t in diffs:
        print(f"\n--- {name}\nRST:\n{rst}STREAM:")
        for r in s:
            print("   " + r.rstrip().replace("\n", "\n   "))
        print("TREE:")
        for r in t:
            print("   " + r.rstrip().replace("\n", "\n   "))
    print(f"\n=== {len(interleaved)} cases mixing logger + docutils records ===")
    for name, kinds, s in interleaved:
        print(f"\n--- {name} order={kinds}")
        for r in s:
            print("   " + r.rstrip().replace("\n", "\n   "))


if __name__ == "__main__":
    sys.exit(main())
