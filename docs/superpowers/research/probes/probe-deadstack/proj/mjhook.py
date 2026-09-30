import os, re, json
import minijinja, markupsafe

SPHINX_THEMES = None
PYCOMPAT = os.environ.get("MJ_PYCOMPAT", "0") == "1"

def _trim(s):
    return re.sub(r'\s*\n\s*', ' ', s.strip())

TRANS_RE = re.compile(r'\{%(-?)\s*trans\b(.*?)(-?)%\}(.*?)\{%(-?)\s*endtrans\s*(-?)%\}', re.S)
PLACEHOLDER_RE = re.compile(r'\{\{-?\s*([A-Za-z_][A-Za-z0-9_]*)\s*-?\}\}')

def split_args(s):
    out, depth, cur, q = [], 0, '', None
    for ch in s:
        if q:
            cur += ch
            if ch == q: q = None
            continue
        if ch in '"\'': q = ch; cur += ch; continue
        if ch in '([{': depth += 1
        if ch in ')]}': depth -= 1
        if ch == ',' and depth == 0:
            out.append(cur); cur = ''; continue
        cur += ch
    if cur.strip(): out.append(cur)
    return [a.strip() for a in out if a.strip()]

def rewrite_trans(src):
    def repl(m):
        lstrip_outer, args, rstrip_inner, body, lstrip_endinner, rstrip_outer = m.groups()
        args = args.strip()
        trimmed = False
        if args.startswith('trimmed'):
            trimmed = True; args = args[len('trimmed'):].strip().lstrip(',').strip()
        elif args.startswith('notrimmed'):
            args = args[len('notrimmed'):].strip().lstrip(',').strip()
        variables = {}
        for a in split_args(args):
            if '=' in a and not a.startswith('='):
                k, v = a.split('=', 1); variables[k.strip()] = v.strip()
            else:
                variables[a] = a
        if rstrip_inner: body = body.lstrip()
        if lstrip_endinner: body = body.rstrip()
        referenced = []
        def ph(mm):
            referenced.append(mm.group(1)); return '%(' + mm.group(1) + ')s'
        parts = PLACEHOLDER_RE.split(body)
        # rebuild with escaping of % in literal parts
        msg = ''
        for i, p in enumerate(parts):
            if i % 2 == 0: msg += p.replace('%', '%%')
            else:
                referenced.append(p); msg += '%(' + p + ')s'
                variables.setdefault(p, p)
        if trimmed: msg = _trim(msg)
        if not referenced: msg = msg.replace('%%', '%')
        lit = json.dumps(msg)
        l = '{{-' if lstrip_outer else '{{'
        r = '-}}' if rstrip_outer else '}}'
        if variables:
            kw = ', '.join(f'{k}=({v})' for k, v in variables.items())
            return f'{l} __trans({lit}, {kw}) {r}'
        return f'{l} __trans({lit}) {r}'
    return TRANS_RE.sub(repl, src)

MACRO_RE = re.compile(r'\{%(-?)\s*macro\s+([A-Za-z_]\w*)\s*\(\s*\)\s*(-?)%\}(.*?)\{%(-?)\s*endmacro\s*(-?)%\}', re.S)
def rewrite_block_macros(src, names_out):
    def repl(m):
        l1, name, r1, body, l2, r2 = m.groups()
        if not re.search(r'\{%-?\s*block\b', body):
            return m.group(0)
        names_out.add(name)
        return (f'{{%{l1} if false %}}{{% block __macro_{name} {r1}%}}{body}'
                f'{{%{l2} endblock %}}{{% endif {r2}%}}')
    return MACRO_RE.sub(repl, src)

MACRO_NAMES = {'relbar', 'sidebar'}
def rewrite_calls(src, names):
    for n in names:
        src = re.sub(r'(?<![\w.])' + n + r'\s*\(\s*\)', f'self.__macro_{n}()', src)
    return src

def preprocess(src):
    names = set()
    src = rewrite_trans(src)
    src = rewrite_block_macros(src, names)
    src = rewrite_calls(src, MACRO_NAMES | names)
    return src

def make_env(app, builder):
    templates = builder.templates
    loaders = templates.loaders
    tplen = templates.templatepathlen
    def load(name):
        ls = loaders
        if name.startswith('!'):
            ls = ls[tplen:]; name = name[1:]
        for loader in ls:
            for sp in loader.searchpath:
                p = os.path.join(sp, name)
                if os.path.isfile(p):
                    return preprocess(open(p, encoding='utf-8').read())
        return None
    def esc(v):
        if v is None:  # probe artifact: minijinja-py passes undefined as None
            return minijinja.Markup('')
        if isinstance(v, minijinja.Markup) or hasattr(v, '__html__'):
            return v
        return minijinja.Markup(str(markupsafe.escape(v)))
    def striptags(v):
        return markupsafe.Markup(str(v)).striptags()
    from sphinx.jinja2glue import _tobool, _toint, _todim, _slice_index, idgen
    state = {}
    def accesskey(key):
        d = state.setdefault('keys', {})
        if key and key not in d:
            d[key] = 1
            return 'accesskey="%s"' % key
        return ''
    def trans(msg, **kw):
        s = msg
        if kw:
            s = s % {k: str(v) for k, v in kw.items()}
        return s
    env = minijinja.Environment(
        loader=load, pycompat=PYCOMPAT, auto_escape_callback=lambda n: None,
        filters={'e': esc, 'escape': esc, 'striptags': striptags, 'tobool': _tobool,
                 'toint': _toint, 'todim': _todim, 'slice_index': lambda v, n: list(_slice_index(v, n))},
        globals={'_': lambda s: s, 'gettext': lambda s: s, '__trans': trans,
                 'accesskey': accesskey, 'idgen': idgen},
    )
    return env, state

def on_page(app, pagename, templatename, ctx, doctree):
    env, state = make_env(app, app.builder)
    c = dict(ctx)
    try:
        c['script_files'] = sorted(c['script_files'], key=lambda js: js.priority)
    except AttributeError: pass
    try:
        c['css_files'] = sorted(c['css_files'], key=lambda css: css.priority)
    except AttributeError: pass
    out_dir = os.path.join(app.outdir, '..', 'mj')
    try:
        out = env.render_template(templatename, **c)
    except minijinja.TemplateError as e:
        out = f'MINIJINJA ERROR: {e}'
    p = os.path.join(out_dir, pagename + ('.xml' if templatename.endswith('.xml') else '.html'))
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, 'w', encoding='utf-8') as f:
        f.write(out)

def setup(app):
    app.connect('html-page-context', on_page, priority=900)
    app.connect('build-finished', on_finished)
    return {'parallel_read_safe': True}

def on_finished(app, exc):
    b = app.builder
    env, state = make_env(app, b)
    ctx = b.globalcontext.copy()
    if b.indexer is not None:
        ctx.update(b.indexer.context_for_searchtool())
    out_dir = os.path.join(app.outdir, '..', 'mj', '_static')
    os.makedirs(out_dir, exist_ok=True)
    for entry in reversed(b.theme.get_theme_dirs()):
        sd = os.path.join(entry, 'static')
        if not os.path.isdir(sd): continue
        for fn in os.listdir(sd):
            if fn.endswith(('.jinja', '_t')):
                src = open(os.path.join(sd, fn), encoding='utf-8').read()
                try:
                    out = env.render_str(preprocess(src), fn, **ctx)
                except minijinja.TemplateError as e:
                    out = f'MINIJINJA ERROR {e}'
                target = fn[:-len('.jinja')] if fn.endswith('.jinja') else fn[:-2]
                open(os.path.join(out_dir, target), 'w', encoding='utf-8').write(out)
