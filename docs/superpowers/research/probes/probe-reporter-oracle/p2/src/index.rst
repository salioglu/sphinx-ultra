----------

Index
=====

.. unknown-directive:: first

.. toctree::

   nonexistent

`anon`__ and `anon2`__

__ http://example.com

.. literalinclude:: example.py
   :caption: *bad caption

.. code-block:: python
   :caption: `also bad

   x = 1

Dup
---

Dup
---

.. note:: unterminated *here

Text with an ``unterminated literal.

.. [#] footnote never referenced

A ref to [1]_ that does not exist.

.. |sub| replace:: *bad

Use |sub|.

.. table:: Caption *bad

   ===  ===
   a    b
   ===  ===

.. image:: pic.png
   :width: notalength

.. figure:: pic.png

   Caption *bad*
