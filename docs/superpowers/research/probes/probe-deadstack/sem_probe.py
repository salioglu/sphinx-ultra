import minijinja, jinja2, jinja2.sandbox
def mj(src, templates=None, **ctx):
    env = minijinja.Environment(pycompat=False, templates=templates or {}, auto_escape_callback=lambda n: None)
    try:
        return env.render_str(src, **ctx) if not templates else env.render_template(src, **ctx)
    except minijinja.TemplateError as e:
        return f"ERR[{e.kind}]: {e.message}"
def j2(src, templates=None, **ctx):
    env = jinja2.sandbox.SandboxedEnvironment(loader=jinja2.DictLoader(templates or {}), extensions=['jinja2.ext.i18n'])
    env.install_null_translations()
    try:
        return (env.get_template(src) if templates else env.from_string(src)).render(**ctx)
    except Exception as e:
        return f"ERR: {e!r}"
cases = [
 ("and/or value", "{% set r = x is not defined and ' &#187;' or x %}[{{ r }}]", {}),
 ("and/or value2", "{% set r = x is not defined and ' &#187;' or x %}[{{ r }}]", {'x':'Q'}),
 ("none render", "[{{ n }}][{{ t }}][{{ f }}][{{ l }}][{{ d }}]", {'n':None,'t':True,'f':False,'l':['a',1],'d':{'k':'v'}}),
 ("lower bool", "{{ b|lower }}", {'b':True}),
 ("escape", "{{ s|e }}", {'s':'a"b\'c/d<e>&'}),
 ("escape None", "[{{ n|e }}][{{ u|e }}]", {'n':None}),
 ("slice 2", "{% for c in items|slice(2) %}[{{ c|join(',') }}]{% endfor %}", {'items':[1,2,3,4,5]}),
 ("slice 2 of 1", "{% for c in items|slice(2) %}[{{ c|join(',') }}]{% endfor %}", {'items':[1]}),
 ("for-if", "{% for c in items if c %}[{{ c }}]{% endfor %}", {'items':[0,1,'',2]}),
 ("nested unpack", "{% for a, (b, c, _) in items %}{{a}}{{b}}{{c}}{{_}};{% endfor %}", {'items':[('x',('y','z','w'))]}),
 ("list neq", "{{ s != [] }} {{ e != [] }} {{ s != None }}", {'s':['a'],'e':[]}),
 ("iterable str", "{{ c is iterable and c is not string }}|{{ l is iterable and l is not string }}", {'c':'abc','l':['a']}),
 ("str concat", "{{ '<a href=\"' + p + '\">' + 'X' + '</a>' }}", {'p':'x.html'}),
 ("tilde", "{{ '_static/' ~ n }}", {'n':5}),
 ("comment ws", "a  {#- c #}  b", {}),
 ("int div", "{{ 7 // 2 }} {{ 7 % 2 }}", {}),
 ("attr on map", "[{{ c|attr('filename') }}][{{ s|attr('filename') }}]", {'c':{'filename':'x.css'},'s':'x.css'}),
 ("float", "{{ f }}", {'f':1.5}),
 ("undefined attr", "[{{ n.title }}]", {'n':None}),
 ("string escape in literal", "{{ 'it\\'s' }}", {}),
 ("safe plus", "{% set t = ' &#8212; '|safe + d|e %}{{ t }}", {'d':'A&B'}),
 ("striptags", "{{ s|striptags }}", {'s':'<b>a</b>  &amp; b'}),
 ("default", "{{ x|default('en') }}", {}),
 ("loop.index", "{% for x in l %}{{ loop.index }}{{ loop.first }}{{ loop.last }};{% endfor %}", {'l':[1,2]}),
 ("inline if", "{{ 'true' if b else 'false' }}", {'b':False}),
 ("None literal", "{{ x == None }}", {'x':None}),
 ("callable space", "{% macro m() %}M{% endmacro %}{{- m () }}", {}),
 ("items method", "{% for k, v in d.items() %}{{k}}={{v}};{% endfor %}", {'d':{'b':1,'a':2}}),
 ("dict iteration order", "{% for k in d %}{{k}};{% endfor %}", {'d':{'b':1,'a':2}}),
]
for name, src, ctx in cases:
    a, b = mj(src, **ctx), j2(src, **ctx)
    flag = "SAME" if a == b else "DIFF"
    print(f"{flag} {name}: minijinja={a!r} jinja2={b!r}")
