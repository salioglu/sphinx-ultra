import harness, traceback
files = {
 'index.rst': 'Index\n=====\n\n.. toctree::\n\n   a\n',
 'a.rst': 'A\n=\n\n.. toctree::\n\n   b\n',
 'b.rst': 'B\n=\n\n.. toctree::\n\n   a\n',
}
for theme in ('alabaster', 'basic'):
    try:
        r = harness.build(files, conf={'html_theme': theme})
        print('####', theme); print(r['warnings'])
    except BaseException as e:
        print('####', theme, 'CRASH', type(e).__name__, str(e)[:200])
# untitled doc
files2 = {
 'index.rst': 'Index\n=====\n\n.. toctree::\n\n   notitle\n   b\n',
 'notitle.rst': 'just text\n',
 'b.rst': 'B\n=\n\ntext\n',
}
for theme in ('alabaster', 'basic'):
    r = harness.build(files2, conf={'html_theme': theme})
    print('####', theme, 'untitled'); print(r['warnings'])
    print(r['resolved']['index'])
r = harness.build(files2, conf={}, builder='dummy')
print('#### dummy untitled'); print(r['warnings'])
