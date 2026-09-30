from sphinx.highlighting import PygmentsBridge
from pygments.lexers import get_lexer_by_name
from pygments.lexers._mapping import LEXERS
b = PygmentsBridge('html', 'sphinx')
def show(label, src, lang, **kw):
    try:
        out = b.highlight_block(src, lang, **kw)
    except Exception as e:
        out = 'EXC %r' % e
    print('---', label, repr(out))
show('Python cap leading blank', '\nx\n\n', 'Python')
show('python leading blank', '\nx\n\n', 'python')
show('txt', 'x', 'txt')
print('num lexers', len(LEXERS), 'num aliases', sum(len(v[2]) for v in LEXERS.values()))
for name in ['console','shell','sh','bash','rst','rest','restructuredtext','json','yaml','toml','ini','cfg','c','cpp','javascript','js','html','xml','diff','udiff','make','text','pycon','ipython','python3','py','sql','docker','dockerfile','powershell','bat','doscon','shell-session','ps1con','jinja','html+jinja','python2','py2','numpy','cython','pytb']:
    try:
        l = get_lexer_by_name(name)
        print(name, '->', type(l).__name__, type(l).__mro__[1].__name__)
    except Exception as e:
        print(name, '-> ERR', e)
from pygments.lexers.shell import BashSessionLexer
from pygments import lex
print(list(lex('$ echo hi\nhi\n', BashSessionLexer())))
