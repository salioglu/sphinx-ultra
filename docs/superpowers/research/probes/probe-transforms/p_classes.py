from docutils import nodes
from sphinx import addnodes
import sphinx.addnodes
cls = set()
for mod in (nodes, addnodes):
    for name in dir(mod):
        c = getattr(mod, name)
        if isinstance(c, type) and issubclass(c, nodes.Element) and c.__module__ in ('docutils.nodes', 'sphinx.addnodes'):
            cls.add(c)
rows = []
for c in sorted(cls, key=lambda c: c.__name__):
    flags = []
    for base in ('TextElement','FixedTextElement','Special','Invisible','Inline','Targetable','Titular','PreBibliographic','Referential','Structural','Body','General','Sequential','Admonition','Part','BackLinkable','Labeled','Decorative','Bibliographic','Root'):
        b = getattr(nodes, base, None)
        if b and issubclass(c, b): flags.append(base)
    if issubclass(c, addnodes.not_smartquotable): flags.append('not_smartquotable')
    if getattr(c, 'support_smartquotes', None) is False: flags.append('support_smartquotes=False')
    rows.append(f"{c.__module__.split('.')[0]:8s} {c.__name__:28s} {' '.join(flags)}")
print('\n'.join(rows))
