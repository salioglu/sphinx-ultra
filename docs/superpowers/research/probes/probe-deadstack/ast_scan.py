import os, collections, jinja2, jinja2.sandbox
from jinja2 import nodes
SPHINX='/root/.cache/uv/archive-v0/b4dBDAdEzskuqge1iT52j/lib/python3.12/site-packages/sphinx'
AL=os.path.join(os.path.dirname(SPHINX),'alabaster')
env = jinja2.sandbox.SandboxedEnvironment(extensions=['jinja2.ext.i18n'])
def files(root):
    for d, _, fs in os.walk(root):
        for f in fs:
            if f.endswith(('.html','.xml','.jinja','_t')) and 'changes' not in d:
                yield os.path.join(d, f)
import sys
scope = sys.argv[1]
roots = {'basic':[os.path.join(SPHINX,'themes','basic')], 'alabaster':[AL], 'others':[os.path.join(SPHINX,'themes',t) for t in os.listdir(os.path.join(SPHINX,'themes')) if t not in ('basic',)]}[scope]
filt=collections.Counter(); tests=collections.Counter(); calls=collections.Counter(); meth=collections.Counter(); stmts=collections.Counter(); names=collections.Counter(); ops=collections.Counter()
for r in roots:
  for p in files(r):
    src=open(p).read()
    try: ast=env.parse(src)
    except Exception as e: print('PARSEFAIL',p,e); continue
    for n in ast.find_all(nodes.Node):
        t=type(n).__name__
        if isinstance(n, nodes.Filter): filt[n.name]+=1
        elif isinstance(n, nodes.Test): tests[n.name]+=1
        elif isinstance(n, nodes.Call):
            if isinstance(n.node, nodes.Name): calls[n.node.name + ('(kw)' if n.kwargs else '')]+=1
            elif isinstance(n.node, nodes.Getattr): meth['.'+n.node.attr+'()']+=1
        elif isinstance(n, nodes.Name) and n.ctx=='load': names[n.name]+=1
        elif isinstance(n, (nodes.Stmt,)): stmts[t]+=1
        elif isinstance(n, (nodes.BinExpr, nodes.UnaryExpr, nodes.Compare, nodes.CondExpr, nodes.Concat, nodes.Getitem, nodes.Getattr, nodes.Slice, nodes.List, nodes.Tuple, nodes.Dict)): ops[t]+=1
        if isinstance(n, nodes.Compare):
            for o in n.ops: ops['cmp:'+o.op]+=1
print('STATEMENTS', dict(stmts)); print('FILTERS', dict(filt)); print('TESTS', dict(tests)); print('CALLS', dict(calls)); print('METHODS', dict(meth)); print('EXPRS', dict(ops)); print('NAMES', sorted(names))
