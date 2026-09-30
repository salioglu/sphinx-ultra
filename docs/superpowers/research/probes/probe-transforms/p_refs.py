import harness, sys
cases = {
'subst': '''T
=

A |name| and |Name| and |undef| and |today| and |version| and |release|.

.. |name| replace:: *replaced* text
.. |img| image:: pic.png
.. |trim| replace:: X
   :trim:

Left |trim| right.
''',
'hyperlinks': '''T
=

Named `ext`_, anonymous `anon`__, indirect `ind`_, internal `sec`_, `inline <https://x.org>`_, alias `al <ext_>`_, unknown `nope`_, dup `d`_.

.. _ext: https://example.com
__ https://anon.example
.. _ind: ext_
.. _d: https://1
.. _d: https://2

Sec
---

text
''',
'footnotes': '''T
=

Auto [#]_, labeled [#lab]_, symbol [*]_, manual [1]_, again [#lab]_, unref.

.. [#] auto note
.. [#lab] labeled note
.. [*] symbol note
.. [1] manual note
.. [2] unreferenced manual
.. [#] unreferenced auto
''',
'transitions': '''T
=

para

----------

para2

S
-

x

----------
''',
'docinfo': ''':author: Me
:orphan:
:tocdepth: 1
:custom: value

T
=

Body.
''',
'docinfo_after_title': '''T
=

:orphan:
:author: Me

Body.
''',
'doctest': '''T
=

>>> 1 + 1
2

    >>> quoted
    x

Text.
''',
'targets': '''T
=

.. _a:
.. _b:

Para after two targets.

.. _c:

.. index:: single: foo

Para after index.

.. index:: single: bar
.. _d:

Para2.

.. _e:

.. note:: note body

.. _f:

.. figure:: pic.png

   caption

.. _g:

S2
--

text
''',
'autonumber': '''T
=

.. code-block:: python
   :caption: cap one

   x = 1

.. figure:: pic.png

   fig caption

.. table:: tab caption

   = =
   a b
   = =

.. list-table:: lt caption

   * - a
''',
'sortids': '''T
=

.. _explicit:

A
-

.. _x:

B
-

text

Dup
---

Dup
---
''',
'modtargets': '''T
=

Mod section
-----------

.. py:module:: mymod

text

.. py:module:: other

more
''',
'only': '''T
=

.. only:: html

   html only

.. only:: latex

   latex only

.. _lbl:

.. only:: bogus and

   x
''',
}
which = sys.argv[1:] or list(cases)
for name in which:
    files = {'index.rst': cases[name], 'pic.png': b'\x89PNG\r\n\x1a\n'}
    r = harness.build(files, conf={'smartquotes': False, 'version': '1.0', 'release': '1.0.1', 'today': 'TODAY'})
    print('########', name)
    print(r['warnings'])
    print(r['resolved']['index'])
    print(harness.body(r['html']['index.html']))
    print('metadata:', dict(r['env'].metadata['index']))
