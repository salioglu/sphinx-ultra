import harness
src = '''"Quoted" Title's -- here
===========================

He said "hello" and 'bye'. It's the '80s -- or 1990--2000 --- maybe... ok. . . done.

Escaped \\"quote\\" and \\-- and \\... and \\'x\\'.

``"literal"`` and :code:`"code"` and :option:`--flag "x"` and :samp:`"s" {x}`.

A `"link" <https://example.com>`_ and :ref:`"lbl" <tgt>` and *"emph"*, **'strong'**.

.. _tgt:

Sub "section"
-------------

.. py:function:: f(a="x", b='y') -> "ret"

   Doc "string" -- ok.

.. glossary::

   "term"
      "definition"

:kbd:`"C"` and :menuselection:`"A" --> "B"` and :guilabel:`"&G"`.

Option list:

--opt "x"  desc "y"

.. code-block:: python

   x = "y"

Footnote [#f]_ and citation [CIT]_.

.. [#f] "note"
.. [CIT] "cite"

Math :math:`"m"`.

|sub|

.. |sub| replace:: "subst" -- text
'''
for sq in (True, False):
    r = harness.build({'index.rst': src}, conf={'smartquotes': sq} if not sq else {})
    print('#### smartquotes', sq)
    print(r['warnings'])
    print(r['resolved']['index'])
    print(repr(r['env'].titles['index'].astext()))
    print(r['env'].tocs['index'].pformat())
    print(r["env"].domains.standard_domain.labels.get("tgt"))
    print(harness.body(r['html']['index.html']))
    import re
    print(re.search(r'<title>.*?</title>', r['html']['index.html'], re.S).group(0))
