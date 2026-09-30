import os, minijinja
SPHINX='/root/.cache/uv/archive-v0/b4dBDAdEzskuqge1iT52j/lib/python3.12/site-packages/sphinx'
AL=os.path.join(os.path.dirname(SPHINX),'alabaster')
BASIC=os.path.join(SPHINX,'themes','basic')
files = [(BASIC, f) for f in sorted(os.listdir(BASIC)) if f.endswith(('.html','.xml'))]
files += [(AL, f) for f in sorted(os.listdir(AL)) if f.endswith('.html')]
files += [(os.path.join(BASIC,'static'), f) for f in sorted(os.listdir(os.path.join(BASIC,'static'))) if f.endswith('.jinja')]
files += [(os.path.join(AL,'static'), 'alabaster.css_t')]
for d, f in files:
    src = open(os.path.join(d,f)).read()
    env = minijinja.Environment(pycompat=False)
    try:
        env.add_template(f, src)
        # force compile by rendering with loader failing gracefully
        print(f"{os.path.basename(d)}/{f}: COMPILED OK")
    except minijinja.TemplateError as e:
        print(f"{os.path.basename(d)}/{f}: ERROR kind={e.kind} line={e.line} msg={e.message!r}")
