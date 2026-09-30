import harness
src = 'T\n=\n\nPara.\n\n.. image:: missing1.png\n\n- item\n\n  .. image:: missing2.png\n\n.. figure:: missing3.png\n\n   cap\n\nText |sub| here.\n\n.. |sub| image:: missing4.png\n\nS\n-\n\n.. image:: sub/*.png\n\n.. image:: https://x.org/a.png\n\n.. image:: pic.*\n'
files = {'index.rst': src, 'sub/one.png': b'\x89PNG\r\n\x1a\n', 'pic.png': b'\x89PNG\r\n\x1a\n', 'pic.svg': b'<svg xmlns="http://www.w3.org/2000/svg"/>', 'doc/x.rst': 'X\n=\n\n.. image:: ../pic.png\n.. image:: /pic.png\n.. image:: pic.png\n'}
r = harness.build(files, conf={'smartquotes': False})
print(r['warnings']); print(r['resolved']['index']); print(r['resolved']['doc/x'])
print(harness.body(r['html']['index.html'])); print(harness.body(r['html']['doc/x.html']))
print(dict(r['env'].images))
print([f for f in r['files'] if f.startswith('_images')])
