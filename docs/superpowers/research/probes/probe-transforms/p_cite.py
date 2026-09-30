import harness
files = {
 'index.rst': 'Index\n=====\n\n.. toctree::\n\n   a\n   b\n\nSee [CIT]_ and [Missing]_ and [dup]_.\n\n.. [dup] first dup\n',
 'a.rst': 'A\n=\n\n.. [CIT] The citation.\n\n.. [Unref] Never referenced.\n\n.. [dup] second dup\n\nlocal [CIT]_\n',
 'b.rst': 'B\n=\n\ntext\n',
}
r = harness.build(files, conf={'smartquotes': False})
print(r['warnings'])
for d in ('index', 'a'): print(r['resolved'][d])
print(harness.body(r['html']['index.html'])); print(harness.body(r['html']['a.html']))
