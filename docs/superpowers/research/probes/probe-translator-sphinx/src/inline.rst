Inline
======

Plain *emph* **strong** ``lit  eral --opt`` `title ref` and :sub:`sub` :sup:`sup`.

Code role :code:`x = 1` and custom role below.

.. role:: python(code)
   :language: python

Highlighted :python:`print("hi", 1)`.

Keys :kbd:`Ctrl+C` and :kbd:`A` menus :menuselection:`File --> Open` label :guilabel:`&Cancel`.

Abbr :abbr:`LIFO (last-in, first-out)` and :abbr:`XYZ`.

Cmd :command:`rm` file :file:`/usr/{name}/x` samp :samp:`print({x})` dfn :dfn:`thing` prog :program:`gcc` mail :mailheader:`Content-Type` mime :mimetype:`text/plain` newsgroup :newsgroup:`comp.lang` makevar :makevar:`CC` regexp :regexp:`a+`.

Manpage :manpage:`ls(1)`.

PEP :pep:`8` RFC :rfc:`2822` and :pep:`8#section`.

External https://example.com and `named <https://example.org/>`_ and anon `anon`__ and mailto someone@example.com.

__ https://anon.example/

Internal link to `Inline`_ and to :ref:`my-label` and :doc:`blocks` and :ref:`labelled section <my-label>`.

.. _my-label:

Labelled Section
----------------

Footnote ref [#f1]_ and numbered [1]_ and auto [#]_ and citation [CIT2002]_ and star [*]_.

.. [#f1] First footnote.
.. [1] Manual footnote.
.. [#] Auto numbered.
.. [*] Star.

.. [CIT2002] A citation.

Substitution |sub| here.

.. |sub| replace:: *replaced text*

Term ref :term:`Apple`. Download :download:`data <data.txt>` and :download:`remote <https://ex.com/f.zip>`.

Numref :numref:`fig-one` and :numref:`Figure {number} <fig-one>` and :numref:`code-one` and :numref:`tab-one`.

Env var :envvar:`HOME` option :option:`-v`.

Unresolved :ref:`nonexistent-label` and :py:func:`missing_func`.

Math :math:`a^2 + b^2 = c^2` and eq :eq:`euler`.

Nested ``code with <html> & "q"`` and literal with \ escaped.

Email link <someone@example.com> and `mail <mailto:x@y.org>`_.

Target _`inline target` here.

Problematic: `unclosed
