Desc
====

.. py:module:: mod

.. py:function:: func(a, b: int = 1, *args, c=None, **kwargs) -> str
   :async:

   Function doc.

   :param a: first
   :type a: str
   :param int b: second
   :returns: something
   :rtype: str
   :raises ValueError: when bad

.. py:function:: opt(a[, b[, c]])

.. py:function:: opt2([a, ]b, c[, d])

.. py:function:: noparams()

.. py:function:: generic[T, U: int](x: T) -> U

.. py:class:: Klass(x)
   :final:

   .. py:method:: meth(self)
      :classmethod:

   .. py:attribute:: attr
      :type: int
      :value: 3

   .. py:property:: prop
      :type: str

.. py:data:: DATA
   :value: 42

.. py:exception:: Err

.. py:decorator:: deco

.. py:function:: long_function_name(parameter_one: int, parameter_two: str, parameter_three: float, parameter_four: list) -> None
   :single-line-parameter-list:

.. py:function:: multi(aaaa, bbbb[, cccc])

.. c:function:: int c_func(int a, char *b)

.. cpp:class:: template<typename T> Foo

.. cpp:function:: void bar(int x) const

.. std:option:: -v, --verbose

   Verbose.

.. envvar:: HOME

   Home dir.

.. describe:: thing

   Generic describe.

.. object:: obj

.. py:function:: nocontent(x)
   :no-index:

.. py:function:: f1(x)
                 f2(y)

   Two signatures.

:py:func:`func` and :py:class:`Klass` and :py:meth:`Klass.meth` and :func:`mod.opt`.
