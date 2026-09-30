Code
====

Literal::

    def f(x):
        return x  # comment

Default block::

    >>> print(1)
    1

.. code-block:: python

   import os
   print("s", 1.5, 0x1F)

.. code-block:: python
   :caption: Captioned *code*
   :name: code-one
   :linenos:
   :emphasize-lines: 2
   :lineno-start: 5

   a = 1
   b = 2
   c = 3

.. code-block:: rst

   Title
   =====

   *emph* and ``lit``.

.. code-block:: console

   $ echo hi
   hi

.. code-block:: bash

   echo "hi" | grep h

.. code-block:: text

   plain <text> & "stuff"

.. code-block:: none

   none  lang   spaces

.. code-block:: pycon

   >>> 1 + 1
   2

.. code-block:: python

   this is not { valid python ]]]

.. code-block:: json

   {"a": [1, 2]}

.. code-block:: json

   {"a": broken}

.. code-block:: nosuchlang

   stuff

.. code-block::

   x = 1

.. highlight:: none

::

   after highlight none

.. highlight:: python
   :linenothreshold: 2

::

   one
   two
   three

.. literalinclude:: example.py
   :language: python
   :lines: 1-2

.. literalinclude:: example.py
   :caption:
   :pyobject: A

.. parsed-literal::

   parsed *emph* literal

.. parsed-literal::

   parsed without markup

.. doctest-block-follows

>>> 1 + 2
3

.. code-block:: python
   :dedent: 4

       indented
