import html_probe as hp, re
out = hp.build(hp.FILES, {"smartquotes": False, "html_theme": "basic"})
START = '<div class="body" role="main">'
END = '<div class="clearer"></div>'
def body_region(page):
    s = page.index(START) + len(START)
    e = page.index(END, s)
    return page[s:e]
for name in ("index", "genindex", "search"):
    page = out["full"][name]
    region = body_region(page)
    print(f"##### {name}.html body region ({len(region)} chars)")
    print(repr(region[:60]), "...", repr(region[-40:]))
    if name == "index":
        body = out["pages"]["index"]["addctx"]["body"]
        print("region == '\\n            \\n  ' + body + '\\n\\n            ':", region == "\n            \n  " + body + "\n\n            ")
    if name == "genindex":
        print(region)
