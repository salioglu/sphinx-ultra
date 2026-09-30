# M2 wave 5 research: the docutils HTML base translator under Sphinx's `HTML5Translator` (part 1)

Research key: `translator-docutils`. Scope: the **docutils 0.22.4** HTML translator
stack that Sphinx 9.1.0's `HTML5Translator` inherits from. This note specifies every
`visit_*`/`depart_*` method that Sphinx does **not** override, plus the shared
primitives (`starttag`, `emptytag`, `attval`, `encode`, the id-span rule, class
assembly, the compact-list checker, `report_messages`). Sphinx's overrides are listed
(section 5) so implementers know which paths are dead or owned by part 2, but they are
not specified here, apart from where a base method's output only makes sense next to
them.

Every "Verified" snippet below is **verbatim output from real Sphinx 9.1.0 / docutils
0.22.4**. It was captured from `ctx['body']` in an `html-page-context` hook, which is
exactly `''.join(visitor.fragment)`, with `html_theme = 'basic'`. The probe projects are
kept under
`/tmp/claude-0/-home-user-sphinx-ultra/46bf5e6b-694f-5b8e-ba0d-36f1851a8974/scratchpad/probe-translator-docutils/`
(`src/`, `src2/` with `keep_warnings=True` and `html_compact_lists=False`, then
`src3/`, `src4/` and `src5/`). Each project's `conf.py` has the dump hook, and
`bodies/*.html` holds the last build's body fragments. To re-run one:

```
cd <probe dir> && PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' \
  --with 'docutils==0.22.4' python -m sphinx -b html -q src out
```

Path abbreviations:

* `BASE` = `docutils/writers/_html_base.py`
* `H5` = `docutils/writers/html5_polyglot/__init__.py`
* `SX` = `sphinx/writers/html5.py`
* `SXB` = `sphinx/builders/html/__init__.py`

All of these live under
`/root/.cache/uv/archive-v0/b4dBDAdEzskuqge1iT52j/lib/python3.12/site-packages/`.

---

## 0. TL;DR for implementers

1. **Output unit.** The page body is `''.join(visitor.fragment)`, and `fragment` is
   `self.body` at `depart_document` time (BASE:979). Sphinx never runs docutils'
   `Writer.translate()`/`apply_template()`. It calls `doctree.walkabout(visitor)`
   directly (SXB:650-665), which means:
   * the docutils **writer transforms are NOT applied** to page doctrees (no
     `writer_aux.Admonitions`, no `universal.Messages`/`FilterMessages`);
   * the `head`, `stylesheet`, `body_prefix` and similar parts are computed but
     discarded. The only other part Sphinx uses is `metatags = ''.join(visitor.meta[2:])`
     (SXB:661).
2. **Settings.** Sphinx's translator settings come from the **html4css1** writer's
   settings spec, not html5_polyglot's, because `sphinx.writers.html.HTMLWriter`
   subclasses `html4css1.Writer` (`sphinx/writers/html.py:23-25`). The two consequences
   for output are `initial_header_level = '1'`, so a top-level section title is `<h1>`,
   and `math_output = ('html', 'math.css')`. The math setting is irrelevant because
   Sphinx overrides math. The full table is in section 2.
3. **Escaping.** `encode()` maps exactly five characters: `& < " > @` become `&amp; &lt;
   &quot; &gt; &#64;`. **`'` is NOT escaped**, and **`@` IS escaped everywhere**,
   including text, hrefs and titles. Non-ASCII characters pass through raw.
   `attval()` first replaces each `[\n\r\t\v\f]` character with one space and then
   calls `encode()`. Do not use the `html_escape` crate (the current placeholder at
   `src/builder.rs:874-879` and `src/html_builder.rs:369-373` does); it escapes a
   different character set.
4. **Attributes are sorted** by lowercased name (plain `str` ordering), and `id` is
   sorted like any other attribute. Empty elements end in ` />`. The `class` value is
   assembled in this order: the `classes=` kwarg list, then the node's `classes`, then
   the words of the `CLASS=` string, de-duplicated with the first occurrence winning.
   Classes starting with `language-` are removed and become `lang="…"` (the first one
   wins).
5. **Multiple ids.** `id=` takes `ids[0]`. Every further id becomes
   `<span id="X"></span>`. The span goes **before** the tag for empty tags (`img`,
   `hr`, `meta`, `col`), `Sequential` nodes (bullet/enumerated/definition/field/option
   lists), `docinfo` and `table`. For every other node it goes **after** the start tag,
   including after its suffix, so for `<section>` it lands after the `\n`. Section 3.3
   has verified examples.
6. **Newline placement is part of the contract.** A missing or extra `\n` breaks byte
   parity. The paragraph rule is the one with the most consequences: `</p>` gets a `\n`
   unless the paragraph is the **only child** (counting invisible children such as
   comments and targets) of a `list_item` or `entry`.
7. **Python formatting semantics must be reproduced:**
   * `round()` is round-half-to-even;
   * `f'{v:g}'` uses 6 significant digits and switches to `e±XX` form;
   * `f'{x:.1%}'` is used for colgroup widths.

   See section 9.
8. **Dead code under Sphinx defaults** (safe to omit in v1, but document it):
   * document title and subtitle (`doctitle_xform=False`);
   * section subtitles;
   * docinfo and all bibliographic items (Sphinx's `MetadataCollector` pops the docinfo
     node);
   * header and footer (captured out of `body`, so they never reach the page);
   * the docutils math path;
   * `citation_reference` (Sphinx turns citation refs into `reference`s);
   * `comment` (Sphinx skips it);
   * `substitution_reference` (always substituted, or `problematic`).

---

## 1. How Sphinx drives the translator

### 1.1 Call sequence (SXB)

* `prepare_writing` (SXB:442-445):
  `self.docsettings = _get_settings(HTMLWriter, defaults=self.env.settings, read_config_files=True)`
  and then `self.docsettings.compact_lists = bool(self.config.html_compact_lists)`.
  `read_config_files=True` means a `docutils.conf` in the working directory or user
  config can change the settings. Treat that as out of scope and state it.
* `write_doc(docname, doctree)` (SXB:650-665):
  ```python
  doctree.settings = self.docsettings
  self.secnumbers = self.env.toc_secnumbers.get(docname, {})
  self.fignumbers = self.env.toc_fignumbers.get(docname, {})
  self.imgpath = relative_uri(self.get_target_uri(docname), '_images')
  self.dlpath  = relative_uri(self.get_target_uri(docname), '_downloads')
  self.current_docname = docname
  visitor = self.create_translator(doctree, self)
  doctree.walkabout(visitor)
  body = ''.join(visitor.fragment)
  clean_meta = ''.join(visitor.meta[2:])
  ctx = self.get_doc_context(docname, body, clean_meta)
  ctx['has_maths_elements'] = getattr(visitor, '_has_maths_elements', False)
  ```
* Before `write_doc`, `write_doc_serialized` runs `post_process_images` (SXB:961-987).
  With `html_scaled_image_link` (default True), every `image` that has
  `scale`/`width`/`height`, is not already inside a `reference`, and lacks the
  `no-scaled-link` class is wrapped in `reference(internal=True, refuri=<imgpath/…>
  or the original uri)`. **This is a doctree mutation the writer then renders.** It
  explains the `<a class="reference internal image-reference" …>` wrappers in section
  7.10.
* `metatags`: `visitor.meta` starts as `['<meta charset="utf-8" />\n',
  '<meta name="generator" content="Docutils 0.22.4: https://docutils.sourceforge.io/" />\n',
  '<meta name="viewport" content="width=device-width, initial-scale=1" />\n']`
  (verified by dumping the translator; see BASE:342,352-354 and H5:129-132).
  `meta[2:]` therefore **always begins with the viewport meta**, followed by one
  `<meta …/>\n` per `meta` node (section 7.14). Verified `metatags` for a page with
  `.. meta:: :description: A description / :keywords: a, b`:
  ```
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <meta content="A description" name="description" />
  <meta content="a, b" name="keywords" />
  ```
* `render_partial(node)` (SXB:409-425) renders titles (`title`, and the prev/next/parents
  titles at SXB:576-608) and toc fragments (SXB:626, 1032) through the **same
  translator class** on a throwaway document. It uses different settings (SXB:155-160):
  `_get_settings(docutils.readers.doctree.Reader, docutils.parsers.rst.Parser,
  HTMLWriter, defaults={'output_encoding': 'unicode', 'traceback': True})`, with **no
  env settings**. Verified values are `cloak_email_addresses=None` (no cloaking),
  `smart_quotes=False`, `image_loading=None`, `language_code='en'` and
  `compact_lists=True` (it ignores `html_compact_lists`). It also applies
  `_PARSER_TRANSFORMS = [universal.Validate, universal.SmartQuotes]` and
  `_WRITER_TRANSFORMS = [universal.Messages, universal.FilterMessages,
  universal.StripClassesAndElements, writer_aux.Admonitions]`. `'title'` is
  `visitor.title`, the inner HTML of the `<h1 class="title">` produced because the
  title node is now a child of `document` (BASE:1771-1789). The page-assembly research
  owns this; the translator port must support both settings profiles.

### 1.2 Dispatch rule (`SphinxTranslator`, `sphinx/util/docutils.py:782-815`)

`dispatch_visit` walks `node.__class__.__mro__` and calls the first `visit_<ClassName>`
that exists. `dispatch_departure` does the same with `depart_`. This matters for Sphinx
node classes that subclass docutils classes. For example, every `desc_sig_*` node
(`desc_sig_name`, `desc_sig_punctuation`, …) and `desc_sig_element` has no own method,
so it resolves to **`visit_inline`/`depart_inline`** and renders as
`<span class="…">…</span>`. The resolution table below was computed from the real MRO:

| sphinx node | resolves to | docutils categories |
|---|---|---|
| `desc_sig_element`, `desc_sig_keyword`, `desc_sig_keyword_type`, `desc_sig_literal_char`, `desc_sig_literal_number`, `desc_sig_literal_string`, `desc_sig_name`, `desc_sig_operator`, `desc_sig_punctuation`, `desc_sig_space` | `inline` | TextElement, Inline |
| `desc_classname` | `desc_addname` | TextElement, FixedTextElement, Inline |
| `pending_xref`, `pending_xref_condition`, `only`, `highlightlang`, `translatable` | none | — |
| every other addnode | its own method | (see below) |

Any leftover node that resolves to nothing hits `unknown_visit`. That logs
`WARNING: unknown node type: %r` and continues into the children. The following
departure then goes through docutils' `NodeVisitor.unknown_departure`, which raises
`NotImplementedError` unless the node is in the visitor's `optional` set. Sphinx
guarantees none remain: `pending_xref` is resolved and `only` is filtered before the
write.

Sphinx addnodes that are **TextElement** matter for the "parent is TextElement" checks
(paragraph, image, reference, raw): `centered`, `compact_paragraph`, all `desc_*`
signature nodes, `desc_signature`, `desc_signature_line`, `download_reference`,
`index`, `literal_emphasis`, `literal_strong`, `manpage`, `number_reference`,
`pending_xref_condition`, `production`, `versionmodified`. **Admonition**
subclasses (these matter for `visit_title`) are `desc`, `productionlist`, `seealso`
and `versionmodified`. **Invisible**: `index`.

### 1.3 docutils class categories as tagname sets

The crate stores node identity as the tagname string (`src/doctree/mod.rs:97-104`,
`src/doctree/kinds.rs`), so every `isinstance` check in the translator must become a
set test. These sets were dumped from docutils 0.22.4 (only lowercase node classes
listed):

* **TextElement**: abbreviation acronym address attribution author caption
  citation_reference classifier comment contact copyright date doctest_block emphasis
  field_name footnote_reference generated inline label line literal literal_block math
  math_block option_argument option_string organization paragraph problematic raw
  reference revision rubric status strong subscript substitution_definition
  substitution_reference subtitle superscript target term title title_reference
  version. Add the Sphinx TextElements listed in 1.2.
* **FixedTextElement**: address comment doctest_block literal_block math_block raw
* **Sequential**: bullet_list definition_list enumerated_list field_list option_list
* **Invisible**: comment pending substitution_definition target, plus Sphinx `index`
* **Admonition**: admonition attention caution danger error hint important note tip
  warning, plus Sphinx `desc` `productionlist` `seealso` `versionmodified`
* **Structural**: section sidebar topic
* **PreBibliographic**: comment decoration meta pending raw substitution_definition
  subtitle system_message target title
* **Bibliographic**: address author authors contact copyright date field organization
  revision status version

### 1.4 NodeVisitor control flow

* `raise nodes.SkipNode` in `visit_X` skips both the children and `depart_X`.
* `raise nodes.SkipChildren` skips the children, but `depart_X` still runs.
* `walkabout` is otherwise a plain pre/post-order walk. Text nodes go through
  `visit_Text`/`depart_Text`.

---

## 2. Effective docutils settings seen by the page translator

This is the verified dump of `app.builder.docsettings` on Sphinx 9.1.0 (probe
`src3/`), with the origin of each value:

| setting | value | origin | effect on body output |
|---|---|---|---|
| `initial_header_level` | `'1'` | html4css1 spec (`docutils/writers/html4css1/__init__.py:67-72`), **not** html5's `'2'` | the section at depth d gets `<h{min(d,6)}>`, plus `aria-level="d"` when d>6 |
| `footnote_references` | `'brackets'` | `_html_base` default (BASE:90-95) | classes `footnote-list brackets`, `footnote brackets` and `footnote-reference brackets` |
| `attribution` | `'dash'` | BASE:96-101 | attribution prefix `—` (U+2014), no suffix |
| `compact_lists` | `bool(html_compact_lists)` (default True) | SXB:445 | whether bullet/enumerated lists can get `simple` |
| `compact_field_lists` | `True` | BASE:110-117 | definition and field lists can get `simple` |
| `table_style` | `''` | BASE:118-123 | no extra table classes; colgroup only for `colwidths-given` tables |
| `math_output` | `('html', 'math.css')` | html4css1 spec (`…/html4css1/__init__.py:73-78`) | **unused**: Sphinx overrides `visit_math`/`visit_math_block` |
| `cloak_email_addresses` | `True` | `sphinx/environment/__init__.py:63` (env `default_settings`) | `mailto:` hrefs and link text are obfuscated (7.1) |
| `image_loading` | `'link'` | env default_settings :61 | `<img src>`; the per-node `:loading:` option still applies |
| `section_self_link` | `False` | env default_settings :71 | no `self-link` anchors |
| `toc_backlinks` | `'entry'` | `docutils/frontend.py:727-730` | `contents` titles would get `<a href="#top">`, but Sphinx strips it (SX:541-551); section titles with `refid` get `toc-backref` links |
| `footnote_backlinks` | `True` | frontend.py:737-740 | label backlinks |
| `report_level` | `2` | frontend.py:771 | writer-time messages at level ≥ 2 are rendered into the body |
| `halt_level` | `5` | env default_settings :72 | — |
| `language_code` | `config.language` (`'en'`) | `sphinx/environment/__init__.py:379` | docinfo labels only (dead) |
| `output_encoding` | `'utf-8'` | docutils default | makes `meta[0]` the charset meta, hence Sphinx's `meta[2:]` |
| `file_insertion_enabled` | `True` | env default_settings :73 | lets `read_size_with_PIL` attempt the read |
| `doctitle_xform`, `sectsubtitle_xform` | `False` | env default_settings :69-70 | no document title or subtitle nodes |
| `smart_quotes` | `True` | env `_update_settings` :382 | read side: text is already curly-quoted by SmartQuotes (the crate has no SmartQuotes yet; see `tests/env_differential.rs:887-903`) |
| `trim_footnote_reference_space` | `config.trim_footnote_reference_space` (False) | read side | — |
| `xml_declaration`, `embed_stylesheet`, `stylesheet_path=['html4css1.css']`, `template` | various | — | head only; ignore |
| `embed_images` | unset | — | no FutureWarning path |

Translator-side derived state (verified): `initial_header_level=1`,
`math_output='html'`, `math_options='math.css'`, `image_loading='link'`,
`body_prefix=['</head>\n<body>\n']`. The translator MRO is `HTML5Translator` →
`SphinxTranslator` → `html5_polyglot.HTMLTranslator` → `_html_base.HTMLTranslator` →
`DoctreeTranslator` → `NodeVisitor`.

Sphinx config values that feed the translator directly are covered in part 2:
`html_permalinks` (True), `html_permalinks_icon` (`'¶'`), `html_secnumber_suffix`
(`'. '`), `html_compact_lists`, `html_scaled_image_link`, `keep_warnings` (read side:
`FilterSystemMessages`, `sphinx/transforms/__init__.py:337-347`, removes every
`system_message` with `level < 5` unless `keep_warnings`, in which case the threshold
is 2).

---

## 3. Primitives (exact algorithms)

### 3.1 `encode(text)` (BASE:281-286, 411-417)

```
str(text).translate({'&':'&amp;', '<':'&lt;', '"':'&quot;', '>':'&gt;', '@':'&#64;'})
```

Nothing else is escaped: `'` stays as it is, and non-ASCII characters are written raw.
The HTML file is later written as UTF-8. It is a single-pass character map, so there is
no double escaping.

### 3.2 `attval(text)` (BASE:387-395)

```
encoded = encode(re.sub('[\n\r\t\v\f]', ' ', text))   # each char → one space, no collapsing
if self.in_mailto and settings.cloak_email_addresses:
    encoded = encoded.replace('%40', '&#37;&#52;&#48;').replace('.', '&#46;')
return encoded
```

### 3.3 `starttag(node, tagname, suffix='\n', empty=False, **attributes)` (BASE:550-614)

Exact algorithm:

1. `tagname = tagname.lower()`, `prefix = []`, and
   `atts = {k.lower(): v for k, v in attributes}`. So `CLASS=` becomes `class` and
   `ROLE=` becomes `role`.
2. `classes = atts.pop('classes', [])`. This is **the caller's list object** and gets
   appended to (it aliases, e.g., `node.parent['classes']` in `visit_field_name`).
3. For each `cls` in `node.get('classes', []) + atts.pop('class', '').split()`:
   * if `cls.startswith('language-')`: `languages.append(cls[9:])`;
   * `elif cls.strip() and cls not in classes`: `classes.append(cls)`.

   The kwarg list comes first and is not itself de-duplicated. After it, node classes
   and then CLASS words are de-duplicated against everything collected so far.
4. If `languages` is non-empty, `atts['lang'] = languages[0]`, and all `language-*`
   classes are dropped.
5. If the node is a `table`, drop `colwidths-auto`, `colwidths-given` and
   `colwidths-grid` from `classes`.
6. If `classes` is non-empty, `atts['class'] = ' '.join(classes)`.
7. Take `ids = node.get('ids', [])` and then `ids.extend(atts.pop('ids', []))`. This
   **mutates node['ids']** when an `ids=` kwarg is passed (only the base `visit_term`
   does that, and Sphinx overrides it).
   * If `ids` is non-empty: `atts['id'] = ids[0]`.
   * For each further id: if `empty` or the node is `Sequential`/`docinfo`/`table`,
     `prefix.append('<span id="%s"></span>' % id)`; otherwise
     `suffix += '<span id="%s"></span>' % id`.
   * **The id value in the span is NOT escaped**, while the `id=` attribute IS escaped
     through attval.
8. `attlist = sorted(atts.items())` gives plain lexicographic order on the lowercased
   name. Each pair renders as `name="attval(value)"`. A list value is
   `' '.join(str(v) for v in value)`; any other value is `str(value)`, so ints print
   as `2`.
9. Return `''.join(prefix) + '<' + ' '.join([tagname] + pairs) + (' /' if empty else '') + '>' + suffix`.

`emptytag(node, tagname, suffix='\n', **attrs)` is `starttag(..., empty=True)`
(BASE:616-618).

The resulting attribute orders seen in practice:

* `aria-level` < `class` < `colspan` < `content` < `controls` < `data`
* then `height` < `href` < `id` < `lang` < `loading` < `name` < `open`
* then `rel` < `role` < `rowspan` < `src` < `start` < `style`
* then `target` < `title` < `type` < `width` < `xml:lang`

Pseudo-Rust sketch:

```rust
fn starttag(node: &Node, tag: &str, suffix: &str, empty: bool,
            kw_classes: Vec<String>, class_str: &str,
            mut atts: BTreeMap<String /*lowercase*/, String>) -> String {
    let mut classes = kw_classes;                    // kwarg list first
    let mut langs = vec![];
    for c in node.attrs.classes.iter().map(String::as_str)
                 .chain(class_str.split_whitespace()) {
        if let Some(l) = c.strip_prefix("language-") { langs.push(l.to_string()); }
        else if !c.trim().is_empty() && !classes.iter().any(|x| x == c) { classes.push(c.into()); }
    }
    if let Some(l) = langs.first() { atts.insert("lang".into(), l.clone()); }
    if node.kind == "table" { classes.retain(|c| !matches!(c.as_str(),
        "colwidths-auto" | "colwidths-given" | "colwidths-grid")); }
    if !classes.is_empty() { atts.insert("class".into(), classes.join(" ")); }
    let (mut prefix, mut suffix) = (String::new(), suffix.to_string());
    if let Some((first, rest)) = node.attrs.ids.split_first() {
        atts.insert("id".into(), first.clone());
        let before = empty || SEQUENTIAL.contains(&node.kind)
            || node.kind == "docinfo" || node.kind == "table";
        for id in rest {
            let span = format!("<span id=\"{id}\"></span>");     // NOT escaped
            if before { prefix.push_str(&span) } else { suffix.push_str(&span) }
        }
    }
    // BTreeMap iteration == Python sorted() on ASCII keys
    let pairs: Vec<String> = atts.iter().map(|(k, v)| format!("{k}=\"{}\"", attval(v))).collect();
    format!("{prefix}<{}{}{}>{suffix}", tag,
            pairs.iter().map(|p| format!(" {p}")).collect::<String>(),
            if empty { " /" } else { "" })
}
```

(`attval` must see `in_mailto`, so it has to be a method on the translator.)

**Verified id placement** (probe `src/misc2.rst`):

```html
<span id="t1"></span><ul class="simple" id="t2">
<li><p>list with two ids</p></li>
</ul>
<p id="t4"><span id="t3"></span>Para with two ids.</p>
<span id="t6"></span><span id="t5"></span><table class="docutils align-default" id="id1">
<span id="t7"></span><img alt="_images/pic.png" id="t8" src="_images/pic.png" />
<div class="admonition note" id="t10">
<span id="t9"></span><p class="admonition-title">Note</p>
```

and for a section (probe `src/sections.rst`; note the span comes **after** the `\n`):

```html
<section id="top-title">
<span id="explicit-top"></span><h1>Top Title<a class="headerlink" href="#top-title" title="Link to this heading">¶</a></h1>
…
<section id="level-two">
<span id="second-target"></span><span id="sub-target"></span><h2>Level Two…
```

The order of the ids (`['level-two', 'second-target', 'sub-target']`,
`['t2', 't1']`) is decided on the read side (PropagateTargets). The writer emits them
in stored order.

**Verified class assembly** (probe `src2/index.rst` and `src5/index.rst`):

| construct | call | output |
|---|---|---|
| `.. rst-class:: foo` + enumerated list | `classes=['arabic','simple']` kwarg | `<ol class="arabic simple foo">` |
| `.. rst-class:: foo` + bullet list | `CLASS='simple'` via `atts['class']` | `<ul class="foo simple">` |
| `.. rst-class:: foo` + definition list | `classes=['simple']` kwarg | `<dl class="simple foo">` |
| `.. rst-class:: foo` + field list | node classes mutated to `[foo, field-list, simple]` | `<dl class="foo field-list simple">` |
| `.. container:: docutils container extra` | `CLASS='docutils container'` | `<div class="docutils container extra">` (de-duplicated) |
| role with `:class: language-fr language-de x` | — | `<span class="x" lang="fr">multi lang</span>` |
| `.. rst-class:: a a b` + para | — | `<p class="a b">` |
| `.. topic:: :class: tc` | `classes=['topic']` | `<aside class="topic tc">` |
| `.. rubric:: :class: rc` | `CLASS='rubric'` | `<p class="rc rubric">` |
| abbr title with specials | — | `<abbr title="a&#64;b &quot;q&quot; &lt;x&gt; &amp; y">E</abbr>` |

### 3.4 Email cloaking (active because `cloak_email_addresses=True`)

* `cloak_mailto(uri)` (BASE:404-409) is `uri.replace('@', '%40')`. Sphinx's
  `visit_reference` calls it when an href starts with `mailto:` and then sets
  `in_mailto = True` (SX:330-337, part 2).
* While `in_mailto` holds, `attval` applies the `%40`/`.` replacement to **every**
  attribute of that `<a>` tag (3.2).
* `visit_Text` (SX:841-860, non-literal branch) runs
  `cloak_email(encode(text))` (BASE:397-402): `'&#64;'` becomes
  `'<span>&#64;</span>'`, then `'.'` becomes `'<span>&#46;</span>'`.
* `depart_reference` (BASE:1539-1543) resets `in_mailto = False`.

Verified:

```html
<a class="reference external" href="mailto:foo&#37;&#52;&#48;example&#46;com">foo<span>&#64;</span>example<span>&#46;</span>com</a>
<a class="reference external" href="mailto:a&#46;b&#37;&#52;&#48;c&#46;d">dot mail</a>
<a class="reference external" href="https://example.com/&#64;user">at link</a>
<code class="docutils literal notranslate"><span class="pre">mail&#64;literal.com</span></code>
```

(Inside `protect_literal_text` there is no text cloaking, only `@` → `&#64;`.)

### 3.5 Raw (unescaped) interpolations: reproduce them literally

These spots format strings **without** `encode`/`attval`. Emit them as-is; do not
"fix" them:

* problematic `'<a href="#%s">' % refid` (BASE:1494)
* label backlinks `href="#%s"` (BASE:1236-1237, 1247)
* system_message backref links (BASE:1644, 1650)
* option-argument `delimiter` (BASE:1429)
* extra-id spans (BASE:592, 596)
* video fallback `<a href="{uri}">{alt}</a>` (BASE:1204)
* flash `{alt}</object>` (BASE:1209)
* `raw` content (BASE:1512)
* `'<span class="sectnum">%s </span>' % encode(sectnum)` (encoded; BASE:1133)

---

## 4. Translator state

### 4.1 Output lists

`body` (the fragment under construction), `meta`, `head`, `title`, `subtitle`,
`docinfo`, `header`, `footer`, `body_prefix`, `body_suffix`, `body_pre_docinfo`,
`html_*` and `fragment` (BASE:327-354). Only `body`, `fragment` and `meta` matter for
Sphinx pages; `title` matters for `render_partial`.

### 4.2 `self.context`: the heterogeneous stack (BASE:358)

`depart_document` asserts it is empty (BASE:983). These are the pushes in methods
**not** overridden by Sphinx:

| visit | pushes | depart pops and emits |
|---|---|---|
| `visit_attribution` | suffix string (`''` for dash) | `pop() + '</p>\n'` |
| `visit_bullet_list` | tuple `(compact_simple, compact_p)` | restores both; emits `'</ul>\n'` |
| `visit_docinfo` | `len(body)` | slices the docinfo out of body (dead) |
| `visit_entry` | `'</th>\n'` or `'</td>\n'` | `pop()` |
| `visit_footer` / `visit_header` | `len(body)` | moves body tail into `footer`/`header` |
| `visit_problematic` | `'</a>'` or `''` | `'</span>' + pop()` |
| `visit_target` | `'</span>'` or `''` | `pop()` |
| `visit_title` (base branch) | close tag | Sphinx `depart_title` reads `context[-1]`, then base pops |

The Sphinx overrides (part 2) also push onto `self.context`:

* `visit_admonition` pushes `'</div>\n'` or `'</details>\n'`;
* the Sphinx `visit_title` branches push a close tag;
* `_visit_sig_parameter_list` pushes the closing paren;
* `visit_download_reference` pushes `'</a>'` or `''`.

### 4.3 Flags

* `section_level` (starts at 0; +1/−1 around sections)
* `compact_simple` (False) and `compact_p` (True)
* `in_mailto`
* `in_document_title` (0)
* `colspecs` (list of the current tgroup's colspecs)
* `messages` (a queue of writer-time `system_message` nodes)
* `initial_header_level`, `image_loading`, `math_output`/`math_options`,
  `math_header`
* Sphinx adds `protect_literal_text`, `_table_row_indices=[0]`,
  `_fieldlist_row_indices=[0]`, `_has_maths_elements` and others (SX:55-67)

### 4.4 Per-node scratch attributes and in-place mutations

These matter for correctness and ordering:

* `row.column` (int), set in `visit_row` and advanced by `visit_entry`
* `tgroup.stubs` (list of each colspec's `stub` attribute or `None`)
* `node.html5tagname` on `container`, `inline`, `topic` and `literal`
* `visit_field_list` **appends** `'field-list'` and possibly `'simple'` to
  `node['classes']`, and **removes** the first `field-indent-*` class
* `visit_container` **removes** an `ins`/`del` class it promotes to a tag name
* `visit_image` **removes** a `controls` class on videos
* Sphinx appends `row-odd`/`row-even` in `visit_row` (node classes) and
  `field-odd`/`field-even` in `visit_field`, sets `figure['align']='default'`, and
  rewrites `image['uri']` and `width`/`height` (part 2)

---

## 5. Override matrix

Computed programmatically from the MROs. Legend:

* **B** = `_html_base` (docutils base)
* **H5** = `html5_polyglot`
* **SX** = Sphinx `HTML5Translator`

"Spec here" means that this note specifies the output of every method in the row that
is not SX-owned.

| node | visit owner | depart owner | status |
|---|---|---|---|
| Text | SX | B (pass) | part 2 (SX:841-860) |
| abbreviation | SX | SX | part 2 |
| acronym | H5 | H5 | spec here |
| address, author, authors, contact, copyright, date, organization, revision, status, version, docinfo, docinfo_item | B/H5 | B/H5 | spec here, but **dead** (docinfo popped) |
| admonition | SX | SX | part 2 (base spec given for reference) |
| attribution | B | B | spec here |
| block_quote | SX | SX | part 2 (`<blockquote>\n<div>` … `</div></blockquote>\n`) |
| bullet_list | SX (wraps base) | B | spec here; SX only adds a toctree-only skip |
| caption | SX (wraps H5) | SX (wraps H5) | H5 part here |
| citation | B | B | spec here |
| citation_reference | B | B | spec here (dead in practice) |
| classifier, definition, term | SX | SX | part 2 |
| colspec | B | B | spec here |
| comment | SX (SkipNode) | — | nothing is emitted |
| compound | B | B | spec here |
| container | H5 | H5 | spec here |
| decoration, footer, header | B / H5 | B / H5 | spec here (contents never reach the fragment) |
| definition_list, definition_list_item | B | B | spec here |
| description | B | B | spec here |
| doctest_block | SX | B | SX highlights and raises SkipNode, so the B depart is dead |
| document | B | B | spec here |
| emphasis, strong, subscript, superscript, title_reference | B | B | spec here |
| entry, tbody, tgroup, thead | B | B | spec here |
| enumerated_list | B | B | spec here |
| field | SX | B (pass) | part 2 (SX drops the base id transfer!) |
| field_list | SX (wraps B) | SX (wraps B) | B part here |
| field_name, field_body | B | B | spec here |
| figure | SX (wraps H5) | H5 | H5 part here |
| footnote, label | B | B | spec here |
| footnote_reference | SX | B | part 2; depart here |
| generated | B | B | spec here |
| image | SX (wraps B) | SX (no-op) | B part here |
| inline | H5 | H5 | spec here (with SX's empty `supported_inline_tags`) |
| legend | H5 | H5 | spec here |
| line, line_block, list_item | B | B | spec here |
| literal | SX | SX | part 2 |
| literal_block | SX (falls back to B for parsed-literal) | B | B part here |
| math, math_block | SX | SX | part 2 (docutils path documented as dead) |
| meta | H5 | H5 | spec here |
| option*, option_group, option_list(_item) | B | B | spec here |
| paragraph | B | B | spec here |
| problematic | B | B | spec here |
| raw | B | — | spec here |
| reference | SX | B | part 2; depart here |
| row | SX | B | part 2; depart here |
| rubric | SX (wraps B) | SX (wraps B) | B part here |
| section | H5 | H5 | spec here |
| sidebar, topic | H5 | H5 | spec here |
| subtitle | B | B | spec here |
| substitution_definition, substitution_reference | B | — | spec here |
| system_message | B | B | spec here |
| table | SX | SX (wraps B) | part 2; B depart here |
| target | B | B | spec here |
| title | SX (wraps B) | SX (wraps B) | B part here |
| transition | B | B | spec here |

Sphinx-only methods (part 2): `acks`, `attention`, `caution`, `centered`,
`compact_paragraph`, `danger`, `desc*`, `download_reference`, `error`, `glossary`,
`hint`, `hlist`, `hlistcol`, `important`, `index` (SkipNode), `literal_emphasis`,
`literal_strong`, `manpage`, `note`, `number_reference`, `production`,
`productionlist`, `seealso`, `start_of_file`, `tabular_col_spec` (SkipNode), `tip`,
`toctree` (SkipNode), `versionmodified`, `warning`.

---

## 6. The "compact"/"simple" logic

### 6.1 Paragraphs (BASE:1482-1490)

In html5 there is **no** `should_be_compact_paragraph`; that exists only in html4css1,
which Sphinx does not use. `<p>` is always emitted.

* `visit_paragraph`: `starttag(node, 'p', '')`, so `<p>` has no newline after it.
* `depart_paragraph`: appends `'</p>'`. **Unless** `node.parent` is a `list_item` or
  `entry` **and** `len(node.parent) == 1`, it also appends `'\n'` and then calls
  `report_messages(node)`.

`len(parent)` counts **all** children, including invisible ones (comment, target,
substitution_definition, and Sphinx `index`), even though those emit nothing. Verified:

```html
<li><p>simple one</p></li>                  ← sole child
<li><p>a</p>
</li>                                        ← list_item = [paragraph, comment]
<li><p>list with target inside</p>
<span class="target" id="inner-trailing"></span></li>
<td><p>a</p></td>                            ← entry with one paragraph
<td><p>v2</p>
<p>multi para</p>
</td>
<dd><p>Def.</p>
</dd>                                        ← definition/field_body/description always get \n
```

`compact_p` is saved and restored by bullet lists (BASE:759-767) but **never read** in
html5. It exists only for bookkeeping parity.

### 6.2 `is_compactable(node)` (BASE:737-754)

```
if 'compact' in node['classes']: return True
if 'open'    in node['classes']: return False
if node is field_list|definition_list and not settings.compact_field_lists: return False
if node is enumerated_list|bullet_list and not settings.compact_lists:      return False
if 'contents' in node.parent['classes']: return True        # table of contents
return check_simple_list(node)                               # SimpleListChecker walk
```

The `contents` rule is checked **after** `compact_lists`. With
`html_compact_lists=False`, even the `.. contents::` list loses `simple` (verified in
probe `src2`, where every `<ul>`/`<ol>` lost `simple` while `<dl … simple>` stayed).

### 6.3 `SimpleListChecker` (BASE:1820-1895)

This is a `GenericNodeVisitor`, walked over the list node **including the node
itself**. It raises `NodeFound`, which means "not simple", from `default_visit`. The
per-type handlers are:

* **pass** (continue into children): `bullet_list`, `enumerated_list`, `docinfo`,
  `definition_list`, `definition_list_item`, `classifier`, `field_list`, `field`,
  `contact`
* **ignore** (SkipNode, never complex): `Text`, `paragraph`, `author`, `copyright`,
  `date`, `organization`, `status`, `term`, `field_name`, `comment`,
  `substitution_definition`, `target`, `pending`
* **`visit_list_item` rule** (also used for `authors`, `address`, `version`,
  `definition` and `field_body`):
  * take `children = [c for c in node.children if not Invisible(c)]`;
  * if `children` is non-empty, `children[0]` is a `paragraph`, and `children[-1]` is
    a `bullet_list`, `enumerated_list` or `field_list`, pop the last one;
  * if `len(children) <= 1`, return and continue walking (the nested list is then
    checked recursively); otherwise raise `NodeFound`.
* The pop rule pops only a trailing `bullet_list`/`enumerated_list`/`field_list`.
  A list item that is a paragraph followed by a **`definition_list`** therefore has
  2 counted children and fails. A list item whose *only* child is a definition list
  passes the count, and the nested list is then checked as its own pass-node.
* **Everything else raises `NodeFound`.** That includes `literal_block`,
  `block_quote`, `system_message`, `image`, `note` and the other admonitions, `table`,
  `line_block`, and every **Sphinx node**. Sphinx registers its nodes with
  `GenericNodeVisitor` (`sphinx.util.docutils.register_node`), so `index`,
  `compact_paragraph`, `pending_xref` and the rest reach `default_visit`.
  Consequences, both verified:
  * a list item containing an `.. index::` directive makes the whole list
    **non-simple**: `<ul>` in probe `src2`;
  * Sphinx toctree `<ul>`s (whose items hold `compact_paragraph`) are never `simple`.

Note that invisible children are filtered only for the *count*. The walk still visits
them, and `target`, `comment`, `substitution_definition` and `pending` are
ignore-listed, but Sphinx's `index` (Invisible) is **not**, so it fails the list.

### 6.4 Where `simple` lands

* **bullet_list** (BASE:756-768; SX:469-473 adds only a skip):
  ```
  old = self.compact_simple
  context.push((compact_simple, compact_p)); compact_p = None
  compact_simple = is_compactable(node)
  atts['class'] = 'simple' if compact_simple and not old
  body += starttag(node, 'ul', **atts)            # '\n' suffix
  depart: (compact_simple, compact_p) = pop(); body += '</ul>\n'
  ```
  A bullet list nested (at any depth) inside a compact bullet list does **not** get
  `simple`, because `old_compact_simple` is True. `compact_simple` is only saved and
  restored by bullet lists, so an enumerated list in between does not reset it.
  Verified: `<ul class="simple">` … nested `<ul>`. With the class ordering from 3.3 the
  result is `class="<node classes> simple"`. Sphinx's skip: if `len(node) == 1` and
  `node[0]` is a `toctree`, SkipNode emits nothing and pushes nothing.
* **enumerated_list** (BASE:1014-1025): `atts = {'classes': []}`. If `'start' in node`,
  `atts['start'] = node['start']`; the int prints as-is, and the parser only sets it
  when it is not 1. If `'enumtype' in node`, append the enumtype. If
  `is_compactable(node)`, append `'simple'`. Emit `starttag(node, 'ol', **atts)`, and
  on depart `'</ol>\n'`. `simple` depends **only** on `is_compactable`, not on
  `compact_simple`, so a nested compactable `<ol>` inside a compact `<ul>` still gets
  `simple`. Verified: `<ol class="arabic simple">` nested in `<ul class="simple">`.
  The enumtypes are `arabic`, `loweralpha`, `upperalpha`, `lowerroman` and
  `upperroman`. `prefix` and `suffix` are **not** rendered.
* **definition_list** (BASE:889-900): if `'details'` is in the node classes, emit
  `starttag(node, 'div')` (see 7.4). Otherwise emit
  `starttag(node, 'dl', classes=['simple'] if is_compactable(node) else [])`, and on
  depart `'</dl>\n'`.
* **field_list**: see 7.5. **docinfo**: see 7.15.

---

## 7. Per-node output spec (methods Sphinx does not override)

Notation:

* `ST(node, 'x', sfx, …)` means `starttag` as in 3.3, with sfx defaulting to `'\n'`.
* `ET(…)` means `emptytag`.
* Strings are Python literals.
* **V** = visit output, **D** = depart output.

### 7.1 Document and sections

* **document** (BASE:951-983)
  * **V**: appends `<title>…</title>\n` to `self.head` (not the fragment).
  * **D**: builds head and prefix parts; `fragment.extend(body)`; asserts the context
    is empty. Emits nothing into the fragment.
* **section** (H5:343-350)
  * **V**: `section_level += 1`; `ST(node, 'section')` gives
    `<section id="…">\n`, plus any extra-id spans after the `\n`, plus the node
    classes (verified `<section class="cls1 cls2" id="heading-with-class">`).
  * **D**: `section_level -= 1`; `'</section>\n'`.
* **title in a section** (BASE:1751-1781 → `section_title_tags` BASE:1732-1749, H5
  wrapper :388-398):
  ```
  h_level = section_level + initial_header_level - 1    # = section_level under Sphinx
  tagname = 'h%i' % min(h_level, 6)
  atts = {'aria-level': h_level} if h_level > 6 else {}
  start = ST(node, tagname, '', **atts)                 # title node's own ids/classes
  if node.hasattr('refid'):                             # set by the docutils Contents transform (toc_backlinks='entry')
      start += ST(nodes.reference(), 'a', '', class='toc-backref', role='doc-backlink', href='#'+refid)
      close = '</a></%s>\n' % tagname
  else:
      close = '</%s>\n' % tagname
  # H5: section_self_link is False → no self-link
  context.push(close)
  ```
  Sphinx then appends the secnumber span (`<span class="section-number">1.2. </span>`),
  and in depart the headerlink (part 2). Verified at depth 1-8:
  ```html
  <h1>Top Title<a class="headerlink" …>¶</a></h1>
  <h6>Level Six…</h6>
  <h6 aria-level="7">Level Seven<a class="headerlink" href="#level-seven" title="Link to this heading">¶</a></h6>
  <h6 aria-level="8">Level Eight…</h6>
  <h2><a class="toc-backref" href="#id1" role="doc-backlink"><span class="sectnum">1.1 </span>Sub A</a><a class="headerlink" href="#sub-a" title="Link to this heading">¶</a></h2>
  ```
  The `toc-backref` case: Sphinx `depart_title` (SX:564-570) sees a close tag starting
  with `</a></h`, emits `'</a><a class="headerlink" href="#ID" title="Link to this heading">¶'`,
  and the base then emits the `'</a></h2>\n'` close. The permalink points at
  `section['ids'][0]`.
* **title, other parents** (BASE:1752-1775): the close tag is `'</p>\n'` unless noted.
  * `topic` parent: `ST(node,'p','',CLASS='topic-title')`. If `toc_backlinks` is truthy
    and `'contents'` is in the topic classes, append
    `'<a class="reference internal" href="#top">'` and use close `'</a></p>\n'`.
    **Sphinx removes that `<a>` again (SX:541-551)**, so the verified output is
    `<p class="topic-title">Contents Title</p>`.
  * `sidebar`: `<p class="sidebar-title">`.
  * Admonition (incl. `seealso`, `desc`, `versionmodified`, `productionlist`):
    `<p class="admonition-title">`. Sphinx inserts the title node for named
    admonitions and uses `<summary>` for collapsible ones (part 2).
  * `table`: `ST(node,'caption','')` and close `'</caption>\n'`. Sphinx adds
    `<span class="caption-text">` and the permalink (part 2).
  * `document`: `ST(node,'h1','',CLASS='title')`, close `'</h1>\n'`, and it sets
    `in_document_title = len(body)`. **Dead for pages** (`doctitle_xform=False`), but
    **live in `render_partial`**: depart moves the whole body into
    `body_pre_docinfo`/`html_title` and sets `self.title = body[start:-1]`, which is the
    inner HTML without `<h1 …>` and `</h1>\n`.
* **title depart** (BASE:1782-1789): `body += context.pop()`, plus the
  document-title capture above.
* **subtitle** (BASE:1612-1629): `ST(node,'p','',classes=[…])`, where the class is
  `sidebar-subtitle` in a sidebar, `subtitle` in the document (dead), or
  `section-subtitle` in a section (dead: `sectsubtitle_xform=False`). **D**:
  `'</p>\n'`. Verified `<p class="sidebar-subtitle">Sidebar Sub</p>`.
* **transition** (BASE:1803-1807): `ET(node,'hr',CLASS='docutils')` gives
  `<hr class="docutils" />\n`.
* **topic** (H5:365-385):
  * `contents` in classes: tag `nav`, only the node classes (the `topic` class is
    dropped), plus `role="doc-toc"` **iff** the parent is `document`. That case also
    rewrites `body_prefix[0]`, which has no fragment effect.
  * `abstract`: `div` + `role="doc-abstract"`.
  * `dedication`: `div` + `role="doc-dedication"`.
  * otherwise `aside`.
  * The class list is `classes=['topic']` + node classes, except in the nav case.
    Suffix `'\n'`. **D**: `'</{tag}>\n'`.

  Verified:
  ```html
  <nav class="contents" id="contents" role="doc-toc">          ← contents before the first section
  <nav class="contents local" id="contents-title">             ← :local: inside a section
  <aside class="topic">
  <div class="topic dedication" role="doc-dedication">          ← from :dedication: docinfo field
  <div class="topic abstract" role="doc-abstract">
  ```
* **sidebar** (H5:353-360): `ST(node,'aside',CLASS='sidebar')`, so
  `<aside class="sidebar">\n`; **D** `'</aside>\n'`.
* **rubric**, base part (BASE:1558-1562; SX:580-601 uses it when there is no valid
  `heading-level`): `ST(node,'p','',CLASS='rubric')` … `'</p>\n'`. Verified
  `<p class="rubric">A Rubric</p>` and `<p class="rc rubric">Rubric</p>`. With
  `:heading-level: 3`, Sphinx emits `<h3 class="rubric">…</h3>\n`.
* **compound** (BASE:844-848): `ST(node,'div',CLASS='compound')`, e.g.
  `<div class="compound">\n` or `<div class="toctree-wrapper compound">\n`;
  **D** `'</div>\n'`.
* **container** (H5:166-182): if exactly one class is in `{'ins','del'}`, that class is
  removed from the node and becomes the tag; otherwise the tag is `div`. Emit
  `ST(node, tag, CLASS='docutils container')`; **D** `'</{tag}>\n'`. Verified:
  * `<div class="custom-class other docutils container">`
  * `<ins class="docutils container">`
  * `<del class="other docutils container">`
  * the Sphinx code-caption wrapper
    `<div class="literal-block-wrapper docutils container" id="code-name">`
* **decoration** (BASE:875-879): nothing.
* **header** and **footer** (H5:218-241): the visit pushes `len(body)`. The depart cuts
  `body[start:]`, wraps it in `<header>`…`</header>\n` or `<footer>`…`</footer>\n`, and
  moves it to `self.header`/`body_prefix` or `self.footer`/`body_suffix`. **Nothing
  stays in the fragment.** Verified: `.. header:: Header text` produces no trace in the
  page.

### 7.2 Inline elements

| node | V | D | ref |
|---|---|---|---|
| emphasis | `ST(n,'em','')` → `<em>` | `</em>` | BASE:985-989 |
| strong | `<strong>` | `</strong>` | BASE:1589-1593 |
| subscript | `<sub>` | `</sub>` | BASE:1595-1599 |
| superscript | `<sup>` | `</sup>` | BASE:1631-1635 |
| title_reference | `<cite>` | `</cite>` | BASE:1791-1795 |
| acronym (H5) | `<abbr>` (no title attr) | `</abbr>` | H5:135-140 |
| inline (H5 + SX `supported_inline_tags=set()`) | `ST(n,'span','')`, always `span` under Sphinx (the `ln` special case needs a docutils `code` literal_block, which Sphinx never renders through the base) | `</span>` | H5:249-277, SX:53 |
| generated | if `'sectnum'` is in classes: `'<span class="sectnum">%s </span>' % encode(astext().rstrip(' '))` then SkipNode; otherwise nothing, and the children render | nothing | BASE:1129-1139 |
| problematic | `'<a href="#REFID">'` if `refid`, else nothing; then `ST(n,'span','',CLASS='problematic')` | `'</span>' + ('</a>' or '')` | BASE:1492-1502 |
| target | if none of `refuri`/`refid`/`refname`: `ST(n,'span','',CLASS='target')` and push `'</span>'`; else push `''` | `pop()` | BASE:1682-1692 |
| substitution_definition | SkipNode (nothing) | — | BASE:1601-1603 |
| substitution_reference | `NotImplementedError` (never reached; the reader substitutes or makes a `problematic`) | — | BASE:1605-1606 |
| footnote_reference (depart) | — | `'<span class="fn-bracket">]</span>'` + `'</a>'` | BASE:1124-1126 |
| citation_reference | `ST(n,'a','[',href='#'+(refid or nameids[refname]),classes=['citation-reference'],role='doc-biblioref')` | `']</a>'` | BASE:793-806; **dead in Sphinx** |
| reference (depart) | — | `'</a>'` + (`'\n'` if the parent is **not** TextElement); `in_mailto=False` | BASE:1539-1543 |

Verified snippets:

```html
<p><em>emph</em> <strong>strong</strong> … <cite>title ref</cite> <sub>sub</sub> <sup>sup</sup>
<p>An <span class="target" id="inline-target">inline target</span> and a ref to <a class="reference internal" href="#inline-target">inline target</a>.</p>
<p><span class="custom">custom role text</span></p>
<p><a href="#id1"><span class="problematic" id="id2">:unknownrole:`oops`</span></a> and <a href="#id5"><span class="problematic" id="id6">|undefined|</span></a> and <a href="#id7"><span class="problematic" id="id8">`bad ref`_</span></a>.</p>
<h1><span class="sectnum">1 </span>Misc<a class="headerlink" …>¶</a></h1>
```

`problematic` detail: with `keep_warnings=False` the `system_message` that `refid`
points to has been removed (`FilterSystemMessages`), but the `<a href="#id1">` is
still emitted. It is a dangling link, and it is what Sphinx really produces.

Block-level `target` detail: a target that was **not** propagated (it is trailing, or
followed only by invisible nodes) keeps its ids and has no `refid`, so it renders
`<span class="target" id="X"></span>` with **no newline**. Verified:

```html
<span class="target" id="trailing-target"></span></section>
```

A target with `refuri` (for example `.. _external: https://…`) or with `refid` (a
propagated one) emits **nothing**, and its ids are therefore absent from the HTML.

### 7.3 Paragraph-like and quote blocks

* **paragraph**: see 6.1.
* **attribution** (BASE:682-694):
  `prefix, suffix = {'dash': ('—',''), 'parentheses'|'parens': ('(',')'), 'none': ('','')}[settings.attribution]`.
  **V**: push the suffix, then `ST(node,'p',prefix,CLASS='attribution')`; the prefix is
  passed as the starttag's *suffix*. **D**: `pop() + '</p>\n'`. Verified inside Sphinx's
  block_quote wrapper:
  ```html
  <blockquote>
  <div><p>Quoted text.</p>
  <p class="attribution">—Attribution Name</p>
  </div></blockquote>
  <blockquote class="epigraph">
  ```
  (`block_quote` itself is SX: `ST(node,'blockquote') + '<div>'` … `'</div></blockquote>\n'`.)
* **line_block** (BASE:1266-1270): `ST(node,'div',CLASS='line-block')` →
  `<div class="line-block">\n` … `'</div>\n'`. Nested line blocks nest.
* **line** (BASE:1258-1264): `ST(node,'div','',CLASS='line')`, plus `'<br />'` if the
  line has no children; **D** `'</div>\n'`. Verified:
  ```html
  <div class="line-block">
  <div class="line">Line one</div>
  <div class="line-block">
  <div class="line">indented line</div>
  <div class="line"><br /></div>
  </div>
  <div class="line">after empty</div>
  </div>
  <div class="lb line-block">
  ```
* **literal_block**, base path (BASE:1307-1315). Sphinx uses it only when
  `node.rawsource != node.astext()`, the parsed-literal heuristic at SX:604-607.
  * **V**: `ST(node,'pre','',CLASS='literal-block')`, plus `'<code>'` if `'code'` is in
    the classes.
  * **D**: `'</code>'` if `'code'`, then `'</pre>\n'`.
  * The children render normally, with Text through Sphinx's `visit_Text` and inline
    markup inside.
  * Verified: `<pre class="literal-block">parsed <em>emph</em> literal</pre>` and
    `<pre class="literal-block">with &lt;angle&gt; &amp; amp <em>emph</em></pre>`.
  * **Crate risk:** the crate keeps no `rawsource` (grep finds none under `src/`). A
    `.. parsed-literal::` **without** any markup has `rawsource == astext()` and is
    therefore **highlighted like a code block**. Verified: it produces
    `<div class="highlight-default notranslate">…`. The writer needs a faithful
    substitute for the `rawsource != astext()` test (part 2 / parser).
* **doctest_block**: SX `visit_doctest_block` calls `visit_literal_block`, which
  highlights and raises SkipNode. The base depart `'\n</pre>\n'` (BASE:944-949) is
  dead. Verified output: `<div class="doctest highlight-default notranslate">…`.
* **raw** (BASE:1504-1516): only when `'html' in node.get('format','').split()`.
  * tag = `span` if the parent is a TextElement, else `div`;
  * if the node has classes: `ST(node, tag, suffix='')` + `astext()` + `'</tag>'`;
  * otherwise just `astext()`, **raw and without any newline**;
  * always SkipNode.

  Verified:
  ```html
  <div class="rawblock">raw</div><p>Inline raw: …           ← block raw, no trailing \n
  <div class="rawcls"><p>raw with class</p></div><div class="docutils container extra">
  <span>html-latex</span><table …                           ← format "html latex"
  <p>Inline raw after def: <span class="raw-html"><b>y</b></span></p>   ← role-derived class → span wrapper
  <p>Line <br/> break.</p>                                   ← raw via substitution, no class
  ```
  A `format: latex`-only raw emits nothing.

### 7.4 Definition lists (with the Sphinx term/classifier/definition overrides in context)

* `definition_list` → `<dl>`/`<dl class="simple">`/`<dl class="simple foo">` + `\n`;
  `</dl>\n` (6.4).
* `definition_list_item` (BASE:903-912): nothing, **unless** the parent list has class
  `details`. In that case it emits `ST(node,'details', open='open' if 'open' in
  parent classes)` and on depart `'</details>\n'`.
* The Sphinx overrides (part 2) produce `<dt>` + term + `</dt>` with no newline, then
  `<dd>` … `</dd>\n`, and for classifiers `<span class="classifier">…</span>`. Because
  Sphinx overrides term and definition but **not** the `details` wrapper, a
  `details`-class list yields a mixed structure. Verified:
  ```html
  <dl class="simple">
  <dt>Term one</dt><dd><p>Definition one.</p>
  </dd>
  <dt>Term two<span class="classifier">classifier</span></dt><dd><p>Definition two.</p>
  <p>Second para.</p>
  </dd>
  <dt>Term three<span class="classifier">c1</span><span class="classifier">c2</span></dt><dd>…
  </dl>
  <div class="details open">
  <details open="open">
  <dt>Open summary</dt><dd><p>Details body.</p>
  </dd>
  </details>
  </div>
  ```
  (The first `<dl>` in that probe had no `simple` because one definition held two
  paragraphs. A list whose definitions each hold one paragraph gets `<dl class="simple">`.)

### 7.5 Field lists

* **field_list** (BASE:1027-1046; SX:979-985 pushes/pops `_fieldlist_row_indices`):
  ```
  classes = node.setdefault('classes', [])
  for i, cls in enumerate(classes):
      if cls.startswith('field-indent-'):
          try: indent = length_or_percentage_or_unitless(cls[13:], 'px')
          except ValueError: break
          atts['style'] = '--field-indent: %s;' % indent
          classes.pop(i); break
  classes.append('field-list')
  if is_compactable(node): classes.append('simple')
  body += ST(node, 'dl', **atts)            # '\n'
  depart: '</dl>\n'
  ```
  `length_or_percentage_or_unitless(v, 'px')` appends `px` to unitless values.
* **field** (SX:987-992): only appends `field-odd`/`field-even` to `field['classes']`
  (a per-list counter starting at 1, so the first field is odd). The base
  `visit_field` (BASE:1048-1053) would copy `field['ids']` onto `field_name`; **Sphinx
  does not do that, so a field's own ids are never rendered.** Depart is the base
  `pass`.
* **field_name** (BASE:1059-1064): `ST(node,'dt','',classes=node.parent['classes'])`,
  where the kwarg list is the field's classes (aliased, see 3.3). **D**:
  `'<span class="colon">:</span></dt>\n'`.
* **field_body** (BASE:1066-1074): `ST(node,'dd','',classes=node.parent['classes'])`,
  plus `'<p></p>'` if it has no children. **D**: `'</dd>\n'`.

Verified:

```html
<dl class="field-list simple">
<dt class="field-odd">Simple<span class="colon">:</span></dt>
<dd class="field-odd"><p>field</p>
</dd>
<dt class="field-even">Other<span class="colon">:</span></dt>
<dd class="field-even"><p>field</p>
</dd>
</dl>
<dt class="field-odd">Empty field<span class="colon">:</span></dt>
<dd class="field-odd"><p></p></dd>
<dl class="field-list simple" style="--field-indent: 10em;">
```

### 7.6 Option lists (BASE:1419-1459)

| node | V | D |
|---|---|---|
| option_list | `ST(n,'dl',CLASS='option-list')` → `<dl class="option-list">\n` | `</dl>\n` |
| option_list_item | — | — |
| option_group | `ST(n,'dt','')` + `'<kbd>'` | `'</kbd></dt>\n'` |
| option | `ST(n,'span','',CLASS='option')` | `'</span>'` + `', '` if the **next sibling** is an `option` |
| option_string | — (text renders) | — |
| option_argument | `node.get('delimiter',' ')` (raw) + `ST(n,'var','')` | `'</var>'` |
| description | `ST(n,'dd','')` | `'</dd>\n'` |

Verified:

```html
<dl class="option-list">
<dt><kbd><span class="option">-a</span></kbd></dt>
<dd><p>option a</p>
</dd>
<dt><kbd><span class="option">-b <var>FILE</var></span></kbd></dt>
<dt><kbd><span class="option">--long=<var>VALUE</var></span></kbd></dt>
<dt><kbd><span class="option">-c</span>, <span class="option">--count</span></kbd></dt>
<dt><kbd><span class="option">/V</span></kbd></dt>
```

### 7.7 Bullet and enumerated list items

* **list_item** (BASE:1272-1276): `ST(node,'li','')` → `<li>` (or `<li id="index-0">`);
  **D** `'</li>\n'`.

Verified mix:

```html
<ul class="simple">
<li><p>nested</p>
<ul>
<li><p>inner a</p></li>
<li><p>inner b</p></li>
</ul>
</li>
</ul>
<ol class="arabic simple" start="3">
<ol class="loweralpha simple">
<ol class="lowerroman simple">
<ul class="open">                       ← .. rst-class:: open
<ul class="compact simple">             ← .. rst-class:: compact (two paragraphs, still simple)
<ol class="arabic">                     ← complex enumerated
```

### 7.8 Footnotes, citations and labels (html5: `<aside>`/DPub roles)

* **footnote** (BASE:1100-1115):
  * **V**: if the previous sibling is not a `footnote`, first emit
    `'<aside class="footnote-list brackets">\n'`, with the label style taken from
    `settings.footnote_references`. Then
    `ST(node,'aside',classes=['footnote', 'brackets'],role='doc-footnote')`.
  * **D**: `'</aside>\n'`, plus a second `'</aside>\n'` if the next sibling is not a
    `footnote`.
  * The test is an exact type match on the immediate siblings, so any node in between
    (comment, target) splits the group.
* **citation** (BASE:777-790): if the previous sibling is not a `citation`, emit
  `'<div role="list" class="citation-list">\n'`. This is a literal string, so
  **role comes before class**. Then
  `ST(node,'div',classes=['citation'],role='doc-biblioentry')`. **D**: `'</div>\n'`,
  plus `'</div>\n'` if the next sibling is not a `citation`.
* **label** (BASE:1229-1250), with `footnote_backlinks=True` and
  `backrefs = parent.get('backrefs', [])`:
  * **V**: `'<span class="label">' + '<span class="fn-bracket">[</span>'`, plus
    `'<a role="doc-backlink" href="#%s">' % backrefs[0]` if `len(backrefs) == 1`.
  * The label text renders next.
  * **D**: `'</a>'` if 1 backref; then `'<span class="fn-bracket">]</span></span>\n'`.
    If there is more than 1 backref, append
    `'<span class="backrefs">(%s)</span>\n' % ','.join('<a role="doc-backlink" href="#%s">%d</a>')`,
    joined with a comma and no space.
* **footnote_reference**: see SX:1026-1034 (part 2):
  `ST(n,'a','',classes=['footnote-reference','brackets'],role='doc-noteref',href='#'+refid)`
  + `'<span class="fn-bracket">[</span>'`, and the base depart closes it.

Verified:

```html
<p>Footnote ref <a class="footnote-reference brackets" href="#f1" id="id1" role="doc-noteref"><span class="fn-bracket">[</span>1<span class="fn-bracket">]</span></a> and … cite <a class="reference internal" href="#cit2002" id="id5"><span>[CIT2002]</span></a> .</p>
<aside class="footnote-list brackets">
<aside class="footnote brackets" id="f1" role="doc-footnote">
<span class="label"><span class="fn-bracket">[</span>1<span class="fn-bracket">]</span></span>
<span class="backrefs">(<a role="doc-backlink" href="#id1">1</a>,<a role="doc-backlink" href="#id6">2</a>)</span>
<p>First footnote.</p>
</aside>
<aside class="footnote brackets" id="id7" role="doc-footnote">
<span class="label"><span class="fn-bracket">[</span><a role="doc-backlink" href="#id2">3</a><span class="fn-bracket">]</span></span>
<p>Auto footnote.</p>
</aside>
…
</aside>
<div role="list" class="citation-list">
<div class="citation" id="cit2002" role="doc-biblioentry">
<span class="label"><span class="fn-bracket">[</span><a role="doc-backlink" href="#id5">CIT2002</a><span class="fn-bracket">]</span></span>
<p>A citation.</p>
</div>
<div class="citation" id="cit2003" role="doc-biblioentry">
<span class="label"><span class="fn-bracket">[</span>CIT2003<span class="fn-bracket">]</span></span>
<p>Another citation.</p>
</div>
</div>
```

Citation *references* are not rendered by `visit_citation_reference` under Sphinx:
Sphinx's citation domain turns them into `pending_xref` and then into
`reference(internal) > inline('[CIT2002]')`, which gives the `<a class="reference
internal" … id="id5"><span>[CIT2002]</span></a>` above. The footnote label `*` for
symbol footnotes renders raw (`*`).

### 7.9 Tables

* **table**: SX `visit_table` (SX:948-964, part 2):
  * `classes = ['docutils'] + [c.strip(' \t\n') for c in table_style.split(',')] + ['align-'+node.get('align','default')]`,
    passed as a CLASS string (empty entries vanish through `.split()`);
  * `style="width: {width}"` with **no** `px` default and **no** semicolon;
  * the table-internal `colwidths-*` classes are filtered by `starttag`;
  * node classes come before the CLASS words;
  * extra ids become **prefix** spans (table rule).

  **D** (SX:966-968 → BASE:1678-1680): `'</table>\n'` + `report_messages(node)`.
* **title → `<caption>`**: 7.1.
* **tgroup** (BASE:1719-1724): **V**: `self.colspecs = []`; `node.stubs = []`. No
  output.
* **colspec** (BASE:815-835):
  * **V**: `colspecs.append(node)`; `node.parent.stubs.append(node.get('stub'))`.
  * **D**: return if the next sibling is also a `colspec`. Return also if
    `'colwidths-auto'` is in the **table** classes, or if (`'colwidths-grid'` is not
    in `table_style` **and** `'colwidths-given'` is not in the table classes). Under
    Sphinx that means: **a colgroup is emitted iff the table has class
    `colwidths-given`**, which the parser sets for explicit `:widths:` (the crate
    already sets it: `src/rst/block.rs:6244-6282, 6682-6685`).
  * Otherwise emit `ST(node,'colgroup')` (**the last colspec's** ids/classes land on
    `<colgroup>`), then for each colspec
    `ET(colspec,'col',style=f'width: {propwidth/total:.1%}')`, then `'</colgroup>\n'`.
  * `propwidth()` is `validate_colwidth(node.get('colwidth',''))`: an int or float
    value is taken as-is; `''`/`'*'` count as 1; a string goes through
    `parse_measure(…, '[*]?')` (`docutils/nodes.py:2431-2442, 3191-3213`).
  * Verified: `<col style="width: 30.0%" />`, `16.7%`, `33.3%`, `50.0%`.
* **thead** / **tbody** (BASE:1695-1699, 1726-1730): `ST(n,'thead')` → `<thead>\n` …
  `'</thead>\n'`, and the same for `<tbody>`.
* **row**: SX (SX:970-977) numbers rows across the whole table (thead and tbody share
  one counter starting at 1): odd rows get `row-odd`, even rows get `row-even`. The
  class is appended to the node, then `ST(n,'tr','')` and `node.column = 0`. **D**
  (BASE:1555-1556): `'</tr>\n'`.
* **entry** (BASE:991-1012):
  ```
  classes = []
  if isinstance(node.parent.parent, nodes.thead): classes.append('head')
  if node.parent.parent.parent.stubs[node.parent.column]: classes.append('stub')
  tag = 'th' if classes else 'td'
  node.parent.column += 1
  if 'morerows' in node: atts['rowspan'] = node['morerows'] + 1
  if 'morecols' in node: atts['colspan'] = node['morecols'] + 1; node.parent.column += node['morecols']
  body += ST(node, tag, '', classes=classes, **atts); context.push('</%s>\n' % tag)
  depart: body += context.pop()
  ```
  The column counter does not account for cells covered by a rowspan from an earlier
  row. That is bug-compatible behavior; it only matters for stub columns combined with
  rowspans, which no rST table directive can produce together. A cell's paragraphs
  follow 6.1. An empty cell renders as `<td></td>`.

Verified (probe `src/tables.rst` and `src2/index.rst`):

```html
<table class="docutils align-default">
<thead>
<tr class="row-odd"><th class="head"><p>H1</p></th>
<th class="head"><p>H2</p></th>
</tr>
</thead>
<tbody>
<tr class="row-even"><td><p>a</p></td>
<td><p>b</p></td>
</tr>
<tr class="row-odd"><td colspan="2"><p>c spans</p></td>
</tr>
</tbody>
</table>
<table class="docutils align-center" id="tbl-name" style="width: 50%">
<caption><span class="caption-text">Table Title</span><a class="headerlink" href="#tbl-name" title="Link to this table">¶</a></caption>
<colgroup>
<col style="width: 30.0%" />
<col style="width: 70.0%" />
</colgroup>
<thead>
…
<table class="myclass docutils align-default" id="id1">
…
<tr class="row-odd"><th class="head stub"><p>Stub</p></th>
…
<tr class="row-even"><th class="stub"><p>r1</p></th>
<td><p>v1</p></td>
<td><p>v2</p>
<p>multi para</p>
</td>
</tr>
<tr class="row-odd"><th class="stub"><p>r2</p></th>
<td><p>v3</p></td>
<td></td>
</tr>
<table class="docutils align-default" style="width: 300">          ← unitless width kept verbatim
<tr class="row-odd"><td rowspan="2"><p>a</p></td>
<td colspan="2"><p>b</p></td>
<tr class="row-odd"><td rowspan="2"><p>row
span</p></td>
```

Two header rows are numbered odd, even, and then the first body row is odd. The CSV
table with `:widths: auto` (`colwidths-auto`) and plain grid/simple tables have **no**
colgroup.

### 7.10 Images and figures

* **image**: Sphinx pre-processing in SX:771-796 (part 2 details):
  1. If `uri in builder.images`, rewrite `node['uri'] = posixpath.join(imgpath, quote(images[uri]))`,
     for example `_images/pic.png` or `../_images/pic.png`.
  2. If `'scale' in node` and not both width and height are set, fill the missing ones
     from the source file's pixel size (`sphinx.util.images.get_image_size`, which uses
     the `imagesize` library) as **unitless strings**. On failure it warns
     `Could not obtain image size. :scale: option is ignored.`

  Then the docutils base `visit_image` (BASE:1153-1217):
  ```
  uri = node['uri']; alt = node.get('alt', uri)          # alt defaults to the (rewritten) uri
  mimetype = mimetypes.guess_type(uri)[0]
  atts = image_size(node)                                # see below
  if 'align' in node: atts['classes'] = ['align-' + node['align']]
  loading = 'link' if mimetype in videotypes else self.image_loading   # 'link'
  loading = node.get('loading', loading)
  if loading == 'lazy': atts['loading'] = 'lazy'
  elif loading == 'embed': read uri2path(uri) (relative to the process CWD!) → data: URI / inline SVG,
       on error queue reporter.error('Cannot embed image "<uri>":\n  <OSError str>')
  suffix = '\n' if (parent not TextElement) or (parent is reference and grandparent not TextElement) else ''
  video (video/mp4|webm|ogg): atts['title'] = alt; 'controls' class → removed, atts['controls']='controls';
        element = ST(node,'video',suffix,src=uri,**atts) + f'<a href="{node["uri"]}">{alt}</a>{suffix}' + f'</video>{suffix}'
  flash ('application/x-shockwave-flash' — unreachable on Python 3.12, see §9.7):
        ST(node,'object','',data=uri,type=mimetype,**atts) + f'{alt}</object>{suffix}'
  embedded svg: prepared svg + suffix
  else: atts['alt'] = alt; element = ET(node,'img',suffix,src=uri,**atts)
  body += element
  if suffix: report_messages(node)                        # writer-time system messages go right here
  ```
  `depart_image` does nothing (the SX override is a no-op for svg, and the base passes).
* **`image_size(node)`** (BASE:419-453):
  ```
  measures = {}                                   # insertion order: width, then height
  for dim in ('width','height'): if dim in node: measures[dim] = parse_measure(node[dim])
      # parse_measure: fullmatch '(-?[0-9.]+) *([a-zA-Zµ]*|%?)'; int() if possible else float()
  if 'scale' in node and len(measures) < 2:
      size = read_size_with_PIL(node)             # queues a warning on failure (see below)
      if size: fill the missing dims as (value, '')
  f = node.get('scale', 100) / 100               # true division → float
  for dim, (value, unit) in measures.items():
      value *= f
      if unit: declarations.append(f'{dim}: {value:g}{unit};')
      else:    size_atts[dim] = f'{round(value)}'          # banker's rounding, int
  if declarations: size_atts['style'] = ' '.join(declarations)
  ```
  `read_size_with_PIL` failure message (BASE:455-484), queued as
  `reporter.warning('\n  '.join(['Cannot scale image!', f'Could not get size from "{uri}":', *problems]), base_node=node)`.
  The possible problems are:
  * `Requires Python Imaging Library.` (Pillow is **not installed** in the oracle
    environment; this output is environment-dependent);
  * `PIL cannot read video images.`;
  * `Reading external files disabled.`;
  * the `str()` of an OSError.

  It only runs when Sphinx could not fill the sizes, i.e. for a missing or unreadable
  file.
* **figure** (SX:764-768 sets `align='default'` if missing, then H5:204-216):
  * **V**: `atts['style'] = f"width: {node['width']}"` (no semicolon; this is the
    `figwidth`), `atts['class'] = f"align-{align}"` (the CLASS string, so it comes
    **after** node classes), then `ST(node,'figure',**atts)`.
  * **D**: `'</figcaption>\n'` if `len(node) > 1`, then `'</figure>\n'`.
* **caption** inside a figure (H5:154-161; SX wraps it at SX:632-663):
  * **V**: `ST(node,'figcaption')`, i.e. `<figcaption>\n` (caption ids/classes), then
    `'<p>'`. Sphinx then adds the fignumber and
    `ST(node,'span','',CLASS='caption-text')`.
  * **D**: Sphinx `'</span>'` + permalink, then H5 `'</p>\n'`. The `<figcaption>` stays
    open until `depart_figure`.
  * A caption whose parent is not a figure (and not Sphinx's code-block container)
    gets only `'<p>'`.
* **legend** (H5:280-287): `'<figcaption>\n'` if the previous sibling is not a
  `caption`; then `ST(node,'div',CLASS='legend')`. **D**: `'</div>\n'`.

Verified (probes `src/images.rst` and `src/img2.rst`; `pic.png` is 40×20 px):

```html
<img alt="_images/pic.png" src="_images/pic.png" />
<a class="reference internal image-reference" href="_images/pic.png"><img alt="Alt text" class="align-center" src="_images/pic.png" style="width: 100px; height: 50px;" />
</a>
<a class="reference internal image-reference" href="_images/pic.png"><img alt="_images/pic.png" height="10" src="_images/pic.png" width="20" />
</a>                                                                         ← :scale: 50%
<a class="reference internal image-reference" href="_images/pic.png"><img alt="_images/pic.png" height="40" src="_images/pic.png" style="width: 20em;" />
</a>                                                                         ← :width: 10em :scale: 200
<a class="reference external image-reference" href="https://example.com"><img alt="_images/pic.png" src="_images/pic.png" />
</a>                                                                         ← :target:
<img alt="_images/pic.png" class="myimg" id="img-name" src="_images/pic.png" />
<figure class="align-right" id="id1" style="width: 50%">
<a class="reference internal image-reference" href="_images/pic.png"><img alt="_images/pic.png" src="_images/pic.png" style="width: 200px;" />
</a>
<figcaption>
<p><span class="caption-text">Figure caption.</span><a class="headerlink" href="#id1" title="Link to this image">¶</a></p>
<div class="legend">
<p>Legend text.</p>
</div>
</figcaption>
</figure>
<figure class="align-default">
<img alt="_images/pic.png" src="_images/pic.png" />
</figure>                                                                    ← no caption: no figcaption, no id
<figure class="figcls align-center" id="fig-named">
<img alt="_images/pic.png" class="imgcls" src="_images/pic.png" />
<p>Inline <img alt="img" src="_images/pic.png" /> here.</p>              ← substitution image: alt = substitution name, no \n
<p>Inline <img alt="top" class="align-top" src="_images/pic.png" /> image.</p>
<img alt="_images/pic.png" loading="lazy" src="_images/pic.png" />
<video controls="controls" src="clip.mp4" title="A video">
<a href="clip.mp4">A video</a>
</video>
<a class="reference internal image-reference" href="https://example.com/remote.png"><img alt="https://example.com/remote.png" src="https://example.com/remote.png" style="width: 33.333%;" />
</a>
<a …><img alt="_images/pic.png" height="1" src="_images/pic.png" width="2" />      ← :width: 7 :height: 3.5 :scale: 33 → 2.31→2, 1.155→1
<a …><img alt="_images/pic.png" src="_images/pic.png" style="width: 0.5in; height: 1.23457em;" />   ← %g, 6 significant digits
```

Writer-time message placement (verified with **keep_warnings=False**, i.e. these are
rendered regardless of `keep_warnings`):

```html
<img alt="_images/pic.png" src="_images/pic.png" />
<aside class="system-message">
<p class="system-message-title">System Message: ERROR/3 (<span class="docutils literal">/abs/path/src/img2.rst</span>, line 7)</p>
<p>Cannot embed image &quot;_images/pic.png&quot;:
  [Errno 2] No such file or directory: '_images/pic.png'</p>
</aside>
<a class="reference internal image-reference" href="missing2.png"><img alt="missing2.png" src="missing2.png" />
<aside class="system-message">
<p class="system-message-title">System Message: WARNING/2 (<span class="docutils literal">/abs/path/src/img2.rst</span>, line 10)</p>
<p>Cannot scale image!
  Could not get size from &quot;missing2.png&quot;:
  Requires Python Imaging Library.</p>
</aside>
</a>
```

The message aside lands **inside** the scaled-image `<a>` because `report_messages`
runs in `visit_image`, before `depart_reference`. The same messages also reach the
warning stream through Sphinx's `LoggingReporter`, e.g.
`…/img2.rst:10: WARNING: Cannot scale image!\n  Could not get size from "missing2.png":\n  Requires Python Imaging Library. [docutils]`.

### 7.11 system_message and report_messages

* **system_message** (BASE:1637-1664):
  ```
  ST(node,'aside',CLASS='system-message')                      # '<aside class="system-message" id="id1">\n'
  '<p class="system-message-title">'
  backref_text = ''                                            # 1 backref:  '; <em><a href="#%s">backlink</a></em>'
                                                               # n backrefs: '; <em>backlinks: ' + ', '.join('<a href="#%s">%d</a>') + '</em>'
  line = ', line %s' % node['line'] if node.hasattr('line') else ''
  'System Message: %s/%s (<span class="docutils literal">%s</span>%s)%s</p>\n'
      % (node['type'], node['level'], encode(node['source']), line, backref_text)
  … children (paragraph(s), and a literal_block of the directive source for directive errors) …
  depart: '</aside>\n'
  ```
  `node['source']` is the **absolute source path**, so oracles must normalize the temp
  dir. `type` is one of `DEBUG`, `INFO`, `WARNING`, `ERROR`, `SEVERE`, and `level` is
  0-4. Under Sphinx, a literal_block inside a system message goes through Sphinx's
  highlighter (`rawsource == astext`). Verified (`keep_warnings=True`):
  ```html
  <p><a href="#id1"><span class="problematic" id="id2">:unknownrole:`oops`</span></a> here.</p>
  <aside class="system-message" id="id1">
  <p class="system-message-title">System Message: ERROR/3 (<span class="docutils literal">/abs/…/src2/index.rst</span>, line 6); <em><a href="#id2">backlink</a></em></p>
  <p>Unknown interpreted text role “unknownrole”.</p>
  </aside>
  <aside class="system-message">
  <p class="system-message-title">System Message: ERROR/3 (<span class="docutils literal">/abs/…/src2/other.rst</span>, line 4)</p>
  <p>Error in “image” directive:
  1 argument(s) required, 0 supplied.</p>
  <div class="highlight-default notranslate"><div class="highlight"><pre><span></span><span class="o">..</span> <span class="n">image</span><span class="p">::</span>
  </pre></div>
  </div>
  </aside>
  ```
  The curly quotes come from SmartQuotes on the read side: in-tree message paragraphs
  are smart-quoted, while writer-time messages built after the transforms keep
  `&quot;`.
* **report_messages(node)** (BASE:620-626): returns immediately if `node.parent` is a
  `system_message` or `entry`. Otherwise it pops `self.messages` FIFO and walks each
  message whose `level >= settings.report_level` (2) into the body. The **flush
  points** in the Sphinx-reachable code are:
  * `depart_paragraph` (only when the `\n` is added);
  * `depart_table`;
  * `visit_image` for block images.

  (`depart_math_block` is overridden by Sphinx and does not flush.) A queued message
  from an **inline** image therefore appears after the enclosing paragraph's `</p>\n`,
  or later.

### 7.12 comment, meta, math

* **comment**: the Sphinx override raises SkipNode, so nothing is emitted. The base
  (BASE:837-842) would emit `'<!-- %s -->\n' % re.sub('-(?=-)', '- ', astext())`.
* **meta** (H5:326-333): if the node has `lang`, `node['xml:lang'] = node['lang']`;
  then `self.meta.append(ET(node,'meta',**node.non_default_attributes()))`, with the
  attributes sorted. Output goes **to `self.meta`, not the body**, and so into
  `metatags` (1.1).
* **math / math_block**: fully overridden by Sphinx (SX:994-1022), which dispatches to
  the configured renderer, `mathjax` by default (`sphinx/ext/mathjax.py:36-78`). For
  reference, verified mathjax output:
  ```html
  <p>Math <span class="math notranslate nohighlight">\(a^2\)</span> inline.</p>
  <div class="math notranslate nohighlight">
  \[E = mc^2\]</div>
  <div class="math notranslate nohighlight" id="equation-eq1">
  <span class="eqno">(1)<a class="headerlink" href="#equation-eq1" title="Link to this equation">¶</a></span>\[x = 1\]</div>
  ```
  The docutils path (BASE:1322-1408; `math_output` `html`/`latex`/`mathjax`/`mathml`
  with the `math_tags` table `html=('span','div',['formula'])`,
  `latex=('tt','pre',['math'])`, `mathjax=('span','div',['math'])`,
  `mathml=('','div',[])`, `problematic=('span','pre',['math','problematic'])`, and
  `visit_math` raising SkipChildren) is **unreachable** under Sphinx. Do not port it.

### 7.13 Admonitions: base behavior, for reference only

The base (BASE:676-680) would emit `ST(node,'aside',classes=['admonition'])` …
`'</aside>\n'` for the generic `admonition` node. The specific ones (`note`, …) would
have been converted by `writer_aux.Admonitions`, which Sphinx does not run. **Sphinx
overrides all admonitions** (SX:375-398, 862-914) and renders
`<div class="{node classes} admonition {name}">\n` + the inserted title
`<p class="admonition-title">Note</p>\n` + body + `</div>\n`. Verified:

```html
<div class="admonition note">
<p class="admonition-title">Note</p>
<p>A note.</p>
</div>
<div class="admonition-custom-title admonition">          ← generic admonition, CLASS='admonition ' (trailing space vanishes)
<div class="myadm admonition">
<div class="extra admonition note">                         ← .. note:: :class: extra
<div class="admonition seealso">
<p class="admonition-title">See also</p>
```

### 7.14 Docinfo family: dead under Sphinx, specified for completeness

Sphinx's `MetadataCollector.process_doc`
(`sphinx/environment/collectors/metadata.py:34-68`) pops the `docinfo` node (the first
non-PreBibliographic child) after recording it in `env.metadata`. Verified: `:author:`
and `:date:` at the top leave no trace in the body. The `:abstract:` and
`:dedication:` fields become `topic` nodes, which *are* rendered (7.1).

For reference:

* `visit_docinfo` pushes `len(body)` and emits `ST(node,'dl',classes=['docinfo'(,'simple')])`.
* `depart_docinfo` emits `'</dl>\n'`, **moves** `body[start:]` to `self.docinfo`, and
  **resets `self.body = []`**. That would drop earlier body content from the fragment.
* `visit_docinfo_item(node, name, meta=True)` emits
  `'<dt class="{name}">{language.labels[name]}<span class="colon">:</span></dt>\n'` +
  `ST(node,'dd','',CLASS=name)`, and with meta it adds a `<meta name=… content=…/>`.
  **D**: `'</dd>\n'`.
* author gets `<p>`…`</p>`; address gets `<pre class="address">`…`\n</pre>\n`.
* H5 routes copyright/date/authors/organization metas to `dcterms.*` and `author`
  metas.

---

## 8. Crate-side mapping notes

* **Node kinds.** `src/doctree/kinds.rs` has consts for only part of the kinds the
  writer must handle. The parser builds the rest from **string literals**: `"topic"`,
  `"sidebar"`, `"rubric"`, `"admonition"`/`"note"`/`"seealso"`/…, `"figure"`,
  `"caption"`, `"legend"`, `"container"`, `"raw"`, `"inline"`, `"generated"`,
  `"math_block"`, `"meta"`, `"decoration"`, `"header"` all occur under `src/`. No
  `"footer"` or `"docinfo"` producer exists yet. `Node.kind` is `&'static str`, and
  deserialized kinds go through the runtime interner (`src/doctree/intern.rs:40`).
  Writer dispatch should match on `node.kind`, with the Sphinx MRO fallbacks from 1.2
  (e.g. `desc_sig_*` → inline).
* **Category sets** for the `isinstance` tests must be the exact lists in 1.3.
* **Attributes** the base writer reads, with the crate spellings found in
  `src/rst/block.rs`:
  * `enumtype`, `start` (Int, set only when ≠ 1), `prefix`/`suffix` (unused by HTML)
    at :1669-1674;
  * `colwidth` (Int) at :2820, :3196, :6277, :6693;
  * `stub` (Int 1) at :6695;
  * `morecols`/`morerows` (Int) at :2770-2773 and :3144;
  * `delimiter` (Str) at :2505;
  * `format` (Str) at :7537;
  * the `colwidths-auto`/`colwidths-given` classes at :6244-6282 and :6682-6685;
  * `backrefs` and `ids` as typed fields in `Attrs` (`src/doctree/mod.rs:80-92`);
  * `refid`/`refuri`/`refname`, `loading`, `scale` (a percentage int), `width`/`height`
    (strings), `align`, `alt`.

  The writer must treat a `Str`/`Int` `width` the way docutils does: `parse_measure`
  of the **string form**.
* **`Node::astext()`** (`src/doctree/mod.rs:197-202`) joins children with `""`. That
  is correct for every place the writer calls `astext`: `raw`, `generated`, the Sphinx
  `literal`/`math`, `literal_block` highlighting, `meta` content. Text nodes already
  hold unescaped text (`src/rst/inline.rs:6-12`), so there is no `\x00` stripping in
  the writer.
* **`rawsource` is not stored** (no occurrence under `src/`). Sphinx's parsed-literal
  test needs it (7.3).
* **`keep_warnings`/`FilterSystemMessages` and SmartQuotes are not implemented**
  (`tests/env_differential.rs:883-903`). Both change HTML bytes:
  * with default `keep_warnings=False`, every reporter `system_message` must vanish
    from the page, while `problematic` nodes and their dangling `href="#idN"` remain;
  * without SmartQuotes, every `"`/`'`/`--`/`...` in prose renders differently. The
    verified bodies show `“quotes”`, `‘single’`, `—` and `…`.

  These are resolve/read-side prerequisites for byte parity of the body.
* **Placeholder to replace:** `src/builder.rs:874-879` (and
  `src/html_builder.rs:369-373`), which emits `<html><body>{escaped source}</body></html>`
  through the `html_escape` crate.

---

## 9. Python-semantics pitfalls for the Rust port

1. **`round()`** in `image_size` is round-half-to-even, returning an int (`round(2.5)
   == 2`, `round(3.5) == 4`). Use `f64::round_ties_even` (stable since Rust 1.77) and
   print it as an integer.
2. **`f'{value:g}{unit}'`**: Python general format with precision 6.
   * Trailing zeros and a trailing `.` are stripped: `20.0` → `20`, `100.0` → `100`,
     `1.2345678` → `1.23457`.
   * Exponent form is used when exp < -4 or exp ≥ 6: `1234567.0` → `1.23457e+06`,
     `0.00001` → `1e-05`. The exponent has at least 2 digits and a sign.

   `value` is always a float here, because it is multiplied by `scale/100` (a float,
   even when scale is 100). Implement a `%g` formatter; Rust's `{}` does not match it.
3. **`f'{x:.1%}'`** for colgroup widths: Python computes `x * 100` in f64 and formats
   with `'.1f'` (correctly rounded, ties resolved on the exact binary value), then
   appends `%`. Rust's `format!("{:.1}%", x * 100.0)` is also correctly rounded on the
   exact binary value, so it matches. **Compute exactly
   `propwidth / total_width` then `* 100.0`, in that order.** `total_width` is the
   Python `sum()` of ints or floats.
4. **`parse_measure`** accepts `int(...)` first, then `float(...)`, with the regex
   `(-?[0-9.]+) *([a-zA-Zµ]*|%?)` fullmatch. `'7'` gives the int 7, and
   `7 * 0.33 = 2.31`.
5. **Sorted attributes**: Python sorts by Unicode code point; all keys here are ASCII.
6. **`str()` of attribute values**: ints print as decimal. The bool path is never
   reached by base methods, and Sphinx's `download=''` prints `download=""`.
7. **`mimetypes.guess_type`** depends on the platform's mime tables. The values below
   were verified in the oracle's Python 3.12:
   * `.mp4` → `video/mp4`, `.webm` → `video/webm`, `.ogv` → `video/ogg`: these are the
     only `<video>` triggers;
   * `.ogg` → `audio/ogg`, NOT a video;
   * `.swf` → `application/vnd.adobe.flash.movie`, **not**
     `application/x-shockwave-flash`, so the docutils `<object>` flash branch
     (BASE:1206-1209) is **unreachable** on 3.12;
   * `.svg`/`.svgz` → `image/svg+xml` (only matters for `:loading: embed`).

   Hard-code this table.
8. **Environment dependence**: the PIL problem text (`Requires Python Imaging
   Library.`) appears only because the oracle environment lacks Pillow. The
   `Cannot embed image` text embeds CPython's `OSError.__str__`
   (`[Errno 2] No such file or directory: '_images/pic.png'`), and the crate already
   has errno-text machinery for include errors. Oracles should pin the Sphinx
   environment exactly (the `uv run --with sphinx==9.1.0 --with docutils==0.22.4`
   command, which has no Pillow).
9. **Sphinx `visit_Text` for literal text** (part 2, but it uses the base regex
   `words_and_spaces = re.compile(r'[^ \n]+| +|\n')`, BASE:276). Verified:
   ``literal text  two spaces`` renders as
   `<span class="pre">literal</span> <span class="pre">text</span>&#160; <span class="pre">two</span> <span class="pre">spaces</span>`.
   A run of n > 1 spaces becomes `'&#160;' * (n-1) + ' '`.

---

## 10. Quick reference: base-method output table

`ST` includes ids and classes by 3.3, and `sfx` is shown when it is not `'\n'`.

| node | start | end |
|---|---|---|
| section | `<section{atts}>\n` | `</section>\n` |
| title (section, depth d) | `<h{min(d,6)}{ aria-level if d>6}>` (+ toc-backref `<a class="toc-backref" href="#R" role="doc-backlink">`) | (SX permalink) `</h{n}>\n` or `</a></h{n}>\n` |
| title (topic/sidebar/admonition) | `<p class="topic-title">` / `sidebar-title` / `admonition-title` | `</p>\n` |
| title (table) | `<caption>` | `</caption>\n` |
| subtitle (sidebar) | `<p class="sidebar-subtitle">` | `</p>\n` |
| paragraph | `<p>` | `</p>` + `\n` unless the sole child of list_item/entry |
| bullet_list | `<ul[ class="… simple"]>\n` | `</ul>\n` |
| enumerated_list | `<ol class="{enumtype}[ simple]"[ start="N"]>\n` | `</ol>\n` |
| list_item | `<li>` | `</li>\n` |
| definition_list | `<dl[ class="simple …"]>\n` | `</dl>\n` |
| field_list | `<dl class="… field-list[ simple]"[ style="--field-indent: X;"]>\n` | `</dl>\n` |
| field_name | `<dt class="field-odd">` | `<span class="colon">:</span></dt>\n` |
| field_body | `<dd class="field-odd">` (+`<p></p>` if empty) | `</dd>\n` |
| option_list | `<dl class="option-list">\n` | `</dl>\n` |
| option_group | `<dt><kbd>` | `</kbd></dt>\n` |
| option | `<span class="option">` | `</span>` (+`, ` before a sibling option) |
| option_argument | `{delimiter}<var>` | `</var>` |
| description | `<dd>` | `</dd>\n` |
| attribution | `<p class="attribution">—` | `</p>\n` |
| line_block | `<div class="line-block">\n` | `</div>\n` |
| line | `<div class="line">` (+`<br />` if empty) | `</div>\n` |
| literal_block (parsed) | `<pre class="literal-block">` | `</pre>\n` |
| compound | `<div class="… compound">\n` | `</div>\n` |
| container | `<div class="… docutils container">\n` (or `ins`/`del`) | `</div>\n` |
| topic | `<aside class="topic …">\n` / `<nav class="contents …"[ role="doc-toc"]>\n` / `<div class="topic abstract" role="doc-abstract">\n` | matching close + `\n` |
| sidebar | `<aside class="sidebar">\n` | `</aside>\n` |
| rubric | `<p class="… rubric">` | `</p>\n` |
| transition | `<hr class="docutils" />\n` | — |
| footnote group | `<aside class="footnote-list brackets">\n` | `</aside>\n` |
| footnote | `<aside class="footnote brackets" id="…" role="doc-footnote">\n` | `</aside>\n` |
| citation group | `<div role="list" class="citation-list">\n` | `</div>\n` |
| citation | `<div class="citation" id="…" role="doc-biblioentry">\n` | `</div>\n` |
| label | `<span class="label"><span class="fn-bracket">[</span>[<a role="doc-backlink" href="#B">]` | `[</a>]<span class="fn-bracket">]</span></span>\n[<span class="backrefs">(…)</span>\n]` |
| table (depart) | — | `</table>\n` + flush messages |
| colgroup | `<colgroup>\n<col style="width: P%" />\n…</colgroup>\n` (colwidths-given only) | — |
| thead / tbody | `<thead>\n` / `<tbody>\n` | `</thead>\n` / `</tbody>\n` |
| row (depart) | — | `</tr>\n` |
| entry | `<th class="head[ stub]"…>` / `<td[ colspan][ rowspan]>` | `</th>\n` / `</td>\n` |
| image (block) | `<img alt="…"[ class][ height][ id][ loading] src="…"[ style][ width] />\n` | — |
| image (inline) | same without `\n` | — |
| figure | `<figure class="… align-X"[ id][ style="width: W"]>\n` | `[</figcaption>\n]</figure>\n` |
| caption (figure) | `<figcaption>\n<p>` | `</p>\n` |
| legend | `[<figcaption>\n]<div class="legend">\n` | `</div>\n` |
| raw (html) | content verbatim (wrapped in `<span|div class>` if classes) | — |
| emphasis/strong/sub/sup/cite | `<em>` `<strong>` `<sub>` `<sup>` `<cite>` | closing tag |
| inline | `<span class="…">` | `</span>` |
| acronym | `<abbr>` | `</abbr>` |
| target (inline) | `<span class="target" id="…">` | `</span>` |
| problematic | `<a href="#R"><span class="problematic" id="…">` | `</span></a>` |
| generated/sectnum | `<span class="sectnum">N </span>` | — |
| system_message | `<aside class="system-message"[ id]>\n<p class="system-message-title">System Message: T/L (<span class="docutils literal">SRC</span>, line N)[; backlink]</p>\n` | `</aside>\n` |
| reference (depart) | — | `</a>` (+`\n` if the parent is not TextElement) |
| footnote_reference (depart) | — | `<span class="fn-bracket">]</span></a>` |
| comment, substitution_definition, decoration, header, footer, docinfo(Sphinx) | nothing in the fragment | — |
