import inspect
from docutils import nodes
from sphinx import addnodes
te, seq, fixed = [], [], []
for mod in (nodes, addnodes):
    for n, c in inspect.getmembers(mod, inspect.isclass):
        if issubclass(c, nodes.Element) and c.__module__ == mod.__name__ and n[0].islower():
            if issubclass(c, nodes.TextElement): te.append(n)
            if issubclass(c, nodes.Sequential): seq.append(n)
print('TextElement:', ' '.join(sorted(te)))
print('Sequential:', ' '.join(sorted(seq)))
from sphinx.writers.html5 import HTML5Translator
print('Admonition subclasses:', ' '.join(sorted(n for mod in (nodes, addnodes) for n,c in inspect.getmembers(mod, inspect.isclass) if issubclass(c, nodes.Admonition) and n[0].islower())))
