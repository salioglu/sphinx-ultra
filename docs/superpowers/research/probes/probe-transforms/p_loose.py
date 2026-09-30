import harness
for src in ['T\n=\n\nA `x`__.\n', 'T\n=\n\nA `x`__.\n\n\n\n', 'T\n=\n\nA `x`__.', 'T\n=\n\nA `x`__.\n\n.. note:: x\n\n   y\n']:
    r = harness.build({'index.rst': src}, conf={'smartquotes': False})
    print(repr(src), '->', r['warnings'].strip().splitlines()[0])
