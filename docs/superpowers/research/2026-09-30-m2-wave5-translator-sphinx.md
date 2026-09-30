# M2 wave 5 research — Sphinx's `HTML5Translator` (upstream spec, part 2)

Research key: `translator-sphinx`. Upstream: Sphinx 9.1.0, docutils 0.22.4, **Pygments 2.21.0**
(the version `uv` resolved; it is NOT pinned by any `tools/gen_*_fixture.py` — see §11.1).

Paths used below:

- `SPHINX` = `/root/.cache/uv/archive-v0/b4dBDAdEzskuqge1iT52j/lib/python3.12/site-packages/sphinx`
- `DOCUTILS` = `.../site-packages/docutils`, `PYGMENTS` = `.../site-packages/pygments`
- Probe projects (re-runnable, real Sphinx 9.1.0, `html_theme='basic'`, plus a tiny
  `dumpbody` extension that writes `context['body']` — i.e. exactly the translator's
  `fragment` — to `_dump/<page>.body`):
  `/tmp/claude-0/-home-user-sphinx-ultra/46bf5e6b-694f-5b8e-ba0d-36f1851a8974/scratchpad/probe-translator-sphinx/src{,2..8}`
  built into `out{,2..8}`. Command:
  `PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' python -m sphinx -b html -q srcN outN`.
  All HTML quoted in this note is copied from those builds unless marked otherwise.

This note is the translator contract: **every `visit_`/`depart_` Sphinx defines, plus every
inherited docutils behaviour that shows up in Sphinx output**, with exact bytes. The page
template / builder / resolve transforms are covered by other research keys; they are only
mentioned where the translator depends on them (§10).

---

## 0. TL;DR for implementers

1. Sphinx never calls `HTMLWriter.translate()` for normal pages. `StandaloneHTMLBuilder.write_doc`
   (`SPHINX/builders/html/__init__.py:650-665`) creates the translator directly, walks the
   *resolved* doctree, and takes `body = ''.join(visitor.fragment)` and
   `metatags = ''.join(visitor.meta[2:])`. The writer's job in wave 5 is therefore:
   **resolved doctree + small write context → `fragment` string (+ `meta[2:]`, + `has_maths_elements`)**.
2. Output is a flat append-only string; the only structural state is a heterogeneous
   `context` stack (push in visit, pop in depart), `section_level`, `protect_literal_text`
   (a counter), `in_mailto`, the list-compactness flags, table/field-list row counters, and
   the parameter-list state machine. Port it as a straight visitor with the same stack
   discipline — `depart_document` asserts the context stack is empty.
3. `starttag()` is the single most important primitive (class ordering + dedupe, sorted
   attributes, extra ids as `<span id>`s, attribute escaping). Get it exactly right first (§2).
4. Text escaping differs by place: docutils `encode()` escapes `& < " > @` (yes, `@` →
   `&#64;`, and `"` → `&quot;` in *text*), attributes additionally fold `\n\r\t\v\f` to space;
   Pygments 2.21 escapes only `& < >` (`"`/`'` left raw). (§2.2, §9.4)
5. Code highlighting: the wrapper is always the Pygments `HtmlFormatter` structure, even for
   `none`/`text`. A byte-exact **formatter + TextLexer** is small and should ship in wave 5;
   a byte-exact **PythonLexer/PythonConsoleLexer** is required for the default
   `highlight_language='default'` and is a substantial, separable port (§9.9). Recommend the
   wave-5 HTML oracle runs its projects with `highlight_language = 'none'` first.
6. Pin Pygments (and Pillow-absence, imagesize, Jinja2) in the HTML fixture generator:
   Pygments 2.19.2/2.20.0 emit `&quot;`/`&#39;` inside highlighted code, 2.21.0 does not (§11.1).

---

## 1. How Sphinx drives the translator

### 1.1 Call path (html builder)

- `SPHINX/builders/html/__init__.py:650-665` `write_doc(docname, doctree)`:
  - `doctree.settings = self.docsettings`
  - `self.secnumbers = self.env.toc_secnumbers.get(docname, {})`
  - `self.fignumbers = self.env.toc_fignumbers.get(docname, {})`
  - `self.imgpath = relative_uri(self.get_target_uri(docname), '_images')`
  - `self.dlpath = relative_uri(self.get_target_uri(docname), '_downloads')`
  - `self.current_docname = docname`
  - `visitor = self.create_translator(doctree, self)`; `doctree.walkabout(visitor)`
  - `body = ''.join(visitor.fragment)`; `clean_meta = ''.join(visitor.meta[2:])`
  - `ctx = self.get_doc_context(docname, body, clean_meta)`;
    `ctx['has_maths_elements'] = visitor._has_maths_elements`; `handle_page(...)`.
- Before that, in the serial part of the write loop, `write_doc_serialized`
  (`:667-673`) runs `post_process_images(doctree)` (`:961-987` + base
  `SPHINX/builders/__init__.py:213-250`): picks an image candidate, fills
  `self.images[candidate] = env.images[candidate][1]` (the unique `_images/` filename),
  and **wraps resized images in a scaled-image-link `reference`** (§7.7). This mutates the
  doctree before the translator sees it.
- `SPHINX/writers/html.py:23-62` `HTMLWriter(html4css1.Writer)`: `translate()` creates the
  translator via `builder.create_translator`, walks, sets `self.output = visitor.astext()`,
  copies the 19 `visitor_attributes` (`head_prefix stylesheet head body_prefix
  body_pre_docinfo docinfo body fragment body_suffix meta title subtitle header footer
  html_prolog html_head html_title html_subtitle html_body`), `clean_meta =
  ''.join(visitor.meta[2:])`, `_has_maths_elements`. In 9.1 `HTMLWriter` is used only for:
  its settings spec (`_get_settings(HTMLWriter, …)`, `:155-160` and `:442-445`) and
  `_WRITER_TRANSFORMS = HTMLWriter(None).get_transforms()` (`:95`, used by `render_partial`).
  **No need to port `HTMLWriter.translate()`.**
- `render_partial(node)` (`:409-425`): used for titles (`title`, `prev/next/parents` titles)
  and the local TOC (`toc`) in the page context (`get_doc_context`, `:564-642`). It puts the
  node into a fresh `docutils.utils.new_document('<partial node>', self._settings)`, applies
  reader+parser+writer transforms, walks with the same translator class and returns
  `{'fragment': ''.join(visitor.fragment), 'title': ''.join(visitor.title)}`. The `title`
  part is the inner HTML of a `title` that is a direct child of `document` (the
  `in_document_title` mechanism, `DOCUTILS/writers/_html_base.py:1771-1789`). NB its
  settings are plain docutils defaults (`cloak_email_addresses=False`, etc.), not
  `docsettings`.

### 1.2 Parts / fragment extraction (docutils base)

`DOCUTILS/writers/_html_base.py:327-354` initialises `body=[]`, `fragment=[]`,
`meta=[generator]`, `head=[]`, …; `depart_document` (`:957-983`) does
`self.fragment.extend(self.body)` and `assert not self.context`. Because Sphinx sets
`doctitle_xform=False` and `sectsubtitle_xform=False` (`SPHINX/environment/__init__.py:59-75`),
nothing is ever moved to `body_pre_docinfo`/`title`, and the docinfo is deleted by
`MetadataCollector` at read time, so **`fragment == body` for every page**. The document's
top section title is an ordinary `<h1>` inside `<section>`.

`meta` list: `_html_base.__init__` starts `meta = [generator]` and, because Sphinx's
`output_encoding` is `'utf-8'` (≠ `'unicode'`), inserts `'<meta charset="utf-8" />\n'` at 0
(`:352-354`); `html5_polyglot.HTMLTranslator.__init__` appends
`'<meta name="viewport" content="width=device-width, initial-scale=1" />\n'`
(`DOCUTILS/writers/html5_polyglot/__init__.py:129-132`). So `meta[2:]` = the docutils
viewport meta + any `.. meta::` tags (`visit_meta`, `emptytag(node,'meta',**attrs)` → sorted
attrs: `<meta content="A description" name="description" />\n`). Probe 4 page head shows
`<meta name="viewport" content="width=device-width, initial-scale=1.0" /><meta name="viewport"
content="width=device-width, initial-scale=1" />\n<meta content="A description"
name="description" />` — the first from basic `layout.html`, the second+ are `metatags`.

### 1.3 Effective docutils settings (`docsettings`)

`_get_settings(HTMLWriter, defaults=env.settings, read_config_files=True)` with
`compact_lists = bool(html_compact_lists)` (`:442-445`). Values the translator reads:

| setting | value | source | effect |
|---|---|---|---|
| `initial_header_level` | `'1'` | html4css1 Writer spec (`DOCUTILS/writers/html4css1/__init__.py` `initial_header_level` default `'1'`) | section at `section_level` n → `<h n>` |
| `cloak_email_addresses` | `True` | `SPHINX/environment/__init__.py:63` | mailto cloaking (§7.1) |
| `image_loading` | `'link'` | env default | never embeds images/SVG |
| `section_self_link` | `False` | env default | no docutils self-link |
| `footnote_references` | `'brackets'` | docutils default | `brackets` classes |
| `footnote_backlinks` | `True` | docutils default | label backlinks |
| `toc_backlinks` | `'entry'` | docutils default | `contents` directive backrefs on titles |
| `attribution` | `'dash'` | docutils default | `—` (U+2014) prefix |
| `compact_lists` | `html_compact_lists` (True) | `:445` | `simple` class |
| `compact_field_lists` | `True` | default | `simple` on field/definition lists |
| `table_style` | `''` | default | only `docutils align-*` |
| `report_level` | `2` | default | writer-generated system messages ≥ WARNING are rendered (§10) |
| `language_code` | `config.language` | Sphinx | only affects docinfo labels |
| `math_output` | irrelevant | — | Sphinx overrides math visitors |

`read_config_files=True` means a `docutils.conf` in the project / user dir can alter these —
ignore for parity but note it.

### 1.4 Builder attributes the translator reads

`builder.highlighter` (PygmentsBridge), `builder.secnumbers` (`{anchorname: tuple}` for this
doc), `builder.fignumbers` (`{figtype: {id: tuple}}` for this doc), `builder.images`
(`{source-relative uri: unique filename}`), `builder.imgpath`, `builder.dlpath`,
`builder.add_permalinks` (True for html), `builder.download_support` (True),
`builder.name` (`'html'`; `'singlehtml'` branches ignored), `builder.srcdir` (image sizes),
`builder.math_renderer_name` (`'mathjax'`), `builder.env.domains.standard_domain`
(`get_enumerable_node_type`). Config read: `html_permalinks` (True), `html_permalinks_icon`
(`'¶'`), `html_secnumber_suffix` (`'. '`), `numfig_format`, `highlight_options` (`{}`),
`html_codeblock_linenos_style` (`'inline'`), `mathjax_inline` (`['\\(', '\\)']`),
`mathjax_display` (`['\\[', '\\]']`), `math_numfig` (True), `numfig` (False), `math_numsep`
(`'.'`). Defaults: `SPHINX/builders/html/__init__.py:1492-1528`, `SPHINX/config.py:256-285`,
`SPHINX/ext/mathjax.py:152-155`.

Crate side: `env.toc_secnumbers` / `env.toc_fignumbers` already exist
(`src/env/mod.rs:177-179`) with the same shape (`BTreeMap<doc, BTreeMap<anchor, Vec<u32>>>`,
`BTreeMap<doc, BTreeMap<figtype, BTreeMap<id, Vec<u32>>>>`); `enumerable_node_type` exists at
`src/env/resolve.rs:742`; Python-`repr` helper for warnings at `src/utils.rs:262`
(`py_repr_str`). The current placeholder write path is `src/builder.rs:878`
(`"<html><body>{}</body></html>"`) and `src/html_builder.rs:359-373`.

### 1.5 Dispatch (important for sig nodes and unknown nodes)

`SphinxTranslator.dispatch_visit/dispatch_departure` (`SPHINX/util/docutils.py:782-812`)
walk `node.__class__.__mro__` and call the first `visit_<ClassName>` that exists. Hence:

- `desc_sig_space/name/operator/punctuation/keyword/keyword_type/literal_number/literal_string/literal_char`
  (all subclass `desc_sig_element(nodes.inline)`, `SPHINX/addnodes.py:313-395`) render via
  **`visit_inline`** → `<span class="w">`, `<span class="n">`, `<span class="o">`,
  `<span class="p">`, `<span class="k">`, `<span class="kt">`, `<span class="m">`,
  `<span class="s">`, `<span class="sc">` (classes come from the node).
  `SigElementFallbackTransform` (`SPHINX/transforms/post_transforms/__init__.py:335-380`)
  returns immediately for `SphinxTranslator` subclasses — **no fallback rewriting happens**.
- `literal_emphasis`/`literal_strong`/`manpage` have their own visitors (→ em/strong).
- `number_reference`/`download_reference` have their own visitors.
- A node with no visitor anywhere in its MRO → `unknown_visit` logs
  `WARNING: unknown node type: <repr>` and continues into the children; the departure then
  hits docutils' `unknown_departure`, which raises `NotImplementedError`. So unknown nodes
  (unresolved `pending_xref`, `only`, `highlightlang`) must never reach the writer — they
  are removed by the post-transforms (ReferencesResolver, OnlyNodeTransform,
  HighlightLanguageTransform).

Rust: a `match kind` with explicit fallbacks mirroring the MRO (e.g. `desc_sig_*` →
inline, `compact_paragraph` has its own no-op arm and must NOT fall to `paragraph`).

---

## 2. Core primitives (docutils `_html_base.HTMLTranslator`)

### 2.1 `starttag(node, tagname, suffix='\n', empty=False, **attributes)` — `DOCUTILS/writers/_html_base.py:550-614`

Exact algorithm:

1. `tagname.lower()`; attribute names lowercased (`CLASS=` → `class`, `ROLE=` → `role`).
2. `classes = atts.pop('classes', [])` — the explicit `classes=[...]` kwarg list comes FIRST.
3. For `cls in node['classes'] + atts.pop('class', '').split()`: if `cls.startswith('language-')`
   collect it as a language (first one becomes `lang="xx"` attribute); elif `cls.strip()`
   and `cls not in classes`: append. **Order = kwarg `classes` → node classes → `CLASS=`
   string; first occurrence wins (dedupe).** Examples from probes:
   - admonition: `CLASS='admonition note'`, node classes `['extra']` → `class="extra admonition important"`.
   - generic admonition: node `['admonition-custom-title']` + `CLASS='admonition '` → `class="admonition-custom-title admonition"`.
   - inline highlighted code: node `['code','highlight','python']` + `CLASS='docutils literal highlight highlight-python'` → `class="code highlight python docutils literal highlight-python"`.
   - definition_list: kwarg `classes=['simple']` + node `['glossary']` → `class="simple glossary"`.
   - topic: kwarg `['topic']` + node `['tclass']` → `class="topic tclass"`.
4. If the node is a `nodes.table`, drop `colwidths-auto`, `colwidths-given`, `colwidths-grid`.
5. `if classes: atts['class'] = ' '.join(classes)`.
6. `ids = node.get('ids', []) + atts.pop('ids', [])`. If any: `atts['id'] = ids[0]`; for each
   further id: if `empty` or node is `Sequential` (bullet_list, enumerated_list,
   definition_list, field_list, option_list) or `docinfo` or `table` → prefix
   `<span id="X"></span>` **before** the tag; otherwise append `<span id="X"></span>` to the
   **suffix** (i.e. right after the start tag and its `\n`).
   Probe: `<section id="labelled-section">\n<span id="my-label"></span><h2>…`;
   cpp: `<dt class="sig sig-object cpp" id="_CPPv4I0E3Foo">\n<span id="_CPPv3I0E3Foo"></span><span id="_CPPv2I0E3Foo"></span>…`;
   figure: `<figure class="align-center" id="id1">\n<span id="fig-one"></span>…`.
7. `attlist = sorted(atts.items())` — **attributes sorted by name**. Each rendered as
   `name="attval(str(value))"` (lists are space-joined first). E.g.
   `<a class="footnote-reference brackets" href="#f1" id="id2" role="doc-noteref">`,
   `<img alt="…" class="align-center" height="10" src="…" style="width: 100px;" width="20" />`,
   `<table class="docutils align-default" id="id2" style="width: 50%">`,
   `<a class="reference download internal" download="" href="…">`.
8. Result: `prefix + '<' + ' '.join([tag] + attrs) + (' /' if empty else '') + '>' + suffix`.

`emptytag(node, tag, suffix='\n', **atts)` = `starttag(..., empty=True)` → `<hr class="docutils" />`.

Hand-built strings that do NOT go through `starttag` (attribute order is literal, not
sorted!): permalinks `<a class="headerlink" href="#ID" title="TITLE">ICON</a>`; label
backlinks `<a role="doc-backlink" href="#ID">`; `<div role="list" class="citation-list">`;
`<aside class="footnote-list brackets">`; `<a class="reference internal" href="#top">`;
`<span class="fn-bracket">`; `<p class="system-message-title">`; `<span class="eqno">`; the
Pygments output.

### 2.2 Escaping

- `encode(text)` (`:411-417`): `str.translate` with `{'&':'&amp;', '<':'&lt;', '"':'&quot;',
  '>':'&gt;', '@':'&#64;'}` (`:281-286`). Applies to ALL text nodes, e.g.
  `Copyright © sign &amp; ampersand &lt; less “ quote ‘ apostrophe &#64; at.` (probe 3; quotes
  are smart-quoted at read time) and `<span class="pre">&quot;q&quot;</span>`. `'` is not escaped.
- `attval(text)` (`:387-395`): replace each of `[\n\r\t\v\f]` with a space, then `encode`;
  if `in_mailto and cloak_email_addresses`: replace `%40` → `&#37;&#52;&#48;` and `.` →
  `&#46;`. Example: `href="https://x.org/?a=1&amp;b=2"`, `title="&quot;quoted&quot; &lt;x&gt;"`.
- Non-ASCII passes through unencoded (UTF-8 output).

### 2.3 `visit_Text` — Sphinx override (`SPHINX/writers/html5.py:841-860`)

```
text = node.astext(); encoded = self.encode(text)
if self.protect_literal_text:
    for token in words_and_spaces.findall(encoded):      # r'[^ \n]+| +|\n'  (_html_base.py:276)
        if token.strip():            '<span class="pre">%s</span>' % token
        elif token in {' ', '\n'}:   token            (bare)
        else:                        '&#160;' * (len(token) - 1) + ' '
else:
    if in_mailto and cloak_email_addresses: encoded = cloak_email(encoded)
    append(encoded)
```

Notes: tokenization runs on the ENCODED string; a newline inside an inline literal is kept
(`<span class="pre">foo</span>\n<span class="pre">bar</span>`, probe 5); `x    y` →
`<span class="pre">x</span>&#160;&#160;&#160; <span class="pre">y</span>`; a tab inside a
word stays inside the `pre` span (tabs are normally expanded by docutils at read time).
`protect_literal_text` is a counter incremented by `visit_desc_signature` and by
non-kbd, non-highlighted `visit_literal`; decremented in the matching departs.
`cloak_email` (`:397-402`): `&#64;` → `<span>&#64;</span>`, `.` → `<span>&#46;</span>`.
`depart_Text` is a no-op.

### 2.4 Paragraph newline rule (`_html_base.py:1482-1490`)

`visit_paragraph`: `starttag(node,'p','')`. `depart_paragraph`: append `</p>`; then, unless
`parent is list_item or entry` **and** `len(parent) == 1`, append `\n` (and call
`report_messages`). So `<li><p>x</p></li>` vs `<li><p>b</p>\n<p>para in b</p>\n</li>`, and
`<td><p>1</p></td>` vs `<td><p>p1</p>\n<p>p2</p>\n</td>`.

### 2.5 Which nodes are `TextElement` (needed by image/reference/raw logic)

Generated with real Sphinx (`te.py`): abbreviation acronym address attribution author caption
centered citation_reference classifier comment compact_paragraph contact copyright date
desc_addname desc_annotation desc_classname desc_inline desc_name desc_optional
desc_parameter desc_parameterlist desc_returns desc_sig_element desc_sig_keyword
desc_sig_keyword_type desc_sig_literal_char desc_sig_literal_number desc_sig_literal_string
desc_sig_name desc_sig_operator desc_sig_punctuation desc_sig_space desc_signature
desc_signature_line desc_type desc_type_parameter desc_type_parameter_list doctest_block
download_reference emphasis field_name footnote_reference generated index inline label line
literal literal_block literal_emphasis literal_strong manpage math math_block
number_reference option_argument option_string organization paragraph
pending_xref_condition problematic production raw reference revision rubric status strong
subscript substitution_definition substitution_reference subtitle superscript target term
title title_reference version versionmodified.

`Admonition` subclasses (matters for `visit_title`): admonition attention caution danger
**desc** error hint important note **productionlist** seealso tip **versionmodified** warning.

---

## 3. Structure: sections, titles, headings, permalinks, numbers

### 3.1 section (`html5_polyglot:343-350`)

`visit_section`: `section_level += 1`; `starttag(node, 'section')` → `<section id="ID">\n`
(+ extra-id spans in the suffix). `depart_section`: `section_level -= 1`; `</section>\n`.

### 3.2 title — Sphinx `visit_title` (`SPHINX/writers/html5.py:516-551`)

1. If `parent is compact_paragraph and parent.get('toctree')` (toctree caption produced by
   `_resolve_toctree`, `SPHINX/environment/adapters/toctree.py:192-201`):
   `starttag(node,'p','',CLASS='caption',ROLE='heading')` + `<span class="caption-text">`;
   push `'</span></p>\n'`. → `<p class="caption" role="heading"><span class="caption-text">Contents</span></p>\n`.
2. elif parent is an `Admonition` with `'collapsible'` in its attributes:
   `starttag(node,'summary','',CLASS='admonition-title')`; push `'</summary>\n'`.
3. else docutils `visit_title` (`_html_base.py:1751-1780`), which pushes the close tag:
   - parent `topic` → `<p class="topic-title">` (close `</p>\n`); if `toc_backlinks` and
     `'contents'` in topic classes, also `<a class="reference internal" href="#top">` and
     close `</a></p>\n` — **Sphinx then pops that `<a …>` again** (step 7).
   - parent `sidebar` → `<p class="sidebar-title">`.
   - parent `Admonition` → `<p class="admonition-title">`.
   - parent `table` → `starttag(node,'caption','')` → `<caption>`; close `</caption>\n`.
   - parent `document` → `<h1 class="title">`, close `</h1>\n`, sets `in_document_title`
     (only happens in `render_partial`).
   - parent `section` → `section_title_tags` (`_html_base.py:1732-1749`,
     `html5_polyglot:388-398`): `h_level = section_level + initial_header_level - 1` =
     `section_level`; tag `h{min(h_level,6)}`; if `h_level > 6` add `aria-level=h_level`
     (`<h6 aria-level="7">`); start = `starttag(node, tag, '', **atts)`; if the title has
     `refid` (contents backlink) append
     `starttag(nodes.reference(),'a','',class='toc-backref',role='doc-backlink',href='#'+refid)`
     → `<a class="toc-backref" href="#id1" role="doc-backlink">` and close tag
     `</a></hN>\n`; else close `</hN>\n`. (`section_self_link` is False → no self-link.)
4. `self.add_secnumber(node)` (§3.4).
5. `self.add_fignumber(node.parent)` (§3.5) — real effect only for table titles.
6. If parent is a `table`: append `<span class="caption-text">`.
7. If parent is topic with `contents` class, `toc_backlinks` truthy, and `body[-1]` starts
   with `<a ` → `body.pop()` and `context[-1] = '</p>\n'`.

`depart_title` (`:553-577`):

```
close_tag = self.context[-1]
if html_permalinks and builder.add_permalinks and parent.hasattr('ids') and parent['ids']:
    if close_tag.startswith('</h'):       add_permalink_ref(parent, 'Link to this heading')
    elif close_tag.startswith('</a></h'): append('</a><a class="headerlink" href="#%s" ' % parent['ids'][0]
                                                 + 'title="Link to this heading">' + icon)
    elif parent is table:                  append('</span>'); add_permalink_ref(parent, 'Link to this table')
elif parent is table:                      append('</span>')
super().depart_title(node)   # appends context.pop(); handles in_document_title
```

Probe outputs:

```html
<h1>Probe Title<a class="headerlink" href="#probe-title" title="Link to this heading">¶</a></h1>
<h2><a class="toc-backref" href="#id1" role="doc-backlink">Sub A</a><a class="headerlink" href="#sub-a" title="Link to this heading">¶</a></h2>
<h1><span class="section-number">1. </span>Chapter<a class="headerlink" href="#chapter" title="Link to this heading">¶</a></h1>
<h6 aria-level="7"><span class="section-number">1.1.1.1.1.1.1. </span>L7<a class="headerlink" href="#l7" title="Link to this heading">¶</a></h6>
<caption><span class="caption-number">Table 1 </span><span class="caption-text">Caption Table</span><a class="headerlink" href="#tab-one" title="Link to this table">¶</a></caption>
<caption><span class="caption-text">No numfig caption</span><a class="headerlink" href="#id5" title="Link to this table">¶</a></caption>
<p class="topic-title">Local TOC</p>
```

With `html_permalinks = False` (probe 7): `<h1>Top</h1>`, `<caption><span class="caption-text">Cap</span></caption>`.
Titles can contain any inline markup (`<h2>Title <em>with</em> <code class="docutils literal notranslate"><span class="pre">markup</span></code> and <a class="reference external" href="https://x.y">link</a><a class="headerlink" …>¶</a></h2>`).

### 3.3 Permalinks — `add_permalink_ref(node, title)` (`:461-466`)

```
icon = config.html_permalinks_icon
if node['ids'] and config.html_permalinks and builder.add_permalinks:
    append(f'<a class="headerlink" href="#{node["ids"][0]}" title="{title}">{icon}</a>')
```

`title` and `icon` are inserted **raw** (icon may contain HTML: probe 3 with
`html_permalinks_icon = '<span>#</span>'` → `…title="Link to this heading"><span>#</span></a>`).
Titles are `_()`-translated (English below). Complete catalogue of callers:

| where | node passed | title |
|---|---|---|
| `depart_title`, section heading | section | `Link to this heading` |
| `depart_title`, heading with toc-backref | section (hand-built string, same shape) | `Link to this heading` |
| `depart_title`, table title | table | `Link to this table` |
| `depart_desc_signature` (not multiline) | desc_signature | `Link to this definition` |
| `depart_desc_signature_line` with `add_permalink` | parent desc_signature | `Link to this definition` |
| `depart_term` in a glossary (`term.parent.parent.parent` is `glossary`) | term | `Link to this term` |
| `depart_caption`, code-block caption | container (`literal-block-wrapper`) | `Link to this code` |
| `depart_caption`, figure caption | figure | `Link to this image` |
| `depart_caption`, caption whose parent has `toctree` | parent.parent | `Link to this toctree` — **dead in practice**: toctree captions are `title` nodes, not `caption` |
| `html_visit_displaymath` (numbered) | math_block | `Link to this equation` |

NOT permalinked: rubric (even with ids), topic/sidebar/admonition titles, toctree captions,
figures without caption, code blocks without caption, unnumbered math.

### 3.4 Section numbers — `get_secnumber` / `add_secnumber` (`:406-433`)

```
get_secnumber(title):
    if title.get('secnumber'): return it
    if parent is section:
        anchorname = '#' + parent['ids'][0]
        if anchorname not in builder.secnumbers: anchorname = ''   # first heading has key ''
        if builder.secnumbers.get(anchorname): return it           # () is falsy → None
    return None
add_secnumber: if secnumber: append('<span class="section-number">%s</span>' % ('.'.join(map(str,n)) + html_secnumber_suffix))
```

`toc_secnumbers[doc]` keys: `''` for the document's first section, `'#id'` for the others
(`SPHINX/environment/collectors/toctree.py:208-245`; depth-exhausted entries are stored as
`()`, so they don't fall back to `''`). Output `<span class="section-number">1.1. </span>`;
with `html_secnumber_suffix = ' '` → `<span class="section-number">1 </span>`.

Section numbers in **references** (toctree entries): `visit_reference` appends
`('%s' + secnumber_suffix) % '.'.join(map(str, node['secnumber']))` as bare text right after
the `<a …>` start tag when `node.get('secnumber')` (`:357-360`) →
`<a class="reference internal" href="chap.html">1. Chapter</a>` (no span).

`.. sectnum::` (docutils) is different: `visit_generated` with class `sectnum` →
`<span class="sectnum">1.1 </span>` (text `rstrip(' ')` of the NBSP padding + one space;
`_html_base.py:1129-1136`).

### 3.5 Figure numbers — `add_fignumber(node)` (`:435-459`)

```
figtype = std_domain.get_enumerable_node_type(node)   # SPHINX/domains/std/__init__.py:1380-1393
if figtype:
    if len(node['ids']) == 0: logger.warning('Any IDs not assigned for %s node', node.tagname, location=node)
    else:
        if node['ids'][0] in builder.fignumbers.get(figtype, {}):
            append('<span class="caption-number">')
            prefix = config.numfig_format.get(figtype)
            if prefix is None: logger.warning('numfig_format is not defined for %s', figtype)   # span left UNCLOSED
            else: append(prefix % '.'.join(map(str, numbers)) + ' '); append('</span>')
```

`get_enumerable_node_type`: section → `'section'`; container that has a `literal_block`
child (`'literal_block' in node` = attribute `literal_block` set, and has such a child) →
`'code-block'`; else `enumerable_nodes[node.__class__]` = figure→`figure`, table→`table`,
**container→`code-block`**. Called from `visit_title` (parent) and `visit_caption` (parent).
For section titles it's a no-op in practice (sections are never keys of `fignumbers`).
Default `numfig_format` (`SPHINX/config.py:684-689`): `Section %s`, `Fig. %s`, `Table %s`,
`Listing %s`. Output: `<span class="caption-number">Fig. 1.1 </span>`. With `numfig=False`
(default) `fignumbers` is empty → no span at all.

---

## 4. Block-level nodes

Format: **upstream location** — exact output. "base" = docutils `_html_base.py`, "h5" =
`html5_polyglot/__init__.py`, "sx" = `SPHINX/writers/html5.py`.

### 4.1 Paragraph-like

- `paragraph` (base 1482-1490): `<p…>` … `</p>` + `\n` rule (§2.4). Classes/ids on `<p>`:
  `<p class="special">`, `<p id="explicit-target">`.
- `compact_paragraph` (sx 728-732): visit/depart **no-op** (children inline, no `<p>`).
- `rubric` (sx 580-601): if `heading-level` in 1..6 → `starttag(node, f'h{n}', '', CLASS='rubric')` / `</hN>\n`
  (`<h3 class="rubric">Heading Rubric</h3>`); else base → `<p class="rubric">…</p>\n`
  (`<p class="rubric" id="rub-id">Rub</p>`). Other levels warn `unsupported rubric heading level: %s`
  (type `html`) — unreachable from rST (the directive only accepts 1–6, `SPHINX/directives/patches.py:214`).
- `centered` (sx 722-726): `starttag(node,'p',CLASS='centered') + '<strong>'` … `'</strong></p>'`
  — **no trailing newline**: `<p class="centered">\n<strong>CENTERED TEXT</strong></p><table class="hlist">…`.
- `transition` (base 1803): `<hr class="docutils" />\n`.
- `line_block`/`line` (base 1258-1270): `<div class="line-block">\n`, each line
  `<div class="line">text</div>\n`, empty line `<div class="line"><br /></div>\n`, nested
  line blocks nest the div.
- `block_quote` (sx 669-673): `starttag(node,'blockquote') + '<div>'` … `'</div></blockquote>\n'`
  → `<blockquote>\n<div><p>Quoted text.</p>\n<p class="attribution">—Attribution</p>\n</div></blockquote>\n`;
  `epigraph`/`highlights`/`pull-quote` just add their class: `<blockquote class="epigraph">`.
- `attribution` (base 682-694): `starttag(node,'p', '—', CLASS='attribution')` … `'' + '</p>\n'`.
- `compound` (base 844-848): `<div class="compound">\n` … `</div>\n`. The toctree wrapper is a
  compound with class `toctree-wrapper` → `<div class="toctree-wrapper compound">` (+ `id` from `:name:`).
  Hidden toctree leaves `<div class="toctree-wrapper compound">\n</div>\n` (probe 7).
- `container` (h5 166-182): if exactly one class in `{'ins','del'}` → that tag (and the class
  is removed), else `div`; `starttag(node, tag, CLASS='docutils container')` → `<div class="custom docutils container">`,
  `<ins class="docutils container">`, `<div class="literal-block-wrapper docutils container" id="code-one">`.
- `topic` (h5 365-385): `contents` class → `<nav>` without the `topic` class (+ `role="doc-toc"`
  only if the topic's parent is the document); `abstract` → `<div role="doc-abstract">`;
  `dedication` → `<div role="doc-dedication">`; else `<aside class="topic …">`. E.g.
  `<nav class="contents local" id="local-toc">`, `<aside class="topic tclass">`.
- `sidebar` (h5 353-360): `<aside class="sidebar">`; title `<p class="sidebar-title">`;
  subtitle (base 1612-1623) `<p class="sidebar-subtitle">Sub</p>\n`.
- `hlist` (sx 828-832): `'<table class="hlist"><tr>'` … `'</tr></table>\n'` (node classes/ids
  ignored); `hlistcol` (sx 834-838): `'<td>'` … `'</td>'`. Full: `<table class="hlist"><tr><td><ul class="simple">\n<li><p>one</p></li>\n<li><p>two</p></li>\n</ul>\n</td><td><ul class="simple">\n<li><p>three</p></li>\n</ul>\n</td></tr></table>\n`.
- `acks` (sx 822-826), `glossary` (sx 816-820): no-ops (children render normally).
- `productionlist` (sx 710-714): `starttag(node,'pre')` → `<pre>\n` … `</pre>\n`; `production`
  (sx 716-720) no-op. Text inside is NOT protected (plain encode). Probe:
  `<pre>\n<strong id="grammar-token-try_stmt">try_stmt</strong>  ::= <a class="reference internal" href="#grammar-token-try1_stmt"><code class="xref docutils literal notranslate"><span class="pre">try1_stmt</span></code></a> | …\n</pre>\n`.
- `raw` (base 1504-1516): only if `'html'` in `format.split()`: if node has classes wrap in
  `span` (parent TextElement) or `div`, `starttag(..., suffix='')`; append `astext()` verbatim;
  **no newline** (`<div class="raw">raw</div><p id="explicit-target">…`, `<div class="rawcls"><b>bold</b></div><p>…`,
  inline `<span class="raw-html"><i>raw</i></span>`). Non-html raw → nothing. SkipNode.
- `comment` (sx 369-372): **SkipNode, nothing emitted** (docutils would emit `<!-- -->`).
- `substitution_definition` (base 1601): SkipNode.
- `target` (base 1682-1692): if none of `refuri/refid/refname` → `starttag(node,'span','',CLASS='target')` … `</span>`
  (`<span class="target" id="index-0"></span>`, `<span class="target" id="inline-target">inline target</span>`,
  trailing unpropagated target `<span class="target" id="trailing"></span></section>` — no newline);
  otherwise nothing (the ids already moved to the next node).
- `index` (sx 810-811), `tabular_col_spec` (sx 813-814), `toctree` (sx 805-808): **SkipNode**.
- `bullet_list` containing only one `toctree` child (sx 469-473): **SkipNode** (only happens
  when rendering `env.tocs` for the local-TOC `toc` context var).
- `meta` (h5 326-330): appended to `self.meta` (not body). `header`/`footer` (h5 218-241):
  moved out of the body into `body_prefix`/`body_suffix` → **absent from `fragment`**
  (probe 4 shows neither in the page). `docinfo` (base 920-931): moved to `self.docinfo`
  → absent from fragment (and deleted at read time anyway). `decoration`: no-op.
- `start_of_file` (sx 69-75): singlehtml only (`<span id="document-%s"></span>`).

### 4.2 Admonitions (sx 375-398, 400-404, 862-914)

```
visit_admonition(node, name=''):
    tag = 'div'; attrs = {}
    if collapsible := node.get('collapsible'):
        tag = 'details'
        if collapsible == 'open': attrs['open'] = 'open'
    append(starttag(node, tag, CLASS=f'admonition {name}', **attrs))   # suffix '\n'
    context.append(f'</{tag}>\n')
    if name: node.insert(0, nodes.title(name, admonitionlabels[name]))   # MUTATES the tree
depart_admonition: append(context.pop())
```

`visit_note/warning/attention/caution/danger/error/hint/important/tip` → `visit_admonition(node, '<name>')`;
`visit_seealso` → `visit_admonition(node, 'seealso')`; generic `admonition` node →
`visit_admonition(node)` (name `''`, the directive already supplied the title child).
Labels (`SPHINX/locale/__init__.py:228-239`, translated): Attention, Caution, Danger, Error,
Hint, Important, Note, **See also**, Tip, Warning. Probe:

```html
<div class="admonition note">
<p class="admonition-title">Note</p>
<p>A note.</p>
</div>
<div class="admonition-custom-title admonition">
<p class="admonition-title">Custom Title</p>
<details class="admonition note" open="open">
<summary class="admonition-title">Note</summary>
<details class="admonition note">            (:collapsible: closed)
<div class="extra admonition important" id="imp-id">
<div class="cls admonition seealso">
```

(`:collapsible:` with no value → `'open'`.)

### 4.3 versionmodified (sx 317-321)

`starttag(node,'div',CLASS=node['type'])` / `</div>\n`. The inner span and text come from the
directive (read side):

```html
<div class="versionadded">
<p><span class="versionmodified added">Added in version 1.0.</span></p>
</div>
<div class="versionchanged">
<p><span class="versionmodified changed">Changed in version 1.0: </span>Multi paragraph first.</p>
<p>Second paragraph.</p>
</div>
```

(`deprecated` → `<div class="deprecated">`, `versionremoved` → `<div class="versionremoved">`.)

### 4.4 Lists and the "simple" algorithm

- `bullet_list` (base 756-768; sx 469-473 adds the toctree skip): push `(compact_simple,
  compact_p)`; `compact_p = None`; `compact_simple = is_compactable(node)`; add `class="simple"`
  **only if `compact_simple and not old_compact_simple`** (a simple list nested in a simple
  list gets no class). `<ul class="simple">\n` / `</ul>\n`, pop state.
- `enumerated_list` (base 1014-1025): classes kwarg `[enumtype]` + `['simple']` if compactable
  (no nested suppression), `start` attr if present: `<ol class="arabic simple" start="3">`,
  `<ol class="loweralpha simple">`.
- `list_item` (base 1272-1276): `starttag(node,'li','')` … `</li>\n` (toctree items:
  `<li class="toctree-l1">`).
- `is_compactable(node)` (base 737-754): `'compact'` in classes → True; `'open'` → False;
  field/definition list and not `compact_field_lists` → False; bullet/enumerated and not
  `compact_lists` → False; `'contents'` in **parent** classes → True; else
  `check_simple_list(node)`.
- `SimpleListChecker` (base 1820-1895, a `GenericNodeVisitor`): walk the list; **any node
  type not listed below raises NodeFound → not simple** (this includes Sphinx nodes such as
  `compact_paragraph`, `index`, `literal_block`, `desc`, …). Allowed:
  - `Text`, `paragraph` (exact class — NOT compact_paragraph), `author`, `copyright`, `date`,
    `organization`, `status`, `term`, `field_name`, `comment`, `substitution_definition`,
    `target`, `pending`: **skip the node and its children** (inline content never inspected).
  - `bullet_list`, `enumerated_list`, `docinfo`, `definition_list`,
    `definition_list_item`, `classifier`, `field_list`, `field`, `contact`: pass (descend).
  - `list_item`, `definition`, `field_body`, `authors`, `address`, `version`: children minus
    `Invisible` ones; if first is `paragraph` and last is bullet/enumerated/field list, drop
    the last; then `len <= 1` → OK (descend), else NodeFound.
  Consequences seen in probes: toctree `<ul>` never has `simple` (compact_paragraph);
  a glossary with one-paragraph definitions → `<dl class="simple glossary">`, with a
  two-paragraph definition → `<dl class="glossary">`; local contents lists →
  `<ul class="simple">` via the `contents` parent rule; nested
  `<ul class="simple">…<ul>…<ol class="arabic simple">` (probe 4).
- `definition_list` (base 889-900): `details` class → `<div class="details">` (+ `open`), else
  `starttag(node,'dl',classes=['simple'] if compactable)` / `</dl>\n`.
- `definition_list_item` (base 903-912): only with parent class `details` →
  `<details>` / `<details open="open">` … `</details>\n`; otherwise nothing.
- `term` (sx 498-513): `starttag(node,'dt','')` (uses the term's OWN ids/classes; docutils
  would use the parent item's) — **no newline after `</dt>`**. `depart_term`: if the next
  sibling is a `classifier` do nothing; else if `term.parent.parent.parent` is `glossary` →
  permalink `Link to this term`; then `</dt>`.
- `classifier` (sx 485-495): `<span class="classifier">` … `</span>` then `</dt>` if the next
  sibling is not a classifier (the glossary permalink is NOT added in this path).
- `definition` (sx 476-482): `starttag(node,'dd','')` / `</dd>\n` (no `details` special case —
  so `details` lists produce `<details>\n<dt>term</dt><dd>…</dd>\n</details>\n`, probe 5).

```html
<dl class="simple">
<dt>term</dt><dd><p>def</p>
</dd>
<dt>term2<span class="classifier">classifier</span></dt><dd><p>def2</p>
</dd>
<dt>term3<span class="classifier">c1</span><span class="classifier">c2</span></dt><dd><p>def3</p>
</dd>
</dl>
<dl class="simple glossary">
<dt id="term-Apple">Apple<a class="headerlink" href="#term-Apple" title="Link to this term">¶</a></dt><dd><p>A fruit.</p>
</dd>
<dt id="term-Banana">Banana<a class="headerlink" href="#term-Banana" title="Link to this term">¶</a></dt><dt id="term-Plantain">Plantain<a class="headerlink" href="#term-Plantain" title="Link to this term">¶</a></dt><dd><p>Yellow.</p>
</dd>
</dl>
```

### 4.5 Field lists (sx 979-992; base 1027-1074)

- `field_list`: Sphinx pushes a row counter then base: `field-indent-<len>` class → removed and
  `style="--field-indent: <len>;"`; append `field-list` class; `simple` if compactable;
  `starttag(node,'dl',**atts)` → `<dl class="field-list simple">` /
  `<dl class="field-list simple" style="--field-indent: 4em;">`; depart `</dl>\n`.
- `field` (Sphinx override, does NOT call base): counter += 1 and **appends `field-odd` /
  `field-even` to the field node's classes** (1-based: first field odd). (Docutils' transfer of
  field ids onto field_name is therefore skipped.)
- `field_name`: `starttag(node,'dt','',classes=field['classes'])` … `<span class="colon">:</span></dt>\n`.
- `field_body`: `starttag(node,'dd','',classes=field['classes'])`, and `<p></p>` if it has no
  children; `</dd>\n`.

```html
<dl class="field-list simple">
<dt class="field-odd">empty field<span class="colon">:</span></dt>
<dd class="field-odd"><p></p></dd>
<dt class="field-even">other<span class="colon">:</span></dt>
<dd class="field-even"><p>x</p>
</dd>
</dl>
```

### 4.6 Option lists (base 1419-1459)

`<dl class="option-list">\n`; `option_group` → `<dt><kbd>` … `</kbd></dt>\n`; `option` →
`<span class="option">` … `</span>` (+ `, ` before a following option); `option_argument` →
delimiter (default `' '`) + `<var>` … `</var>`; `description` → `<dd>` … `</dd>\n`.
`<dt><kbd><span class="option">-b <var>FILE</var></span>, <span class="option">--bfile=<var>FILE</var></span></kbd></dt>`.

### 4.7 Tables (sx 948-977; base 815-835, 991-1012, 1666-1730)

- `visit_table` (sx): push row counter 0; `classes = [c.strip(' \t\n') for c in
  table_style.split(',')]`, insert `docutils` at 0, append `align-<align or 'default'>`;
  `style = 'width: %s' % width` if `width` (**no `px` added, no trailing `;`** — unlike
  docutils); `starttag(node,'table',CLASS=' '.join(classes), **atts)` (node classes first,
  `colwidths-*` filtered). `depart_table`: pop counter, base → `</table>\n` + report_messages.
- `tgroup` resets `colspecs`; `colspec` collects; after the last colspec emit
  `<colgroup>\n<col style="width: 30.0%" />\n…</colgroup>\n` **only if** the table has class
  `colwidths-given` (or `table_style` contains `colwidths-grid`) and not `colwidths-auto`;
  width = `propwidth/total` formatted `{:.1%}`.
- `thead`/`tbody`: `<thead>\n` / `</thead>\n`, `<tbody>\n` / `</tbody>\n`.
- `row` (sx): counter += 1 across thead **and** tbody of the same table; appends `row-odd`
  (odd) / `row-even` to the row's classes; `starttag(node,'tr','')`; depart base `</tr>\n`.
- `entry` (base): classes `head` (in thead) and/or `stub` (stub column) → `th`, else `td`;
  `rowspan = morerows+1`, `colspan = morecols+1`; `starttag(node, tag, '', **atts)`; close
  `</td>\n`/`</th>\n`.
- Table title → `<caption>…</caption>\n` (§3.2), emitted before colgroup.

```html
<table class="docutils align-center" id="tab-one">
<caption><span class="caption-number">Table 1 </span><span class="caption-text">Caption Table</span><a class="headerlink" href="#tab-one" title="Link to this table">¶</a></caption>
<colgroup>
<col style="width: 30.0%" />
<col style="width: 70.0%" />
</colgroup>
<thead>
<tr class="row-odd"><th class="head"><p>X</p></th>
<th class="head"><p>Y</p></th>
</tr>
</thead>
<tbody>
<tr class="row-even"><td><p>a</p></td>
<td><p>b</p></td>
</tr>
</tbody>
</table>
<table class="longtable docutils align-default" id="id1">        (list-table :class: longtable, :widths: auto)
<tr class="row-odd"><th class="head stub"><p>H1</p></th>
<tr class="row-even"><th class="stub"><p>s1</p></th>
<table class="docutils align-default" id="id2" style="width: 50%">   (csv-table :width: 50%)
<table class="docutils align-default" style="width: 300">             (table :width: 300)
<tr class="row-even"><td colspan="2"><p>span</p></td>
<tr class="row-odd"><td><p>one</p></td>
<td></td>                                                             (empty cell)
```

### 4.8 Footnotes / citations (base 777-806, 1100-1126, 1229-1250; sx 1026-1034)

- `footnote`: before the first of a run of consecutive footnote siblings
  `<aside class="footnote-list brackets">\n`; each footnote
  `starttag(node,'aside',classes=['footnote','brackets'],role='doc-footnote')`; depart
  `</aside>\n` plus a closing `</aside>\n` after the last of the run.
- `citation`: same grouping with `<div role="list" class="citation-list">\n` and
  `starttag(node,'div',classes=['citation'],role='doc-biblioentry')`.
- `label`: `<span class="label"><span class="fn-bracket">[</span>` + (one backref:
  `<a role="doc-backlink" href="#REF">`) + text + (`</a>`) + `<span class="fn-bracket">]</span></span>\n`
  and, with >1 backrefs, `<span class="backrefs">(<a role="doc-backlink" href="#id1">1</a>,<a role="doc-backlink" href="#id2">2</a>)</span>\n`.
- `footnote_reference` (Sphinx override, re-adds `footnote-reference`):
  `starttag(node,'a',suffix='',classes=['footnote-reference','brackets'],role='doc-noteref',href='#'+refid)`
  + `<span class="fn-bracket">[</span>`; depart (base) `<span class="fn-bracket">]</span></a>`.
- `citation_reference` (base): normally never reaches the writer — Sphinx turns citation
  references into `pending_xref` → resolved `reference` + `inline` (`<a class="reference internal" href="#cit2002" id="id5"><span>[CIT2002]</span></a>`).

```html
<a class="footnote-reference brackets" href="#f1" id="id2" role="doc-noteref"><span class="fn-bracket">[</span>2<span class="fn-bracket">]</span></a>
<aside class="footnote-list brackets">
<aside class="footnote brackets" id="f1" role="doc-footnote">
<span class="label"><span class="fn-bracket">[</span><a role="doc-backlink" href="#id2">2</a><span class="fn-bracket">]</span></span>
<p>First footnote.</p>
</aside>
<aside class="footnote brackets" id="a" role="doc-footnote">
<span class="label"><span class="fn-bracket">[</span>1<span class="fn-bracket">]</span></span>
<span class="backrefs">(<a role="doc-backlink" href="#id1">1</a>,<a role="doc-backlink" href="#id2">2</a>)</span>
<p>Multi-ref footnote.</p>
</aside>
</aside>
<div role="list" class="citation-list">
<div class="citation" id="cit2002" role="doc-biblioentry">
<span class="label"><span class="fn-bracket">[</span><a role="doc-backlink" href="#id5">CIT2002</a><span class="fn-bracket">]</span></span>
<p>A citation.</p>
</div>
</div>
```

### 4.9 system_message (base 1637-1664)

Only present in the tree when `keep_warnings=True` (Sphinx's `FilterSystemMessages`,
`SPHINX/transforms/__init__.py:337`, otherwise strips everything below level 5 at read time),
or when the *writer* itself creates one (§10). Output (probe 4):

```html
<aside class="system-message">
<p class="system-message-title">System Message: ERROR/3 (<span class="docutils literal">/abs/path/index.rst</span>, line 7)</p>
<p>Unknown directive type “unknowndir”.</p>
<div class="highlight-default notranslate"><div class="highlight"><pre><span></span><span class="o">..</span> <span class="n">unknowndir</span><span class="p">::</span> <span class="n">arg</span>

   <span class="n">body</span>
</pre></div>
</div>
</aside>
```

Title: `System Message: %s/%s (<span class="docutils literal">%s</span>%s)%s</p>\n` with
type, level, `encode(source)`, `', line N'` if the node has `line`, backref text (`; <em><a
href="#ID">backlink</a></em>` or `; <em>backlinks: <a href="#a">1</a>, …</em>`). Note the
source is the **absolute path** (oracle must normalise it), and the embedded
`literal_block` is highlighted as `default` (its rawsource equals its text).

`problematic` (base 1492-1502): with `refid` → `<a href="#REFID"><span class="problematic" id="ID">…</span></a>`.

---

## 5. Literal blocks, captions, highlighting glue

### 5.1 `visit_literal_block` (sx 604-630)

```
if node.rawsource != node.astext():          # parsed-literal (has inline markup)
    return base.visit_literal_block(node)    # '<pre class="literal-block">' … '</pre>\n' (+ <code> if 'code' class)
lang = node.get('language', 'default')
linenos = node.get('linenos', False)
highlight_args = node.get('highlight_args', {}); highlight_args['force'] = node.get('force', False)
opts = config.highlight_options.get(lang, {})
if linenos and config.html_codeblock_linenos_style: linenos = config.html_codeblock_linenos_style   # 'inline'
highlighted = highlighter.highlight_block(node.rawsource, lang, opts=opts, linenos=linenos, location=node, **highlight_args)
starttag = self.starttag(node, 'div', suffix='', CLASS='highlight-%s notranslate' % lang)
append(starttag + highlighted + '</div>\n'); raise SkipNode
```

- `highlight_args` from the directive (`SPHINX/directives/code.py`): `hl_lines`
  (from `:emphasize-lines:`), `linenostart` (`:lineno-start:`/`:lineno-match:`). They are
  passed straight to the Pygments formatter (§9).
- Class is `highlight-<lang as written>` even when the lexer fell back: `highlight-nosuchlang`,
  `highlight-guess`, `highlight-default` (for `::` blocks and doctest blocks),
  `highlight-Python` for `.. code-block:: Python`.
- Node classes first, node ids on the div: `<div class="extra-cls highlight-python notranslate" id="named-block">`,
  `<div class="doctest highlight-default notranslate">` (`doctest` class added by
  `DoctestTransform`, `SPHINX/transforms/__init__.py:327`).
- **Parsed-literal detection needs `rawsource`.** Crate nodes do not store it
  (`src/doctree/mod.rs` `Node`; `run_parsed_literal` `src/rst/block.rs:6927-6954` discards the
  raw text). A parsed-literal whose inline parse yields exactly its raw text (no markup, no
  escapes) IS highlighted by Sphinx (probe: `.. parsed-literal::\n\n   parsed without markup`
  → `<div class="highlight-python notranslate">…<span class="n">parsed</span> …`). The crate
  must record either the raw text or a boolean "raw differs from text" on parsed-literal
  `literal_block`s (a non-pformat side field), and treat every other literal_block
  (`::`, code-block, code, sourcecode, literalinclude, doctest_block, system-message
  literal) as highlightable. `TrimDoctestFlagsTransform` rewrites rawsource and text together.
- Parsed-literal output: `<pre class="literal-block">parsed <em>emph</em> literal</pre>\n`.
- `doctest_block` (sx 665-666) → `visit_literal_block` (no `language` attribute is ever stamped
  on doctest blocks, so always `default` → pycon lexer via the `>>>` rule).

### 5.2 Captions and the code-block wrapper (sx 632-663; h5 154-161)

```
visit_caption:
    if parent is container and parent.get('literal_block'): append('<div class="code-block-caption">')
    else: base.visit_caption          # h5: figure → starttag(node,'figcaption') ('<figcaption>\n') then '<p>'
    add_fignumber(node.parent)
    append(starttag(node,'span','',CLASS='caption-text'))
depart_caption:
    append('</span>')
    if code-block container: add_permalink_ref(parent, 'Link to this code')
    elif parent is figure:   add_permalink_ref(parent, 'Link to this image')
    elif parent.get('toctree'): add_permalink_ref(parent.parent, 'Link to this toctree')
    if code-block container: append('</div>\n') else base.depart_caption  # '</p>\n'
```

```html
<div class="literal-block-wrapper docutils container" id="code-one">
<div class="code-block-caption"><span class="caption-number">Listing 1 </span><span class="caption-text">Captioned <em>code</em></span><a class="headerlink" href="#code-one" title="Link to this code">¶</a></div>
<div class="highlight-python notranslate"><div class="highlight"><pre><span></span><span class="linenos">5</span><span class="n">a</span> <span class="o">=</span> <span class="mi">1</span>
<span class="hll"><span class="linenos">6</span><span class="n">b</span> <span class="o">=</span> <span class="mi">2</span>
</span><span class="linenos">7</span><span class="n">c</span> <span class="o">=</span> <span class="mi">3</span>
</pre></div>
</div>
</div>
```

(`literalinclude` with empty `:caption:` → caption text is the file name: `<span class="caption-text">example.py</span>`.)

### 5.3 Literal blocks in lists / cells

Nothing special: `<li><div class="highlight-text notranslate"><div class="highlight"><pre><span></span>in list\n</pre></div>\n</div>\n</li>\n`,
`<td><div class="highlight-text notranslate">…</div>\n</td>\n`.

---

## 6. Inline nodes

### 6.1 literal / kbd / code (sx 676-708)

```
visit_literal:
  if 'kbd' in classes: append(starttag(node,'kbd','',CLASS='docutils literal notranslate')); return   # NOT protected
  lang = node.get('language')
  if 'code' not in classes or not lang:
      append(starttag(node,'code','',CLASS='docutils literal notranslate')); protect_literal_text += 1; return
  highlighted = highlighter.highlight_block(node.astext(), lang, opts=highlight_options.get(lang,{}), location=node, nowrap=True)
  append(starttag(node,'code',suffix='',CLASS='docutils literal highlight highlight-%s' % lang) + highlighted.strip() + '</code>')
  raise SkipNode
depart_literal: kbd → '</kbd>'; else protect -= 1, '</code>'
```

Probe outputs:

```html
<code class="docutils literal notranslate"><span class="pre">lit</span>&#160; <span class="pre">eral</span> <span class="pre">--opt</span></code>
<code class="code docutils literal notranslate"><span class="pre">x</span> <span class="pre">=</span> <span class="pre">1</span></code>          (:code: without language)
<code class="code highlight python docutils literal highlight-python"><span class="nb">print</span><span class="p">(</span><span class="s2">"hi"</span><span class="p">,</span> <span class="mi">1</span><span class="p">)</span></code>
<kbd class="kbd docutils literal notranslate">Ctrl</kbd>+<kbd class="kbd docutils literal notranslate">C</kbd>
<code class="file docutils literal notranslate"><span class="pre">/usr/</span><em><span class="pre">name</span></em><span class="pre">/x</span></code>
<code class="xref py py-func docutils literal notranslate"><span class="pre">func()</span></code>
<code class="code highlight json docutils literal highlight-json"><span class="p">{</span><span class="err">broke</span><span class="kc">n</span><span class="p">}</span></code>   (after "Lexing literal_block '{broken}' as "json"…" warning)
```

Highlighted inline code has **no `notranslate`**, no `<span></span>`, no trailing `\n`
(`nowrap=True` + `.strip()` — note strip also eats leading/trailing whitespace of the source).
A custom role based on `literal` with class `kbd` → `<kbd …>` too (probe 2).

### 6.2 Other inline nodes

| node | visitor | output |
|---|---|---|
| emphasis | base 985 | `<em>` … `</em>` (`<em class="dfn">thing</em>`, `<em class="mailheader">`) |
| strong | base 1589 | `<strong>` (`<strong class="command">rm</strong>`, `program`, `makevar`) |
| literal_emphasis | sx 916-920 | = emphasis |
| literal_strong | sx 922-926 | = strong |
| manpage | sx 940-944 | = literal_emphasis: `<em class="manpage">ls(1)</em>`; with `manpages_url`: `<em class="manpage"><a class="manpage reference external" href="https://man.example/ls.1">ls(1)</a></em>` |
| abbreviation | sx 928-938 | `starttag(node,'abbr','',title=explanation)` if explanation: `<abbr title="last-in, first-out">LIFO</abbr>`, `<abbr>XYZ</abbr>` |
| acronym | h5 135 | `<abbr>` |
| inline | h5 249-277 with `supported_inline_tags = set()` (sx 53) | always `<span class="…">` (custom role `del` → `<span class="del">gone</span>`, menuselection `<span class="menuselection">File ‣ Open</span>`, guilabel `<span class="guilabel"><span class="accelerator">C</span>ancel</span>`, xref fallbacks `<span class="xref std std-ref">…</span>`, desc_sig_*) |
| title_reference | base 1791 | `<cite>` |
| subscript / superscript | base 1595/1631 | `<sub>` / `<sup>` |
| generated | base 1129 | only `sectnum` class special (§3.4); otherwise transparent |
| problematic | base 1492 | §4.9 |
| target | base 1682 | §4.1 |
| math | sx 994 → mathjax | §7.9 |
| image (inline) | §7.6 | `<img alt="img" src="_images/sub_img.png" />` (no newline in TextElement) |

`visit_literal` of the base (the `words_and_spaces`/`in_word_wrap_point` logic,
`h5 290-322`) is **never used** by Sphinx — Sphinx's override replaces it entirely.

---

## 7. References, images, math

### 7.1 reference (sx 324-360; depart = base 1539-1543)

```
atts = {'class': 'reference'}
if node.get('internal') or 'refuri' not in node: atts['class'] += ' internal'
else:                                             atts['class'] += ' external'
if 'refuri' in node:
    atts['href'] = node['refuri'] or '#'
    if cloak_email_addresses and href.startswith('mailto:'):
        atts['href'] = cloak_mailto(href)   # '@' → '%40'
        self.in_mailto = True                # BEFORE starttag → attval cloaks every attribute
else:
    atts['href'] = '#' + node['refid']
if not isinstance(node.parent, TextElement):   # must be exactly one image child (assert)
    atts['class'] += ' image-reference'
if 'reftitle' in node: atts['title'] = node['reftitle']
if 'target' in node:   atts['target'] = node['target']
if 'rel' in node:      atts['rel'] = node['rel']
append(starttag(node, 'a', '', **atts))          # CLASS string → node classes FIRST ("pep reference external")
if node.get('secnumber'): append(('%s' + secnumber_suffix) % '.'.join(map(str, secnumber)))
depart: '</a>' + ('\n' if parent not TextElement) ; in_mailto = False
```

```html
<a class="reference external" href="https://example.org/">named</a>
<a class="reference internal" href="#inline">Inline</a>
<a class="reference internal" href="blocks.html"><span class="doc">Blocks</span></a>
<a class="reference internal" href="#mod.func" title="mod.func"><code class="xref py py-func docutils literal notranslate"><span class="pre">func()</span></code></a>
<a class="pep reference external" href="https://peps.python.org/pep-0008/"><strong>PEP 8</strong></a>
<a class="reference external" href="mailto:someone&#37;&#52;&#48;example&#46;com">someone<span>&#64;</span>example<span>&#46;</span>com</a>
<a class="reference external" href="mailto:x&#37;&#52;&#48;y&#46;org">mail</a>
<a class="reference internal image-reference" href="_images/img.png"><img … />
</a>
```

`number_reference` (sx 362-366) = reference: `<a class="reference internal" href="media.html#fig-one"><span class="std std-numref">Fig. 1</span></a>`.
Math `:eq:` → `<a class="reference internal" href="blocks.html#equation-euler">(1)</a>`.

### 7.2 download_reference (sx 734-761)

```
atts = {'class': 'reference download', 'download': ''}
if not builder.download_support: context.append('')
elif 'refuri' in node:   class += ' external'; href = refuri → '<a …>' ; context '</a>'
elif 'filename' in node: class += ' internal'; href = posixpath.join(builder.dlpath, urllib.parse.quote(node['filename']))
else: context ''
depart: append(context.pop())
```

`<a class="reference download internal" download="" href="_downloads/70f5b20d30a0cea2b1b573401b61bcfc/data.txt"><code class="xref download docutils literal notranslate"><span class="pre">data</span></code></a>`;
from `sub/page.rst`: `href="../_downloads/fb0d0d7ff27f33697ba3b787ec894677/conf.py"`.
`node['filename']` = `<md5 hex of the posix source-relative path>/<basename>` (read-time
`DownloadFileCollector`). `quote()` uses `safe='/'` (space → `%20`, non-ASCII → UTF-8 %XX).

(§7.3–7.5 intentionally unused; titles/permalinks are in §3.)

### 7.6 image (sx 771-803; base 419-453, 1153-1217)

```
olduri = node['uri']
if olduri in builder.images: node['uri'] = posixpath.join(builder.imgpath, urllib.parse.quote(builder.images[olduri]))
if 'scale' in node and not ('width' in node and 'height' in node):
    size = get_image_size(builder.srcdir / olduri)          # imagesize lib; SPHINX/util/images.py:41
    if size is None: warning('Could not obtain image size. :scale: option is ignored.', location=node)
    else: node.setdefault('width', str(size[0])); node.setdefault('height', str(size[1]))
base.visit_image(node)
depart_image: nothing (the svg special case is a no-op in 0.22)
```

Base `visit_image`: `alt = node.get('alt', uri)` — **the rewritten, relative URI**
(`alt="_images/img.png"`, from a subdirectory `alt="../_images/img.png"`); `atts =
image_size(node)`: for `width`/`height` present → `parse_measure` (value, unit); if `scale`
and fewer than two measures, docutils tries PIL (`read_size_with_PIL`); multiply every
value by `scale/100`; unit present → `style` declaration `'{dim}: {value:g}{unit};'`
(joined with a space), unitless → attribute `str(round(value))`. `align` → classes
`[f'align-{align}']`; `loading` only if `lazy`. Suffix `\n` unless the parent is a
TextElement (a `reference` parent counts as block when *its* parent is not a TextElement).
Then `emptytag(node,'img',suffix,src=uri,alt=alt,**atts)`, then `report_messages` if block.

```html
<img alt="_images/img.png" src="_images/img.png" />
<a class="reference internal image-reference" href="_images/img.png"><img alt="Alt text" class="align-center" src="_images/img.png" style="width: 100px;" />
</a>
<a class="reference internal image-reference" href="_images/img.png"><img alt="_images/img.png" height="10" src="_images/img.png" width="20" />
</a>                                                           (:scale: 50% of a 40x20 png)
<a class="reference external image-reference" href="https://example.com"><img alt="_images/img.png" src="_images/img.png" />
</a>                                                           (:target:)
<img alt="_images/pic.svg" src="_images/pic.svg" />
<a class="reference internal image-reference" href="_images/pic.svg"><img alt="_images/pic.svg" src="_images/pic.svg" width="50" />
</a>
<img alt="https://example.com/remote.png" src="https://example.com/remote.png" />
<img alt="_images/img.png" class="no-scaled-link" height="30" src="_images/img.png" />
<img alt="missing.png" src="missing.png" />                  (not in builder.images → not rewritten)
<img alt="nonexistent.svg" class="a b" src="nonexistent.svg" />
```

### 7.7 Scaled image link (builder, before the translator)

`post_process_images` (`SPHINX/builders/html/__init__.py:961-987`), when
`html_scaled_image_link` (default True): for each `image` having any of `scale`/`width`/
`height`, whose parent is not a `reference`, and without class `no-scaled-link`: replace it
by `reference('', '', internal=True, refuri=imgpath/images[uri] or uri)` containing the
image. Hence `class="reference internal image-reference"` even for remote images
(`href="https://example.com/r.png"`). With `html_scaled_image_link = False` (probe 3):
`<img alt="_images/img.png" src="_images/img.png" style="width: 10px;" />` unwrapped.

### 7.8 figure (sx 764-768; h5 204-215, 154-161, 280-287)

`visit_figure`: `node.setdefault('align','default')` then h5: `style="width: X"` from
`width` (figwidth), `class="align-X"`; `starttag(node,'figure',**atts)`. Caption →
`<figcaption>\n<p>` + number + `<span class="caption-text">…</span>` + permalink + `</p>\n`;
legend → (`<figcaption>\n` if no caption before) `<div class="legend">\n…</div>\n`; depart:
`</figcaption>\n` if `len(node) > 1`, then `</figure>\n`.

```html
<figure class="align-center" id="id1">
<span id="fig-one"></span><a class="reference internal image-reference" href="_images/img.png"><img alt="_images/img.png" src="_images/img.png" style="width: 200px;" />
</a>
<figcaption>
<p><span class="caption-number">Fig. 1 </span><span class="caption-text">Figure <em>caption</em>.</span><a class="headerlink" href="#id1" title="Link to this image">¶</a></p>
<div class="legend">
<p>Legend text.</p>
</div>
</figcaption>
</figure>
<figure class="align-default" id="fig-noleg" style="width: 50%">
<img alt="_images/sub_img.png" src="_images/sub_img.png" />
</figure>
```

### 7.9 math (sx 994-1022 → `SPHINX/ext/mathjax.py:36-78`)

Sphinx sets `_has_maths_elements = True` on any math/math_block and dispatches to
`html_inline_math_renderers[builder.math_renderer_name]` — `mathjax` (always loaded,
`SPHINX/builders/html/__init__.py:1549`).

Inline (`html_visit_math`): `starttag(node,'span','',CLASS='math notranslate nohighlight')`
+ `mathjax_inline[0] + encode(astext()) + mathjax_inline[1] + '</span>'`; SkipNode.
→ `<span class="math notranslate nohighlight">\(a^2 + b^2 = c^2\)</span>`.

Block (`html_visit_displaymath`):

```
append(starttag(node,'div',CLASS='math notranslate nohighlight'))        # suffix '\n'
if node.get('no-wrap', node.get('nowrap', False)):
    append(encode(astext())); append('</div>'); SkipNode                  # NO trailing newline
if node['number']:
    number = get_node_equation_number(self, node)   # SPHINX/util/math.py:13-27
    append('<span class="eqno">(%s)' % number); add_permalink_ref(node, 'Link to this equation'); append('</span>')
append(mathjax_display[0])
parts = [p for p in astext().split('\n\n') if p.strip()]
if len(parts) > 1: append(r' \begin{align}\begin{aligned}')
for i, part in enumerate(parts):
    part = encode(part)
    append(r'\begin{split}' + part + r'\end{split}' if r'\\' in part else part)
    if i < len(parts) - 1: append(r'\\')
if len(parts) > 1: append(r'\end{aligned}\end{align} ')
append(mathjax_display[1]); append('</div>\n'); SkipNode
```

`get_node_equation_number`: if `math_numfig and numfig`: `'.'.join(fignumbers['displaymath'][id])`
with the last `.` replaced by `math_numsep`; else `node['number']`.

```html
<div class="math notranslate nohighlight">
\[e^{i\pi} + 1 = 0\]</div>
<div class="math notranslate nohighlight" id="equation-euler">
<span class="eqno">(1)<a class="headerlink" href="#equation-euler" title="Link to this equation">¶</a></span>\[e^{i\pi} + 1 = 0\]</div>
<div class="math notranslate nohighlight">
\[ \begin{align}\begin{aligned}a = b\\\begin{split}c = d \\ e = f\end{split}\end{aligned}\end{align} \]</div>
<div class="math notranslate nohighlight">
\begin{equation} x \end{equation}</div><nav class="contents local" id="local-toc">
```

Page-level consequence (template side): `install_mathjax` adds the MathJax 4 script
(`https://cdn.jsdelivr.net/npm/mathjax@4/tex-mml-chtml.js`, `defer`) only when
`has_maths_elements` (default `html_assets_policy`).

---

## 8. Object descriptions (desc family) — sx 84-313

State fields (sx 55-67 + set in `_visit_sig_parameter_list`): `protect_literal_text`,
`param_separator`, `optional_param_level`, `required_params_left`, `is_first_param`,
`params_left_at_level`, `param_group_index`, `list_is_required_param`,
`multi_line_parameter_list`, `trailing_comma`, `max_optional_param_level`.

| node | visit | depart |
|---|---|---|
| desc | `starttag(node,'dl')` → `<dl class="py function">\n` (classes `[domain, objtype]` from read side; `describe`/`object` have no domain → `<dl class="describe">`) | `</dl>\n\n` |
| desc_signature | `starttag(node,'dt')` → `<dt class="sig sig-object py" id="mod.func">\n` (+ extra-id spans after the `\n`); `protect_literal_text += 1` | `protect -= 1`; if not `is_multiline`: permalink `Link to this definition`; `</dt>\n` |
| desc_signature_line | nothing | if `add_permalink`: permalink of `node.parent`; then `<br />` |
| desc_content | `starttag(node,'dd','')` → `<dd>` | `</dd>` (no newline) |
| desc_inline | `starttag(node,'span','')` → `<span class="cpp-expr sig sig-inline cpp">` (NOT protected) | `</span>` |
| desc_name | `<span class="sig-name descname">` | `</span>` |
| desc_addname | `<span class="sig-prename descclassname">` | `</span>` |
| desc_type | nothing | nothing |
| desc_returns | `' <span class="sig-return">'` + `'<span class="sig-return-icon">&#x2192;</span>'` + `' <span class="sig-return-typehint">'` | `</span></span>` |
| desc_annotation | `starttag(node,'span','',CLASS='property')` → `<span class="property">` | `</span>` |
| desc_parameterlist | `_visit_sig_parameter_list(node, desc_parameter, '(', ')')` | `_depart_sig_parameter_list` |
| desc_type_parameter_list | `_visit_sig_parameter_list(node, desc_type_parameter, '[', ']')` | same |
| desc_parameter / desc_type_parameter | algorithm below | algorithm below |
| desc_optional | algorithm below | algorithm below |
| desc_sig_* | via `visit_inline` → `<span class="n">` etc. | `</span>` |

Everything inside a `desc_signature` is protected: `<span class="pre">mod.</span>`,
spaces bare, `@` → `<span class="pre">&#64;</span>`, `<span class="pre">'&lt;x&gt;'</span>`.

### 8.1 Parameter-list algorithm (verbatim semantics)

```
_visit_sig_parameter_list(node, group_cls, open, close):
    append(f'<span class="sig-paren">{open}</span>')
    is_first_param = True; optional_param_level = 0; params_left_at_level = 0; param_group_index = 0
    list_is_required_param = [isinstance(c, group_cls) for c in node.children]
    required_params_left = sum(list_is_required_param)
    param_separator = node.child_text_separator          # ', ' for both list classes
    multi_line_parameter_list = node.get('multi_line_parameter_list', False)
    trailing_comma = node.get('multi_line_trailing_comma', False)
    if multi_line_parameter_list:
        append('\n\n'); append(starttag(node,'dl'))       # '<dl>\n'
        param_separator = param_separator.rstrip()       # ','
    context.append(close)
_depart_sig_parameter_list:
    if node.get('multi_line_parameter_list'): append('</dl>\n\n')
    append(f'<span class="sig-paren">{context.pop()}</span>')

visit_desc_parameter:
    on_sep = multi_line_parameter_list
    if on_sep and not (is_first_param and optional_param_level > 0): append(starttag(node,'dd',''))
    if is_first_param: is_first_param = False
    elif not on_sep and not required_params_left: append(param_separator)
    if optional_param_level == 0: required_params_left -= 1
    else: params_left_at_level -= 1
    if not node.hasattr('noemph'): append('<em class="sig-param">')
depart_desc_parameter:
    if not node.hasattr('noemph'): append('</em>')
    is_required = list_is_required_param[param_group_index]
    if multi_line_parameter_list:
        is_last_group = param_group_index + 1 == len(list_is_required_param)
        next_is_required = not is_last_group and list_is_required_param[param_group_index + 1]
        opt_left = params_left_at_level > 0
        if opt_left or is_required and (is_last_group or next_is_required):
            if not is_last_group or opt_left or trailing_comma: append(param_separator)
            append('</dd>\n')
    elif required_params_left: append(param_separator)
    if is_required: param_group_index += 1

visit_desc_optional:
    params_left_at_level = count of desc_parameter children
    optional_param_level += 1; max_optional_param_level = optional_param_level
    if multi_line_parameter_list:
        if is_first_param:          append(starttag(node,'dd','')); append('<span class="optional">[</span>')
        elif required_params_left:  append(param_separator); append('<span class="optional">[</span>'); append('</dd>\n')
        else:                       append('<span class="optional">[</span>'); append(param_separator); append('</dd>\n')
    else: append('<span class="optional">[</span>')
depart_desc_optional:
    optional_param_level -= 1; level = optional_param_level
    if multi_line_parameter_list:
        is_last_group = param_group_index + 1 == len(list_is_required_param)
        if level == max_optional_param_level - 1 and (not is_last_group or level > 0 or trailing_comma):
            append(param_separator)
        append('<span class="optional">]</span>')
        if level == 0: append('</dd>\n')
    else: append('<span class="optional">]</span>')
    if level == 0: param_group_index += 1
```

Note the separator is appended raw (never through `visit_Text`, so no `pre` span) and the
state lives on the translator (not a stack) — type-param list and param list are
processed sequentially and each resets it.

### 8.2 Probe outputs

Single line (probe 1):

```html
<dl class="py function">
<dt class="sig sig-object py" id="mod.func">
<span class="property"><span class="k"><span class="pre">async</span></span><span class="w"> </span></span><span class="sig-prename descclassname"><span class="pre">mod.</span></span><span class="sig-name descname"><span class="pre">func</span></span><span class="sig-paren">(</span><em class="sig-param"><span class="n"><span class="pre">a</span></span></em>, <em class="sig-param"><span class="n"><span class="pre">b</span></span><span class="p"><span class="pre">:</span></span><span class="w"> </span><span class="n"><span class="pre">int</span></span><span class="w"> </span><span class="o"><span class="pre">=</span></span><span class="w"> </span><span class="default_value"><span class="pre">1</span></span></em>, <em class="sig-param"><span class="o"><span class="pre">*</span></span><span class="n"><span class="pre">args</span></span></em>, <em class="sig-param"><span class="n"><span class="pre">c</span></span><span class="o"><span class="pre">=</span></span><span class="default_value"><span class="pre">None</span></span></em>, <em class="sig-param"><span class="o"><span class="pre">**</span></span><span class="n"><span class="pre">kwargs</span></span></em><span class="sig-paren">)</span> <span class="sig-return"><span class="sig-return-icon">&#x2192;</span> <span class="sig-return-typehint"><span class="pre">str</span></span></span><a class="headerlink" href="#mod.func" title="Link to this definition">¶</a></dt>
<dd><p>Function doc.</p>
<dl class="field-list simple">
…
</dl>
</dd></dl>

<dl class="py function">
<dt class="sig sig-object py" id="mod.opt">
<span class="sig-prename descclassname"><span class="pre">mod.</span></span><span class="sig-name descname"><span class="pre">opt</span></span><span class="sig-paren">(</span><em class="sig-param"><span class="n"><span class="pre">a</span></span></em><span class="optional">[</span>, <em class="sig-param"><span class="n"><span class="pre">b</span></span></em><span class="optional">[</span>, <em class="sig-param"><span class="n"><span class="pre">c</span></span></em><span class="optional">]</span><span class="optional">]</span><span class="sig-paren">)</span><a class="headerlink" href="#mod.opt" title="Link to this definition">¶</a></dt>
<dd></dd></dl>

```

`opt2([a, ]b, c[, d])`: `(<span class="optional">[</span><em>a</em>, <span class="optional">]</span><em>b</em>, <em>c</em><span class="optional">[</span>, <em>d</em><span class="optional">]</span>)`
(ems abbreviated). Nested content: `…</dd></dl>\n\n</dd></dl>\n\n`. Generic type params:
`<span class="sig-paren">[</span><em class="sig-param">…T…</em>, <em class="sig-param">…U: int…</em><span class="sig-paren">]</span><span class="sig-paren">(</span>…`.
No-index: `<dt class="sig sig-object py">` (no id, no permalink). Two signatures: two `<dt>`
then one `<dd>`. `py:data` value: `<span class="property"><span class="w"> </span><span class="p"><span class="pre">=</span></span><span class="w"> </span><span class="pre">42</span></span>`.
Decorator: `<span class="sig-prename descclassname"><span class="pre">&#64;</span></span>`.

C/C++ (multi-line signatures + `noemph` params):

```html
<dt class="sig sig-object c" id="c.c_func">
<span class="kt"><span class="pre">int</span></span><span class="w"> </span><span class="sig-name descname"><span class="n"><span class="pre">c_func</span></span></span><span class="sig-paren">(</span><span class="kt"><span class="pre">int</span></span><span class="w"> </span><span class="n"><span class="pre">a</span></span>, <span class="kt"><span class="pre">char</span></span><span class="w"> </span><span class="p"><span class="pre">*</span></span><span class="n"><span class="pre">b</span></span><span class="sig-paren">)</span><a class="headerlink" href="#c.c_func" title="Link to this definition">¶</a><br /></dt>
<dt class="sig sig-object cpp" id="_CPPv4I0E3Foo">
<span id="_CPPv3I0E3Foo"></span><span id="_CPPv2I0E3Foo"></span><span class="k"><span class="pre">template</span></span><span class="p"><span class="pre">&lt;</span></span>…<span class="p"><span class="pre">&gt;</span></span><br /><span class="k"><span class="pre">class</span></span><span class="w"> </span><span class="sig-name descname"><span class="n"><span class="pre">Foo</span></span></span><a class="headerlink" href="#_CPPv4I0E3Foo" title="Link to this definition">¶</a><br /></dt>
```

Multi-line parameter lists (probe 2, `python_maximum_signature_line_length = 20`, default
trailing comma True):

```html
<span class="sig-name descname"><span class="pre">mlopt</span></span><span class="sig-paren">(</span>

<dl>
<dd><em class="sig-param"><span class="n"><span class="pre">aaaa</span></span></em>,</dd>
<dd><em class="sig-param"><span class="n"><span class="pre">bbbb</span></span></em><span class="optional">[</span>,</dd>
<dd><em class="sig-param"><span class="n"><span class="pre">cccc</span></span></em><span class="optional">[</span>,</dd>
<dd><em class="sig-param"><span class="n"><span class="pre">dddd</span></span></em>,<span class="optional">]</span><span class="optional">]</span></dd>
</dl>

<span class="sig-paren">)</span><a class="headerlink" href="#mlopt" title="Link to this definition">¶</a></dt>
```

`mlfirstopt([aaaa, bbbb])` → `<dd><span class="optional">[</span><em …>aaaa</em>,</dd>\n<dd><em …>bbbb</em>,<span class="optional">]</span></dd>\n`;
`mltype[TTTT, UUUU](xxxx: TTTT) -> None` → a `<dl>` for the type params (`[`…`]`) and a
second `<dl>` for the params, each `\n\n<dl>\n…</dl>\n\n`;
`mlopt3(aaaa[, bbbb], cccc)` → `<dd>…aaaa</em>,<span class="optional">[</span></dd>\n<dd>…bbbb</em>,<span class="optional">]</span></dd>\n<dd>…cccc</em>,</dd>\n`.
Other domains seen: `<dl class="js function">`, `<dl class="rst directive">` with
`<span class="sig-name descname"><span class="pre">..</span> <span class="pre">mydir::</span></span><span class="sig-prename descclassname"> <span class="pre">arg</span></span>`,
`<dl class="std option">` with `<span id="cmdoption-verbose"></span>…<span class="sig-prename descclassname"></span>`.

The domain class on `desc_signature` comes from the resolve-time `PropagateDescDomain`
post-transform (already ported: `src/env/resolve.rs:831`).

---

## 9. Pygments — what Sphinx asks for and what comes back

### 9.1 PygmentsBridge (`SPHINX/highlighting.py:98-237`)

- Builder: `init_highlighter` (`SPHINX/builders/html/__init__.py:237-258`) → style from
  `pygments_style`, else the theme's `pygments_style.default` (basic `theme.toml`:
  `pygments_style = { default = "none" }`), else `'sphinx'`. The style only affects
  `_static/pygments.css`, **never the body**: `HtmlFormatter` (noclasses=False) emits class
  names, not inline styles.
- `formatter_args = {'style': style}`; formatter class `HtmlFormatter`; `get_formatter(**kwargs)`
  merges the call kwargs (`linenos`, `hl_lines`, `linenostart`, `nowrap`). **No** `cssclass`
  override (stays `highlight`), no `wrapcode`, no `lineanchors`, no `filename`.
- `lexer_classes` (`:44-50`): `'none'` → `TextLexer(stripnl=False)`, `'python'` →
  `PythonLexer(stripnl=False)`, `'pycon'` → `PythonConsoleLexer(stripnl=False)`, `'rest'` →
  `RstLexer(stripnl=False)`, `'c'` → `CLexer(stripnl=False)`. Everything else via
  `get_lexer_by_name(lang, **opts)` — default lexer options `stripnl=True`,
  `stripall=False`, `ensurenl=True`, `tabsize=0`.

`get_lexer(source, lang, opts, force, location)` (`:136-181`):

```
if lang in {'py','python','py3','python3','default'}:
    lang = 'pycon' if source.startswith('>>>') else 'python'
if lang == 'pycon3': lang = 'pycon'
if lang in lexers: return lexers[lang]                 # app.add_lexer() registrations, no filter
elif lang in lexer_classes: lexer = lexer_classes[lang](**opts)
else:
    try: lexer = guess_lexer(source, **opts) if lang == 'guess' else get_lexer_by_name(lang, **opts)
    except ClassNotFound:
        warning('Pygments lexer name %r is not known', lang, location=location, type='misc', subtype='highlighting_failure')
        lexer = lexer_classes['none'](**opts)
if not force: lexer.add_filter('raiseonerror')
return lexer
```

The case-sensitive set test matters: `.. code-block:: Python` goes to `get_lexer_by_name`
(case-insensitive alias match) → `PythonLexer()` with **stripnl=True** (leading/trailing
blank lines dropped), vs `python` → `stripnl=False`. Verified:
`'Python'` on `'\nx\n\n'` → `<pre><span></span><span class="n">x</span>\n</pre>`;
`'python'` → `<pre><span></span>\n<span class="n">x</span>\n\n</pre>`.

`highlight_block(source, lang, opts, force, location, **kwargs)` (`:183-230`):

```
lexer = get_lexer(...); formatter = get_formatter(**kwargs)
try: hlsource = highlight(source, lexer, formatter)
except ErrorToken as err:
    if lang == 'default': lang = 'none'          # SILENT fallback (lang = the caller's value)
    else:
        warning('Lexing literal_block %r as "%s" resulted in an error at token: %r. Retrying in relaxed mode.',
                source, lang, str(err), type='misc', subtype='highlighting_failure', location=location)
        if force: lang = 'none' else: force = True
    lexer = get_lexer(source, lang, opts, force, location)
    hlsource = highlight(source, lexer, formatter)
return hlsource
```

The task brief's "Could not lex literal_block as …" wording is from old Sphinx; 9.1 emits
exactly (probe 1):
`code.rst:67: WARNING: Lexing literal_block '{"a": broken}' as "json" resulted in an error at token: 'b'. Retrying in relaxed mode. [misc.highlighting_failure]`
and `code.rst:71: WARNING: Pygments lexer name 'nosuchlang' is not known [misc.highlighting_failure]`.
`%r` is Python `repr()` of the whole block source (use `py_repr_str`); `str(err)` is the
value of the first `Token.Error` token (`RaiseOnErrorTokenFilter`,
`PYGMENTS/filters/__init__.py:763-788`: raises on `ttype is Error` exactly, not subtypes).
Location = the literal_block's source:line (the `.. code-block::` directive line; for an
inline `:code:` role, the enclosing paragraph's first line). These are **write-phase**
warnings, emitted in document write order (after resolve warnings).

Silent `default` fallback examples (verified): `$ pip install foo`, ``a = `b` ``, `a?` as
`default` → plain TextLexer output; `C:\path\to` does NOT error in the Python lexer
(`<span class="n">C</span><span class="p">:</span>\<span class="n">path</span>…`). Knowing
when the Python lexer produces an Error token requires the real Python lexer.

### 9.2 Lexer input preprocessing (`PYGMENTS/lexer.py:206-251`, `get_tokens` 253-275)

Strip a leading BOM; `\r\n` → `\n`; `\r` → `\n`; `stripall` → `strip()`, elif `stripnl` →
`strip('\n')`; `tabsize > 0` → `expandtabs`; `ensurenl` and not ending in `\n` → append `\n`.
Then tokens are passed through filters (`raiseonerror`).

### 9.3 RegexLexer engine (`PYGMENTS/lexer.py:667-762`)

`flags = re.MULTILINE`. At each `pos`, try the current state's rules in order with
`regex.match(text, pos)` (anchored at `pos`, but `^`/`\A`/`\b`/lookbehind see the whole
string — Python `re` semantics). On match: emit token(s) (plain type → one token with the
whole match; callback → e.g. `bygroups` emits one token per non-empty group, `using(...)`
re-lexes a group with another lexer or `this`), `pos = m.end()`, apply state change
(`'#pop'`, `'#push'`, name, tuple of those, negative int = pop n keeping ≥1, `combined(...)`
= anonymous merged state, `default(state)` = empty match transition). No rule matched: if
`text[pos] == '\n'` reset stack to `['root']` and emit `(Whitespace, '\n')`; else emit
`(Error, text[pos])` and advance one char (this is what trips `raiseonerror`).
`DelegatingLexer` (`:289-…`, used by `PythonConsoleLexer`) lexes with the language lexer,
buffers every `Other`/needle token's text into one string, lexes that buffer with the root
lexer, and splices the language tokens back in with `do_insertions` — so Python code spread
over `>>>`/`...` prompt lines is lexed as ONE contiguous program.
`PythonConsoleLexer` = DelegatingLexer(PythonTracebackLexer, DelegatingLexer(PythonLexer,
_PythonConsoleLexerBase, Other.Code), Other.Traceback) (`PYGMENTS/lexers/python.py:648-731`).

Token stream examples (Pygments 2.21):

```
PythonLexer('def f(x):\n    return x  # c\n'):
Keyword 'def' | Text.Whitespace ' ' | Name.Function 'f' | Punctuation '(' | Name 'x' | Punctuation ')' | Punctuation ':' |
Text.Whitespace '\n' | Text '    ' | Keyword 'return' | Text ' ' | Name 'x' | Text '  ' | Comment.Single '# c' | Text.Whitespace '\n'
PythonConsoleLexer('>>> for i in x:\n...     pass\nout\n'):
Generic.Prompt '>>> ' | Keyword 'for' | Text ' ' | Name 'i' | Text ' ' | Operator.Word 'in' | Text ' ' | Name 'x' | Punctuation ':' |
Text.Whitespace '\n' | Generic.Prompt '... ' | Text '    ' | Keyword 'pass' | Text.Whitespace '\n' | Generic.Output 'out\n'
```

### 9.4 HtmlFormatter algorithm (`PYGMENTS/formatters/html.py`)

`format_unencoded` (`:956-…`): `source = _format_lines(tokens)`; if not nowrap and
`linenos == 2` (`'inline'`) → `_wrap_inlinelinenos`; if `hl_lines` → `_highlight_lines`;
if not nowrap: `wrap()` (= `_wrap_pre`), if `linenos == 1` (`'table'`/True) →
`_wrap_tablelinenos`, then `_wrap_div`. `linenos` option: `'inline'` → 2, other truthy → 1.

`_format_lines` (`:830-918`) — the part a Rust port must copy exactly:

- CSS class of a token type: `_get_css_classes(ttype)` = `_get_ttype_class` of the type,
  prefixed by the classes of every non-standard ancestor: standard type → its short name
  from `STANDARD_TYPES` (`PYGMENTS/token.py:123-…`, table in §9.6); non-standard subtype →
  nearest standard ancestor's name + `-Sub-Parts` (e.g. `Comment.Single.Foo` →
  `c1-Foo`), and then `"<ancestor classes> <own>"` (e.g. `"s2 s2-Foo"`). `Token.Text`
  and `Token` → `''` → **no span**.
- Token value escaped with `html.escape(value, quote=False)` (`_translate_parts`,
  `:820-828`) → **only `&` `<` `>`**; then split on `\n`.
- Spans coalesce: consecutive tokens with the same span opener share ONE `<span>`
  (`Punctuation ')'` + `Punctuation ':'` → `<span class="p">):</span>`;
  JSON error chars → `<span class="err">broke</span>`).
- Every output line ends with the line separator `\n`; an open span is closed before the
  `\n` and reopened on the next line only if that line has non-empty content from the same
  token (`<span class="s2">"""a</span>\n<span class="s2">b"""</span>`). Empty pieces never
  create spans; a `Whitespace '\n'` token therefore produces just `\n`.
- `_wrap_pre`: `<pre><span></span>` + lines + `</pre>` (the empty span is always present).
- `_wrap_div`: `<div class="highlight">` + … + `</div>\n`.
- `_wrap_inlinelinenos` (`:718-781`): width `mw = len(str(nlines + linenostart - 1))`, each line
  prefixed `<span class="linenos">%*d</span>` (right-aligned with spaces: `<span class="linenos"> 1</span>` … `<span class="linenos">10</span>`).
- `_highlight_lines` (`:920-939`): line i (1-based count of emitted lines, after inline linenos)
  in `hl_lines` → `<span class="hll">LINE</span>` (LINE includes its trailing `\n` and the
  linenos span).
- `_wrap_tablelinenos` (`:650-716`): `<table class="highlighttable"><tr><td class="linenos"><div class="linenodiv"><pre>`
  + `'\n'.join('<span class="normal">%*d</span>')` + `</pre></div></td><td class="code">` + `<div>` + (pre block) + `</div>` + `</td></tr></table>`,
  all inside `_wrap_div`.

Exact outputs (verified with Sphinx's bridge):

```
none  '\n\nabc\n\n'        → '<div class="highlight"><pre><span></span>\n\nabc\n\n</pre></div>\n'
text  '\n\nabc\n\n'        → '<div class="highlight"><pre><span></span>abc\n</pre></div>\n'   (stripnl)
none  'abc'                → '<div class="highlight"><pre><span></span>abc\n</pre></div>\n'
none  ''                   → '<div class="highlight"><pre><span></span>\n</pre></div>\n'
none  'a\r\nb'             → '<div class="highlight"><pre><span></span>a\nb\n</pre></div>\n'
none  'a\tb'               → '<div class="highlight"><pre><span></span>a\tb\n</pre></div>\n'
inline start=98 'a\nb\nc'  → '…<pre><span></span><span class="linenos"> 98</span>a\n<span class="linenos"> 99</span>b\n<span class="linenos">100</span>c\n</pre></div>\n'
inline + hl_lines=[1,3]    → '…<pre><span></span><span class="hll"><span class="linenos">1</span>a\n</span><span class="linenos">2</span>b\n<span class="hll"><span class="linenos">3</span>c\n</span></pre></div>\n'
hl_lines=[2]               → '…<pre><span></span>a\n<span class="hll">b\n</span>c\n</pre></div>\n'
table 'a\nb'               → '<div class="highlight"><table class="highlighttable"><tr><td class="linenos"><div class="linenodiv"><pre><span class="normal">1</span>\n<span class="normal">2</span></pre></div></td><td class="code"><div><pre><span></span>a\nb\n</pre></div></td></tr></table></div>\n'
python nowrap 'print(1)\n' → '<span class="nb">print</span><span class="p">(</span><span class="mi">1</span><span class="p">)</span>\n'
```

Full Sphinx block for `none` / `text` (what a "no-highlight parity mode" must emit):

```html
<div class="highlight-none notranslate"><div class="highlight"><pre><span></span>none  lang   spaces
</pre></div>
</div>
<div class="highlight-text notranslate"><div class="highlight"><pre><span></span>plain &lt;text&gt; &amp; "stuff"
</pre></div>
</div>
```

### 9.5 `highlight_language` / `.. highlight::` interplay

`HighlightLanguageTransform` (post-transform, `SPHINX/transforms/post_transforms/code.py:30-86`)
stamps `language` (+ `force`) from the innermost `highlight` setting onto every
`literal_block` lacking one, and `linenos = (text.count('\n') >= linenothreshold - 1)`
(threshold default `sys.maxsize` → False). The crate currently lacks this stamp
(`KNOWN_HIGHLIGHT_STAMP_GAPS`, `tests/env_differential.rs:701-790`); the writer needs it.
`TrimDoctestFlagsTransform` (`:89-133`, `trim_doctest_flags=True`) strips
`# doctest: +FLAG` and `<BLANKLINE>` from pycon/`>>>` blocks and all doctest blocks.
`highlight_options` is keyed by the *original* `lang` string.

### 9.6 CSS classes a native highlighter must emit (`PYGMENTS/token.py` STANDARD_TYPES)

```
Text ''  Whitespace w  Escape esc  Error err  Other x
Keyword k  .Constant kc  .Declaration kd  .Namespace kn  .Pseudo kp  .Reserved kr  .Type kt
Name n  .Attribute na  .Builtin nb  .Builtin.Pseudo bp  .Class nc  .Constant no  .Decorator nd
  .Entity ni  .Exception ne  .Function nf  .Function.Magic fm  .Property py  .Label nl
  .Namespace nn  .Other nx  .Tag nt  .Variable nv  .Variable.Class vc  .Variable.Global vg
  .Variable.Instance vi  .Variable.Magic vm
Literal l  .Date ld
String s  .Affix sa  .Backtick sb  .Char sc  .Delimiter dl  .Doc sd  .Double s2  .Escape se
  .Heredoc sh  .Interpol si  .Other sx  .Regex sr  .Single s1  .Symbol ss
Number m  .Bin mb  .Float mf  .Hex mh  .Integer mi  .Integer.Long il  .Oct mo
Operator o  .Word ow      Punctuation p  .Marker pm
Comment c  .Hashbang ch  .Multiline cm  .Preproc cp  .PreprocFile cpf  .Single c1  .Special cs
Generic g  .Deleted gd  .Emph ge  .Error gr  .Heading gh  .Inserted gi  .Output go  .Prompt gp
  .Strong gs  .Subheading gu  .EmphStrong ges  .Traceback gt
```

Seen in probes: python (`k kn nn nb p s2 s1 mf mh mi c1 n nf nc o ow w`), pycon
(`gp go`), console/BashSession (`gp nb w go`, plain text for args), bash (`nb w s2 p`),
rst (`gh ge s`), json (`p nt w mi err kc`), c (`kt w n p`), guess→bash (`ch nb w`).

### 9.7 Version sensitivity — must pin

Same input `x = "a" + 'b'` as python:

- Pygments 2.19.2 and 2.20.0: `<span class="s2">&quot;a&quot;</span> <span class="o">+</span> <span class="s1">&#39;b&#39;</span>`
- Pygments 2.21.0 (the oracle env here): `<span class="s2">"a"</span> <span class="o">+</span> <span class="s1">'b'</span>`

Lexer rules also move between releases (2.21's PythonLexer knows `lazy import` and
t-strings). The HTML fixture generator must pin `pygments==2.21.0` and record its version in
the fixture header next to `sphinx_version`/`docutils_version`.

### 9.8 Lexer name space

Pygments 2.21 has 602 lexers / 927 aliases (`PYGMENTS/lexers/_mapping.py`). Deciding whether
to emit `Pygments lexer name %r is not known` requires the full alias table (e.g. `txt` IS
unknown and warns; `console` → BashSessionLexer, `shell`/`sh`/`bash` → BashLexer,
`rst`/`rest`/`restructuredtext` → RstLexer, `json` → JsonLexer (hand-written `Lexer`, not
regex), `yaml` → YamlLexer (ExtendedRegexLexer with callbacks), `c`/`cpp` → CFamilyLexer,
`ipython` unknown, `numpy` → NumPyLexer(PythonLexer)). The alias table is static data that
can be generated by a `tools/gen_pygments_aliases.py`. `guess` runs `guess_lexer` over every
lexer's `analyse_text` — not reproducible without all lexers.

### 9.9 Feasibility assessment and recommendation

Byte-exact by construction (small, do in wave 5):

- `HtmlFormatter` (≈200 lines of logic above): escaping, coalescing, per-line span
  close/reopen, `<span></span>`, inline and table linenos incl. width padding, `hl_lines`,
  nowrap. Generic over `Vec<(TokenType, String)>` where `TokenType` is a path like
  `["Name","Function"]`.
- Lexer preprocessing (`stripnl` vs `stripnl=False` per Sphinx's `lexer_classes`, `ensurenl`,
  CRLF, BOM).
- `TextLexer` (`none`, `text`, unknown-name fallback): one `Text` token → escaped text.
- The Sphinx fallback state machine (§9.1) given a lexer that can report `Error` tokens.

Byte-exact but a real port (separate task, likely wave 5b/M3):

- **PythonLexer** (≈400 lines of rule tables in `PYGMENTS/lexers/python.py:25-~425`, uses
  `bygroups`, `using(this)`, `include`, `combined`, `default`, `words(...)` with `\b`
  suffixes, Unicode XID classes, 4 lookaround constructs, MULTILINE `^`/`$`), plus
  `PythonConsoleLexer`/`_PythonConsoleLexerBase`/`PythonTracebackLexer` and
  `DelegatingLexer`+`do_insertions`. This is mandatory for parity with the default
  `highlight_language='default'` (every `::` block). The Rust `regex` crate lacks
  lookaround; use `fancy-regex` (or regex-automata anchored searches for the non-lookaround
  rules) and replicate Python `re` semantics for `\s \w \b \d` (Unicode), `^`/`$` with
  MULTILINE, anchored `match` at `pos` with look-behind context. Validate with a
  **token-stream differential fixture** (`list(lexer.get_tokens(src))` from real Pygments
  over a corpus incl. error-triggering inputs), independent of HTML.
- Second tier: `RstLexer`, `BashLexer`, `BashSessionLexer` (`console`), `JsonLexer`,
  `IniLexer`, `DiffLexer`, `CLexer`. Each is its own port; `YamlLexer`/C-family are costly.
- A semi-automatic path: dump each lexer's compiled `_tokens` tables (regex pattern strings,
  token types, state transitions; `bygroups`/`using` closures can be introspected) from the
  pinned Pygments into generated Rust data + one generic RegexLexer interpreter. Worth a spike
  before hand-porting.

Not viable for parity: `syntect` (Sublime grammars → different token boundaries and class
names; `docs/IMPLEMENTATION_STATUS.md:112` says syntect "returns when highlighting is wired" —
it can only back a non-parity fast mode), tree-sitter.

**Recommendation for wave 5:**

1. Ship the exact formatter + TextLexer + fallback machinery + alias table.
2. Treat every non-ported language as "highlight-divergent": emit the TextLexer rendering
   inside the correct wrapper (`highlight-<lang>` class preserved), never emit the
   `Lexing literal_block` warning for it, emit `lexer name … is not known` only for names
   absent from the alias table.
3. Build the wave-5 HTML oracle with `highlight_language = 'none'` (a `-D` conf override the
   harness can express) and only `none`/`text` explicit languages (plus `:code:` without
   language), so every other byte of the page is compared strictly; add a separate,
   explicitly-exempted project for highlighted languages until the Python lexer port lands.
4. Port PythonLexer/PythonConsoleLexer next with a token-level oracle; then flip the HTML
   oracle to the default `highlight_language`.

---

## 10. Writer-time warnings and messages (reporter channel)

Emitted during writing (order = write order of documents, then tree order):

| message | where | type |
|---|---|---|
| `Pygments lexer name %r is not known` | highlighting.py:169 | `misc.highlighting_failure` |
| `Lexing literal_block %r as "%s" resulted in an error at token: %r. Retrying in relaxed mode.` | highlighting.py:207 | `misc.highlighting_failure` |
| `Could not obtain image size. :scale: option is ignored.` | html5.py:786 | none |
| `numfig_format is not defined for %s` | html5.py:446 | none |
| `Any IDs not assigned for %s node` | html5.py:456 | none |
| `unsupported rubric heading level: %s` | html5.py:586 | `html` |
| `unknown node type: %r` | util/docutils.py:815 | none |
| docutils writer messages (`Cannot scale image!…`, SVG parse errors) | via `document.reporter` → `[docutils]` | — |

The docutils-writer messages are ALSO rendered into the body (report_level 2) — probe 8,
remote image with `:scale:` and no Pillow installed:

```
index.rst:4: WARNING: Could not obtain image size. :scale: option is ignored.
index.rst:4: WARNING: Cannot scale image!
  Could not get size from "https://example.com/r.png":
  Requires Python Imaging Library. [docutils]
```
```html
<a class="reference internal image-reference" href="https://example.com/r.png"><img alt="https://example.com/r.png" src="https://example.com/r.png" />
<aside class="system-message">
<p class="system-message-title">System Message: WARNING/2 (<span class="docutils literal">/abs/src8/index.rst</span>, line 4)</p>
<p>Cannot scale image!
  Could not get size from &quot;https://example.com/r.png&quot;:
  Requires Python Imaging Library.</p>
</aside>
</a>
```

This depends on Pillow being absent in the oracle environment (another reason to pin the
environment). Low priority, but it is a real `[docutils]` reporter-channel case at write time.

---

## 11. Inputs the translator needs from earlier phases (dependencies)

The writer's parity is only as good as the resolved doctree it walks:

1. **Toctree resolution** (`_resolve_toctree`, `KNOWN_RESOLVED_GAPS` `TOCTREE_RESOLUTION`):
   produces `compact_paragraph(toctree=True)` + optional caption `title` + `bullet_list`
   of `list_item(classes=['toctree-lN'])` → `compact_paragraph` → `reference(internal,
   refuri, secnumber?)`. Output shape in §3/§12.
2. **HighlightLanguageTransform + TrimDoctestFlagsTransform** (§9.5).
3. **Image candidates + post_process_images** (§7.6-7.7) and the `_images` unique-name map.
4. **PropagateTargets** in-tree (ids land on the following node: `<p id="explicit-target">`,
   `<section id="labelled-section">\n<span id="my-label"></span>`).
5. **OnlyNodeTransform** (the `only` node is replaced by its children, ids moved to the first
   child: `<p id="index-0">Only html.</p>`).
6. MetadataCollector's docinfo removal.
7. `rawsource` knowledge for parsed-literal (§5.1).
8. Per-doc `toc_secnumbers`/`toc_fignumbers`, `imgpath`/`dlpath` via `relative_uri`.

Suggested Rust shape:

```rust
struct WriteCtx<'a> {
    docname: &'a str,
    secnumbers: &'a BTreeMap<String, Vec<u32>>,               // env.toc_secnumbers[doc]
    fignumbers: &'a BTreeMap<String, BTreeMap<String, Vec<u32>>>,
    images: &'a HashMap<String, String>,                       // uri -> unique filename
    imgpath: String, dlpath: String,                           // relative_uri(target_uri(doc), "_images"/"_downloads")
    srcdir: &'a Path,
    cfg: &'a HtmlWriterConfig,  // permalinks, icon, secnumber_suffix, numfig/_format, math_*, mathjax_*, highlight_options, linenos_style
    highlighter: &'a dyn Highlighter,
    warnings: &'a mut Vec<BuildWarning>,
}
struct Translator { body: String, context: Vec<Ctx>, section_level: u32, protect_literal_text: u32,
    in_mailto: bool, compact_simple: bool, compact_p: Option<bool>, table_rows: Vec<u32>, field_rows: Vec<u32>,
    sig: SigParamState, meta: Vec<String>, has_maths: bool, messages: Vec<Node> }
```

`Ctx` is a small enum (`Str(String)`, `CompactState(bool, Option<bool>)`, …) mirroring the
heterogeneous docutils stack.

### 11.1 Oracle recommendations for the HTML differential

- Pin in `uv run`: `sphinx==9.1.0 docutils==0.22.4 pygments==2.21.0 jinja2==3.1.6
  imagesize==2.0.1 alabaster==1.0.0 snowballstemmer==3.1.1 babel==2.18.0` (versions present
  in this env), and assert Pillow is NOT importable.
- Normalise absolute source paths inside `system-message` titles (keep_warnings projects).
- Compare `context['body']` (the fragment) separately from the full page — the `dumpbody`
  extension in the probe dir is a 10-line template for that.

---

## 12. Toctree / local-TOC rendering (translator output of resolved trees)

```html
<div class="toctree-wrapper compound">
<p class="caption" role="heading"><span class="caption-text">Contents</span></p>
<ul>
<li class="toctree-l1"><a class="reference internal" href="inline.html">Inline</a><ul>
<li class="toctree-l2"><a class="reference internal" href="inline.html#labelled-section">Labelled Section</a></li>
</ul>
</li>
<li class="toctree-l2"><a class="reference internal" href="desc.html#mod.func"><code class="docutils literal notranslate"><span class="pre">func()</span></code></a></li>
</ul>
</div>
```

Numbered: `<a class="reference internal" href="chap.html">1. Chapter</a>`,
`<a class="reference internal" href="chap.html#section-one">1.1. Section One</a>`.
`:name:` puts the id on the wrapper: `<div class="toctree-wrapper compound" id="main-toc">`
(no toctree permalink). Local TOC (`toc` context var, via `render_partial` of
`document_toc`, `SPHINX/environment/adapters/toctree.py:50-67`, references rewritten to
`anchorname or '#'`):

```html
<ul>
<li><a class="reference internal" href="#">Inline</a><ul>
<li><a class="reference internal" href="#labelled-section">Labelled Section</a></li>
</ul>
</li>
</ul>
```

(never `simple`, because of the compact_paragraph children).

---

## 13. Checklist of every visitor defined in `SPHINX/writers/html5.py` (123 methods)

start_of_file ✓(singlehtml) · desc ✓ · desc_signature ✓ · desc_signature_line ✓ ·
desc_content ✓ · desc_inline ✓ · desc_name ✓ · desc_addname ✓ · desc_type ✓ ·
desc_returns ✓ · desc_parameterlist ✓ · desc_type_parameter_list ✓ · desc_parameter ✓ ·
desc_type_parameter ✓ · desc_optional ✓ · desc_annotation ✓ · versionmodified ✓ ·
reference ✓ · number_reference ✓ · comment ✓ · admonition ✓ · seealso ✓ · (helpers
get_secnumber/add_secnumber/add_fignumber/add_permalink_ref ✓) · bullet_list ✓ ·
definition ✓ · classifier ✓ · term ✓ · title ✓ · rubric ✓ · literal_block ✓ · caption ✓ ·
doctest_block ✓ · block_quote ✓ · literal ✓ · productionlist ✓ · production ✓ · centered ✓ ·
compact_paragraph ✓ · download_reference ✓ · figure ✓ · image ✓ · toctree ✓ · index ✓ ·
tabular_col_spec ✓ · glossary ✓ · acks ✓ · hlist ✓ · hlistcol ✓ · Text ✓ · note/warning/
attention/caution/danger/error/hint/important/tip ✓ · literal_emphasis ✓ ·
literal_strong ✓ · abbreviation ✓ · manpage ✓ · table ✓ · row ✓ · field_list ✓ · field ✓ ·
math ✓ · math_block ✓ · footnote_reference ✓ (depart inherited).

Inherited docutils visitors that appear in Sphinx output and are specified above:
section, paragraph, emphasis, strong, subscript, superscript, title_reference, inline,
acronym, target, problematic, raw, transition, line_block, line, attribution, compound,
container, topic, sidebar, subtitle, generated, bullet/enumerated list, list_item,
definition_list(_item), field_name/field_body, option_list family, table internals
(tgroup/colspec/thead/tbody/entry), footnote/citation/label, system_message, legend, meta,
header/footer/docinfo (moved out of fragment), substitution_definition (skipped),
image internals, document (head/title only).
