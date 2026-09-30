import harness, re
files = {
 'index.rst': '''"Top" -- Title
==============

.. toctree::
   :caption: "Cap" -- x

   "Explicit" -- t <a>
   a

'Start' and "*emph*" and "``lit``" end. ``code``'s apostrophe. A "quote with :math:`x`" here.

He said "she said 'hi'" -- ok. x--y and 5'10" and rock 'n' roll. "Hello," she said. 'Twas.

.. rst-class:: language-de

"Deutsch" und 'einfach'.

:doc:`a` and :ref:`sec` and :ref:`"exp" <sec>`.

.. _sec:

"Sec" Title
-----------

.. note:: A "note".

.. figure:: pic.png

   A "caption".

:Field "name": field "body"

term "x"
   def "y"

| line "block"

.. rubric:: "Rubric"

- "item"
''',
 'a.rst': '"A" Doc\n=======\n\ntext\n',
 'pic.png': b'\x89PNG\r\n\x1a\n',
}
for conf in ({}, {'language': 'de'}, {'language': 'fr'}, {'language': 'ja'}, {'smartquotes_action': 'qe'}):
    r = harness.build(files, conf=conf)
    print('####', conf); print(r['warnings'])
    b = harness.body(r['html']['index.html'])
    print(b)
    print(re.search(r'<title>.*?</title>', r['html']['index.html']).group(0))
    print(re.search(r'<title>.*?</title>', r['html']['a.html']).group(0))
    if conf == {}:
        print(r['resolved']['index'])
