import harness
src = '''T
=

.. _lbl:

Identity
--------

.. _lbl2:

日本
----

See :ref:`lbl` and :ref:`lbl2`.
'''
r = harness.build({'index.rst': src}, conf={'smartquotes': False})
print(r['warnings']); print(r['resolved']['index']); print(r['env'].tocs['index'].pformat())
print(harness.body(r['html']['index.html']))
print(r['env'].domains.standard_domain.labels)
