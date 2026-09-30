import harness
files = {
 'index.rst': 'Index\n=====\n\n.. toctree::\n\n   a\n   b\n\nSee :eq:`e1`, :eq:`e2`, :eq:`nope`, :numref:`e1`.\n',
 'a.rst': 'A\n=\n\n.. math::\n   :label: e1\n\n   x\n\n.. math::\n   :label:\n\n   auto\n\n.. math:: y\n   :name: e2\n\nRef :eq:`e1`.\n',
 'b.rst': 'B\n=\n\n.. math::\n   :label: e1\n\n   dup\n',
}
for conf in ({'smartquotes': False}, {'smartquotes': False, 'numfig': True}):
    r = harness.build(files, conf=conf)
    print('####', conf); print(r['warnings'])
    for d in ('index', 'a'): print(r['resolved'][d])
    print(harness.body(r['html']['a.html']))
    print(harness.body(r['html']['index.html']))
    print(r['env'].domains['math'].data)
