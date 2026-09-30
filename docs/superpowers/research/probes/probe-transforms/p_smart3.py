import harness
src = '''T
=

See https://example.com/a--b/it's... and "https://x.org".

--long "arg"   Option "desc" -- here.
-s ARG         Short.

.. versionadded:: 1.0 "quoted" -- reason

.. code-block:: python
   :caption: A "caption" -- here

   x
'''
r = harness.build({'index.rst': src})
print(r['resolved']['index'])
print(harness.body(r['html']['index.html']))
