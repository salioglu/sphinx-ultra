B
=

.. include:: lit_part.inc
   :literal:
   :name: lit-block

.. include:: numbered.inc
   :literal:
   :number-lines:

.. include:: code_plain.inc
   :code:

.. include:: code_numbered.inc
   :code:
   :number-lines:

.. literalinclude:: example.py
   :pyobject: Foo.method
   :lineno-match:

.. literalinclude:: example.py
   :lines: 6-8
   :emphasize-lines: 2
   :caption: Example tail
