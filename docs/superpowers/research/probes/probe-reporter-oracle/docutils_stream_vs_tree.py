"""Stream-vs-tree over tools/gen_doctree_fixture.py's 735 docutils cases
(parse layer only, report_level=1 like that fixture)."""
import importlib.util, io, sys
from pathlib import Path
from docutils import nodes
from docutils.frontend import get_default_settings
from docutils.parsers.rst import Parser
from docutils.utils import new_document

spec = importlib.util.spec_from_file_location("gdf", "/home/user/sphinx-ultra/tools/gen_doctree_fixture.py")
gdf = importlib.util.module_from_spec(spec); spec.loader.exec_module(gdf)

class Rec(io.StringIO):
    def __init__(self): super().__init__(); self.records = []
    def write(self, s): self.records.append(s); return super().write(s)

def run(text):
    settings = get_default_settings(Parser)
    settings.report_level = 1; settings.halt_level = 5
    rec = Rec(); settings.warning_stream = rec
    settings.auto_id_prefix = "id"; settings.id_prefix = ""
    doc = new_document("<snippet>", settings)
    Parser().parse(text, doc)
    return rec.records, doc

stats = dict(cases=0, with_msgs=0, literal_only=0, order=0, set_diff=0)
examples = {"order": [], "set_diff": []}
for family, name, text in gdf.CASES:
    stats["cases"] += 1
    stream, doc = run(text)
    tree_nodes = list(doc.findall(nodes.system_message))
    tree = [m.astext() + "\n" for m in tree_nodes]
    if not stream and not tree: continue
    stats["with_msgs"] += 1
    if stream == tree: continue
    # compare on first line (the "(TYPE/N) text" header line) to ignore literal children
    head = lambda r: r.split("\n", 1)[0]
    s_heads = [head(r) for r in stream]; t_heads = [head(r) for r in tree]
    if s_heads == t_heads:
        stats["literal_only"] += 1
    elif sorted(s_heads) == sorted(t_heads):
        stats["order"] += 1; examples["order"].append((f"{family}.{name}", text, s_heads, t_heads))
    else:
        stats["set_diff"] += 1; examples["set_diff"].append((f"{family}.{name}", text, s_heads, t_heads))
print(stats)
for kind in ("order", "set_diff"):
    print(f"\n##### {kind}: {len(examples[kind])}")
    for n, text, s, t in examples[kind]:
        print(f"\n--- {n}\nRST: {text!r}\nSTREAM: " + "\n        ".join(s) + "\nTREE:   " + "\n        ".join(t))
