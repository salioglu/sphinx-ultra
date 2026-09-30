from sphinx.highlighting import PygmentsBridge
import pygments
print("pygments", pygments.__version__)
b = PygmentsBridge('html', 'sphinx')
def show(label, src, lang, **kw):
    try:
        out = b.highlight_block(src, lang, **kw)
    except Exception as e:
        out = 'EXC %r' % e
    print('---', label, repr(out))
show('none leading blank', '\n\nabc\n\n', 'none')
show('text leading blank', '\n\nabc\n\n', 'text')
show('none no trailing nl', 'abc', 'none')
show('none empty', '', 'none')
show('none CRLF', 'a\r\nb', 'none')
show('none tab', 'a\tb', 'none')
show('inline linenos 10 lines', '\n'.join(str(i) for i in range(10)), 'none', linenos='inline')
show('inline linenos start 98', 'a\nb\nc', 'none', linenos='inline', linenostart=98)
show('hl_lines+linenos', 'a\nb\nc', 'none', linenos='inline', hl_lines=[1,3])
show('hl_lines only', 'a\nb\nc', 'none', hl_lines=[2])
show('table linenos', 'a\nb', 'none', linenos='table')
show('table linenos 10', '\n'.join('x' for i in range(10)), 'none', linenos='table')
show('nowrap', 'print(1)\n', 'python', nowrap=True)
show('default dollar', '$ pip install foo', 'default')
show('default backtick', 'a = `b`', 'default')
show('default question', 'a?', 'default')
show('default windows', 'C:\\path\\to', 'default')
show('default pycon', '>>> 1\n1', 'default')
show('python dollar (warn+relaxed)', '$ x', 'python')
show('python multiline string', 's = """a\nb"""\n', 'python')
show('python empty line', 'a\n\nb', 'python')
show('python unicode', 'é = "ü"  # ☃', 'python')
show('python ws', 'def  f( a ):\n\tpass', 'python')
