Top
===

.. toctree::

   sub/page

Refs [#a]_ and again [#a]_ and cite [C1]_ and [C1]_.

.. [#a] Multi-ref footnote.
.. [#b] Unreferenced footnote.

.. [C1] Cited twice.

Title *with* ``markup`` and `link <https://x.y>`_
-------------------------------------------------

.. code-block:: python
   :name: named-block
   :class: extra-cls

   x = 1

* .. code-block:: text

     in list

* item

+---------------------+
| .. code-block:: text|
|                     |
|    in cell          |
+---------------------+

.. table:: No numfig caption
   :align: left
   :width: 50%

   ===  ===
   a    b
   ===  ===

.. figure:: img.png

   Fig caption no numfig.

.. topic:: T
   :class: tclass

   body

   Nested:

      quote level 1

         quote level 2

.. container:: named
   :name: cont-id

   inside

Trailing target below.

.. _trailing:
