from pygments.lexers import PythonLexer, PythonConsoleLexer
for t, v in PythonLexer(stripnl=False).get_tokens('def f(x):\n    return x  # c\n'):
    print(repr(t), repr(v))
print('---')
for t, v in PythonConsoleLexer(stripnl=False).get_tokens('>>> for i in x:\n...     pass\nout\n'):
    print(repr(t), repr(v))
