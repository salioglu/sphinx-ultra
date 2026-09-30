Blocks
======

.. note::

   A note.

.. warning:: Warn text.

.. attention:: a

.. caution:: c

.. danger:: d

.. error:: e

.. hint:: h

.. important:: i

.. tip:: t

.. admonition:: Custom Title

   Generic admonition.

.. admonition:: With Class
   :class: myclass

   Body.

.. note::
   :collapsible:

   Collapsible closed? (default open)

.. note::
   :collapsible: closed

   Collapsed.

.. seealso::

   Something else.

.. seealso:: Short form.

.. versionadded:: 1.0

.. versionadded:: 1.1
   With explanation.

.. versionchanged:: 2.0
   Changed thing.

.. deprecated:: 3.0
   Gone.

.. versionremoved:: 4.0
   Removed.

.. centered:: CENTERED TEXT

.. hlist::
   :columns: 2

   * one
   * two
   * three

.. rubric:: A Rubric

.. rubric:: Heading Rubric
   :heading-level: 3

.. topic:: Topic Title

   Topic body.

.. sidebar:: Sidebar Title
   :subtitle: Sub

   Sidebar body.

.. glossary::

   Apple
      A fruit.

   Banana
   Plantain
      Yellow.

.. productionlist::
   try_stmt: `try1_stmt` | `try2_stmt`
   try1_stmt: "try" ":" suite
            : ("except" [expression] ":" suite)+

.. container:: custom

   In container.

.. compound::

   Compound para.

Line block:

| line one
| line two
|    indented

Block quote:

   Quoted text.

   -- Attribution

Definition list:

term
   def

term2 : classifier
   def2

term3 : c1 : c2
   def3

Fields:

:field one: value
:field two: value two

Bullets:

* a
* b

  para in b

* c

Enum:

3. x
4. y

(a) alpha
(b) beta

Options:

-a         option a
--long=ARG  long option
-b FILE, --bfile=FILE  both

----------

.. raw:: html

   <div class="raw">raw</div>

.. _explicit-target:

Para after target.

.. index:: single: foo; bar

.. only:: html

   Only html.

.. only:: latex

   Only latex.

.. acks::

   * Alice
   * Bob

.. tabularcolumns:: |l|r|

.. math::

   e^{i\pi} + 1 = 0

.. math::
   :label: euler

   e^{i\pi} + 1 = 0

.. math::

   a = b

   c = d \\ e = f

.. math::
   :nowrap:

   \begin{equation} x \end{equation}

.. |date| date::

.. contents:: Local TOC
   :local:

Sub A
-----

.. rst-class:: special

Para with class.

Sub B
-----

Text.
