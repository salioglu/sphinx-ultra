Top
===

Literal across lines ``foo
bar`` end and ``a	tab`` and ``x    y``.

C expr :cpp:expr:`a + b` and :c:texpr:`int*` and :cpp:any:`nothing`.

Index role :index:`Foo <single: foo>` and :index:`bar`.

:empty field:
:other: x

.. rst-class:: details

term
   hidden def

.. rst-class:: details open

term2
   shown def

Abbr :abbr:`A&B ("quoted" <x>)`.

.. rst-class:: field-indent-4em

:a: b

Email in text user@example.com plain.

Title attr: `link <https://x.org/?a=1&b=2>`_.

.. _Top:

Anchor `Top`_.

.. py:function:: f(a: "str" = '<x>')

.. py:function:: g() -> list[int]

.. js:function:: jsf(a, b)

.. rst:directive:: .. mydir:: arg

.. rst:role:: myrole

.. py:type:: Alias
   :canonical: int

.. py:function:: h(*, a, b=1, /, c)
