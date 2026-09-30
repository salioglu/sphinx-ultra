import harness
src = '''T
=

Literal::

   plain

.. highlight:: python
   :linenothreshold: 3

::

   a
   b

::

   a
   b
   c

.. code-block::

   x

.. parsed-literal::

   *p*

.. code-block:: pycon

   >>> f()  # doctest: +SKIP
   <BLANKLINE>

>>> g()  # doctest: +ELLIPSIS
...

.. highlight:: c
   :force:

::

   int x;

.. math::

   x
'''
for conf in ({'smartquotes': False}, {'smartquotes': False, 'highlight_language': 'rst', 'trim_doctest_flags': False}):
    r = harness.build({'index.rst': src}, conf=conf)
    print('#####', conf); print(r['warnings']); print(r['resolved']['index'])
    print(harness.body(r['html']['index.html'])[:3000])
