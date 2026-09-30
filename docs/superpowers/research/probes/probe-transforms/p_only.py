import harness
files = {
 'index.rst': 'Index\n=====\n\n.. only:: latex\n\n   :ref:`nope-in-latex`\n\n   .. toctree::\n\n      a\n\n.. only:: html\n\n   .. toctree::\n\n      b\n\n:ref:`nope-outside`\n',
 'a.rst': 'A\n=\n',
 'b.rst': 'B\n=\n',
}
r = harness.build(files, conf={'smartquotes': False})
print(r['warnings']); print(r['resolved']['index'])
print(r['env'].tocs['index'].pformat())
print(r['env'].collect_relations())
