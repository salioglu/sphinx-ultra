import pygments
from sphinx.highlighting import PygmentsBridge
b = PygmentsBridge('html', 'sphinx')
print(pygments.__version__, repr(b.highlight_block('x = "a" + \'b\'\n', 'python')))
print(repr(b.highlight_block('def f(): pass\n', 'python')))
