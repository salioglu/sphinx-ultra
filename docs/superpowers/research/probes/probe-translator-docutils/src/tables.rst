Tables
======

+-------+-------+
| H1    | H2    |
+=======+=======+
| a     | b     |
+-------+-------+
| c spans       |
+---------------+

=====  =====
Col A  Col B
=====  =====
x      y
=====  =====

.. table:: Table Title
   :align: center
   :width: 50%
   :widths: 30 70
   :name: tbl-name

   =====  =====
   A      B
   =====  =====
   1      2
   =====  =====

.. list-table:: List Table
   :header-rows: 1
   :stub-columns: 1
   :widths: 1 2 3
   :class: myclass

   * - Stub
     - H2
     - H3
   * - r1
     - v1
     - v2

       multi para
   * - r2
     - v3
     -

.. csv-table:: CSV
   :header: "a", "b"
   :widths: auto

   1, 2

.. table::
   :width: 300

   =  =
   a  b
   =  =

+-----+-----+
| row | x   |
| span+-----+
|     | y   |
+-----+-----+
