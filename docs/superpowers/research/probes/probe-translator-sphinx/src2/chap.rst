Chapter
=======

.. _chap-sec:

Section One
-----------

.. _fig-a:

.. figure:: img.png

   Cap A.

.. code-block:: python
   :caption: Code cap

   x = 1

.. table:: Tab cap

   ===  ===
   a    b
   ===  ===

.. math::
   :label: eq1

   a = b

Ref :eq:`eq1` and :numref:`fig-a` and :ref:`chap-sec` and :numref:`chap-sec`.

Level3
~~~~~~

L4
^^

L5
""

L6
''

L7
``

L7 text.

.. rubric:: Rub
   :name: rub-id

.. role:: del
.. role:: kbd2(literal)
   :class: kbd

Del :del:`gone` and kbd2 :kbd2:`X`.

.. container:: ins

   ins container

.. code:: python

   code_directive = 1

.. code::

   no_lang = 1

.. sourcecode:: c

   int x;

.. code-block:: python
   :force:

   this is ~~~ forced

.. code-block:: guess

   #!/bin/bash
   echo hi

Tabs:

.. code-block:: text

   a	b

.. code-block:: python

   # trailing   
   x = "a" + 'b'  # 'quote' <tag> & amp @

Inline code fail :code:`x`.

.. role:: json(code)
   :language: json

Bad json inline :json:`{broken}`.

.. raw:: html
   :class: rawcls

   <b>bold</b>

Inline raw role:

.. role:: raw-html(raw)
   :format: html

Some :raw-html:`<i>raw</i>` text.
