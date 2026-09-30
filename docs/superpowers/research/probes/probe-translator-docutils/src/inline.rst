Inline
======

*emph* **strong** ``literal text  two spaces`` `title ref` :sub:`sub` :sup:`sup`
:abbr:`HTML (Hyper Text)` :code:`x = 1` :kbd:`Ctrl+C` :title-reference:`tr`

A link https://example.com/a?b=1&c=2 and `named <https://example.org>`_ and
mailto:user@example.com and user@example.com.

An _`inline target` and a ref to `inline target`_.

.. role:: custom

:custom:`custom role text`

.. |sub| replace:: substituted *text*

Substitution: |sub|.

:unknownrole:`oops` and |undefined| and `bad ref`_.

.. raw:: html

   <div class="rawblock">raw</div>

Inline raw: :raw-html:`<b>x</b>`

.. role:: raw-html(raw)
   :format: html

Inline raw after def: :raw-html:`<b>y</b>`

.. role:: lang-de
   :class: language-de

:lang-de:`deutsch`

.. comment here

Para with trailing text.
