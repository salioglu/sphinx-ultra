import harness
src = 'T\n=\n\nA `x`__ and `y`__ and [#]_ [#]_ and [*]_ [*]_.\n\n__ https://one\n\n.. [#] only one\n.. [*] only sym\n\n.. _ind: missing_\n\nuse `ind`_\n\n.. _c1: c2_\n.. _c2: c1_\n\nuse `c1`_\n'
r = harness.build({'index.rst': src}, conf={'smartquotes': False, 'keep_warnings': True})
print(r['warnings']); print(r['resolved']['index'])
