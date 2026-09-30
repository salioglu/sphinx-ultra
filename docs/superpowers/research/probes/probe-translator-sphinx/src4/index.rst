:author: Me
:version: 1

Top
===

.. unknowndir:: arg

   body

.. header:: Header text

.. footer:: Footer text

.. sectnum::

Sec A
-----

.. meta::
   :description: A description
   :keywords: a, b

.. epigraph::

   Epigraph text.

   -- Someone

.. highlights::

   Highlight text.

.. pull-quote::

   Pull quote.

.. line-block::

   one

   three

Title ref with target: `Sec A`_.

.. _sec a again:

Paragraph.

.. |br| raw:: html

   <br />

A |br| B

Sec B
-----

Deep nested:

* item

  - nested

    1. num

Term
  : classifier

.. note:: One line note with **strong**.

.. class:: special

   Paragraph with class via class directive.

.. image:: nonexistent.svg
   :class: a b

.. important::
   :name: imp-id
   :class: extra

   Named admonition.

.. versionchanged:: 1.0

   Multi paragraph first.

   Second paragraph.

.. seealso::
   :class: cls

   Also.
