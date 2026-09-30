Tables
======

+-----+-----+
| A   | B   |
+=====+=====+
| 1   | 2   |
+-----+-----+
| 3   | 4   |
+-----+-----+
| span      |
+-----------+

===  ===
X    Y
===  ===
a    b
c    d
===  ===

.. table:: Caption Table
   :name: tab-one
   :align: center
   :widths: 30 70

   ===  ===
   X    Y
   ===  ===
   a    b
   ===  ===

.. list-table:: List Table
   :header-rows: 1
   :stub-columns: 1
   :widths: auto
   :class: longtable

   * - H1
     - H2
   * - s1
     - v1

.. csv-table:: CSV
   :header: "A", "B"
   :width: 50%

   1, 2

.. table::
   :width: 300

   ===  ===
   a    b
   ===  ===

===  ===
one
===  ===

Two-para cell:

+-------+
| p1    |
|       |
| p2    |
+-------+
