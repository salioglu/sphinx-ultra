:orphan:
:tocdepth: 2

Probe Title
===========

.. toctree::
   :caption: Contents
   :numbered:

   other

Para with *emph*, **strong**, ``lit``, `title ref`, :sub:`s`, :sup:`u`,
:abbr:`LIFO (last-in, first-out)`, :kbd:`Ctrl+C`, :guilabel:`&Cancel`,
:menuselection:`Start --> Programs`, :file:`/usr/{lib}`, :samp:`print({x})`,
:command:`ls`, :dfn:`term`, :program:`prog`, :regexp:`a+`, :mimetype:`t/p`,
:manpage:`ls(1)`, :download:`pic <pic.png>`, :code:`x = 1`, :math:`a^2`,
:pep:`8`, :rfc:`2822`, :index:`idx-role`, "quotes" -- dashes...

A named_ link, an anonymous__ link, `phrase <https://example.org/p>`_,
https://auto.example.com and mail@example.com. Footnote [#]_ and [#fnnamed]_
and [1]_ and star [*]_ and citation [CIT2002]_. Sub |sub| and |today|.

.. _named: https://example.com/named
__ https://example.com/anon

.. [#] Auto footnote.
.. [#fnnamed] Named auto.
.. [1] Manual.
.. [*] Star.
.. [CIT2002] Citation.

.. |sub| replace:: *replaced*

.. _label-a:

Section Two
-----------

.. note:: A note.

.. seealso:: Other.

.. versionadded:: 1.0
   Added text.

.. deprecated:: 2.0

.. code-block:: python
   :caption: Code caption
   :linenos:
   :emphasize-lines: 1

   x = 1
   y = 2

.. code:: python

   z = 3

::

   literal

>>> 1 + 1
2

.. math::
   :label: eq1

   e = mc^2

.. figure:: pic.png
   :alt: alt text

   Caption text.

   Legend.

.. image:: pic.png
   :width: 50%

.. only:: html

   Only html.

.. only:: latex

   Only latex.

.. index:: single: foo; bar

.. glossary::

   Term
      Definition.

.. rubric:: A rubric

.. hlist::
   :columns: 2

   * a
   * b
   * c

.. centered:: Centered text

.. acks::

   * Ack one

.. tabularcolumns:: |l|l|

.. productionlist::
   rule: "a" | "b"

.. sectionauthor:: Someone <a@b.c>

.. raw:: html

   <b>raw</b>

.. topic:: Topic title

   Topic body.

.. sidebar:: Side

   Sidebar body.

.. container:: custom

   Contained.

.. compound::

   Compound para.

.. list-table:: LT
   :header-rows: 1

   * - H1
     - H2
   * - a
     - b

+-----+-----+
| g1  | g2  |
+=====+=====+
| x   | y   |
+-----+-----+

| line one
|    line two

term
   definition

:field: value

-a  option a

.. py:function:: func(a: int, b=1) -> str
   :module: mod

   Doc.

   :param a: the a
   :type a: int
   :returns: something

Ref :ref:`label-a`, :ref:`Custom <label-a>`, :doc:`other`, :numref:`label-a`,
:term:`Term`, :py:func:`mod.func`, :eq:`eq1`, :any:`Term`.

Broken *emphasis

.. |today| replace:: TODAY

.. transition test

----

After transition.
