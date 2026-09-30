# M2 wave 5 research — the doctree IR and the node-kind census a writer must render

Research key: `doctree`. Scope: `src/doctree/*` (read in full), every node constructor in
`src/rst/block.rs`, `src/rst/inline.rs`, `src/py/*`, `src/env/*`, what survives the resolve
pass, and what Sphinx 9.1.0 / docutils 0.22.4 hand their HTML writer that this crate does
not produce. All `SP/` paths are under
`/root/.cache/uv/archive-v0/b4dBDAdEzskuqge1iT52j/lib/python3.12/site-packages/sphinx`,
`DU/` under `.../site-packages/docutils`. Crate paths are relative to `/home/user/sphinx-ultra`.

Probe artifacts (real Sphinx 9.1.0 html builds, `keep_warnings=True`, default smartquotes)
live in `/tmp/claude-0/-home-user-sphinx-ultra/46bf5e6b-694f-5b8e-ba0d-36f1851a8974/scratchpad/probe-doctree/`:
`src/index.rst` + `index.writer.pformat` (the exact doctree `StandaloneHTMLBuilder.write_doc`
receives), `probe2.py`/`src2` (read-phase vs writer-time for meta/contents/sectnum/index
role/collapsible/heading-level/highlight), `probe3.py`/`src3` (index `:name:`, `:index:`
role, versionadded arg+content), `probe4.py`/`src4` (toctree caption/class/name/explicit
title read-phase attrs). `transforms.py` prints the full transform registry. Run any of
them with `PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' --with
'docutils==0.22.4' python <probe>.py` from that directory.

---

## 0. TL;DR for implementers

1. **One generic node type.** `Node { kind: &'static str, span, text: Option<String>, attrs, children }`
   (`src/doctree/mod.rs:96-104`). `kind` is the docutils/Sphinx tagname string; there are no
   per-kind Rust types, no parent pointers, no `rawsource`, no `document` back-reference.
   Text leaves are `kind == "#text"` with `text: Some(..)`.
2. **The trees are docutils *parse-layer* output plus a handful of inlined Sphinx read
   transforms.** Almost every docutils/Sphinx *transform* that reshapes the tree before the
   HTML writer sees it is **not run**: Substitutions, PropagateTargets, Anonymous/Indirect/
   External/InternalTargets, Footnotes, citation transforms, DocInfo removal, AutoNumbering
   (except literalinclude), DoctestTransform, SphinxSmartQuotes, Transitions checks,
   FilterSystemMessages, ReorderConsecutiveTargetAndIndexNodes, OnlyNodeTransform,
   HighlightLanguageTransform, ImageCollector (`candidates`/uri rewrite), toctree resolution.
   The writer (or a new transform layer before it) must supply every one of them to reach
   byte parity. §8 is the complete map.
3. **The resolve pass exists but its output is thrown away.** `SphinxBuilder::xref_phase`
   (`src/builder.rs:1294-1359`) resolves a *clone* of each doctree with a stub
   `relative_uri = |_,_| String::new()` (`:1309`) and keeps only `doctree.root.pformat()`
   (`:1344-1347`) for the env oracle. The writer must re-resolve with real HTML URIs (or the
   phase must be restructured) — and must not double-emit the resolution warnings.
4. **`pending_xref` for many Sphinx roles is wrong at parse time.** In sphinx mode every role
   that is not a docutils built-in becomes a `pending_xref` (`src/rst/inline.rs:1461-1468`):
   `:kbd:`, `:guilabel:`, `:menuselection:`, `:file:`, `:samp:`, `:abbr:`, `:command:`,
   `:dfn:`, `:program:`, `:regexp:`, `:mimetype:`, `:mailheader:`, `:makevar:`, `:newsgroup:`,
   `:manpage:`, `:download:` all come out as `pending_xref refdomain="std" reftype="kbd"…`,
   and after resolve fall back to `<literal classes="xref std std-kbd">`. Sphinx produces
   specific nodes (§9.2). `:index:` produces an ERROR `problematic` in sphinx mode
   (`inline.rs:1445` + `:1579-1584`). `:eq:` is never resolved (math domain missing).
5. **Sentinel encodings the writer must decode:** Python `None` is stored as the string
   `"True"` (pformat's rendering) on `toctree[caption]`, `math_block[label|number]`,
   `pending_xref[py:module|py:class|std:program]`, `desc_signature[module]`
   (`src/env/std_domain.rs:320-334` `is_none_sentinel`); Python bools are `Int(0|1)`;
   several attributes are **Python-repr strings** the writer must parse:
   `literal_block[highlight_args]` (`"{'hl_lines': [1, 3], 'linenostart': 5}"`),
   `desc_signature[_toc_parts]` (`"('mod', 'func')"`), `index[entries]` and
   `toctree[entries]` (lists of tuple reprs).
6. **system_message nodes** are fully shaped in-tree (`level`, `line`, `source`, `type`,
   `ids`, `backrefs`, paragraph + optional literal_block children) but carry `Span::ZERO`;
   their location lives only in attributes. None are printed today (the reporter channel).
   Default Sphinx builds (`keep_warnings=False`) strip *all* of them from the tree before
   writing (`SP/transforms/__init__.py:337-347`, filterlevel 5).

---

## 1. The IR (`src/doctree/`)

### 1.1 Files

| File | Lines | Content |
|---|---|---|
| `src/doctree/mod.rs` | 358 | `Span`, `AttrValue`, `Attrs`, `Node`, `Doctree`, bincode entry points |
| `src/doctree/kinds.rs` | 84 | tagname consts (a *subset* — many kinds are string literals elsewhere, §1.8) |
| `src/doctree/intern.rs` | 157 | `&'static str` interner for deserialization (`MAX_INTERNED = 4096`) |
| `src/doctree/ids.rs` | 715 | docutils `make_id`, sphinx `_make_id`, name normalizers, `IdRegistry` (parse-time only) |
| `src/doctree/messages.rs` | 77 | `system_message` builders |
| `src/doctree/pformat.rs` | 289 | byte-exact `document.pformat()` |

### 1.2 `Span` (`mod.rs:41-56`)

```rust
pub struct Span { pub source: u16, pub line: u32, pub start: u32, pub end: u32 }
```
- `source` indexes `Doctree::sources` (0 = the document; included files and detached
  sub-parses — csv cells, literalinclude captions — push entries; `block.rs:541-619`).
- `line` is 1-based, **0 = unknown/unstamped**. Convention: nodes stamped with the first line
  of their span, except `section`, stamped one past (the underline / title line,
  `block.rs:868-874`).
- `start..end` = byte range in the parser's *processed* text (tab-expanded, rstripped).
- `Span::ZERO` is used by: every `system_message` and its children (`messages.rs:24,29,38,47`),
  all `py/*` signature nodes (`annotations.rs:444-450`, `arglist.rs:557-617`) except
  `pending_xref` from `type_to_xref` (uses `ctx.span`, `annotations.rs:138`), doc-field
  transformer output (line forced to 0, `block.rs:9651-9664`), toc nodes built by
  `env/toctree.rs` (`:450-458`, `:585-596`), `title` from `document_title` (`:945`).
- `Doctree::source_and_line(span)` (`mod.rs:234-242`) → `(path, line)`, unknown source →
  entry 0.
- The HTML writer does not need spans for output; the reporter channel and any
  write-time warnings (e.g. resolution, image-not-readable) do. Unstamped nodes must inherit
  the nearest stamped ancestor (docutils `get_source_line`); `resolve.rs:803-825`
  (`contributes_location`) already encodes which kinds *never* count (`document`, `desc`,
  `desc_content`).

### 1.3 `AttrValue` and `Attrs` (`mod.rs:68-92`)

```rust
pub enum AttrValue { Int(i64), Str(String), List(Vec<String>) }
pub struct Attrs {
    pub ids: Vec<String>, pub names: Vec<String>, pub dupnames: Vec<String>,
    pub classes: Vec<String>, pub backrefs: Vec<String>,
    pub extra: Vec<(&'static str, AttrValue)>,   // kept SORTED by key
}
```
- The five docutils universal list attributes are typed fields. Everything else is in
  `extra`, sorted by key (invariant maintained by `Node::set`, `mod.rs:176-181`).
- `Int` carries Python ints **and bools** (`True`→`1`). `Str` carries strings **and the None
  sentinel `"True"`** (§0 item 5). `List` is for element-specific list attributes, which
  pformat serial-escapes and joins: `toctree[entries|includefiles]`, `index[entries]`,
  `desc_signature[allnames]`. Empty `List` still prints `attr=""`; empty universal lists are
  suppressed (`pformat.rs:19-28`, `:60-70`).
- Keys are `&'static str`, so new attribute names must be literals (or go through
  `intern`, which is `pub(crate)`, `mod.rs:21`, `intern.rs:66`).

### 1.4 `Node` (`mod.rs:96-208`)

```rust
pub struct Node { pub kind: &'static str, pub span: Span, pub text: Option<String>,
                  pub attrs: Attrs, pub children: Vec<Node> }
```
API (complete):
- `Node::elem(kind, span)` — empty element (`:141`).
- `Node::text_node(s, span)` — `#text` leaf (`:151`).
- `shallow_copy()` — kind/span/text/attrs, **no children** (docutils `Element.copy()`, `:164`).
- `set(key, value)` — insert/overwrite in sorted `extra` (`:176`).
- `get(key) -> Option<&AttrValue>` — binary search (`:183`).
- `astext()` — concatenation of all text descendants **joined with `""`** (`:197-202`).
  docutils joins non-`TextElement` children with `"\n\n"` (`Element.child_text_separator`);
  this matters for `system_message` (a separate helper exists:
  `block.rs:11423 system_message_astext`), field bodies with several paragraphs
  (metadata), `image.astext()` (docutils returns `alt`), and `Text.astext()` (null-escape
  removal — the crate already stores unescaped text). The writer should add a faithful
  `astext` (per-kind separator) rather than reuse this one.
- `pformat()` — §1.6.
- **Missing** (writer will want them): attribute removal (today: `attrs.extra.retain(..)`
  by hand), `findall`/pre-order iterator, parent/sibling access, `replace_self`,
  `hasattr` for universal lists (`!attrs.ids.is_empty()` etc.).

Serde: `Node` has a hand-written `Deserialize` through a shadow struct that interns `kind`
(`mod.rs:106-138`); `Attrs::extra` serializes as a map with interned keys
(`intern.rs:94-137`).

### 1.5 `Doctree` (`mod.rs:210-247`) and persistence

```rust
pub struct Doctree { pub root: Node /* kind "document" */, pub sources: Vec<String> }
```
- `to_bincode`/`from_bincode` (`mod.rs:249-266`), bincode `config::standard()`.
- On disk: `<cache>/doctrees/<blake3(docname)>.doctree`, 8-byte header `b"SUDT"` +
  `DOCTREE_FORMAT_VERSION` (currently **2**, `src/builder.rs:36-67`, `:1482-1533`). The doc
  comment at `builder.rs:43-60` says: bump it whenever the serialized shape **or the meaning
  of what the parser stores** changes. Any read-phase transform wave 5 adds (footnote
  numbering, PropagateTargets, AutoNumbering, docinfo removal, smartquotes…) changes stored
  meaning → bump.
- `RegistryExport` (`src/rst/mod.rs:252-318`) is the only parse-time state that survives:
  `nameids`, `index_serial`, domain registration records, `log_warnings`, `dependencies`,
  `included`. **The docutils `document.ids` set and the auto-id counter (`IdRegistry.ids`,
  `id_counter`, `ids.rs:206-223`) are not exported.** Every docutils/Sphinx transform that
  allocates new auto ids after the parse (AutoNumbering → `id12`, Footnotes/Dangling
  references → `problematic`/`system_message` ids, `set_id` for new targets) needs that
  counter to reproduce `idN` numbering byte-exactly. Either run those transforms inside the
  parse (while `IdRegistry` is alive, the approach `literalinclude_container` took at
  `block.rs:4273-4283`) or export `ids` + counter.

### 1.6 `pformat` rules (`pformat.rs`)

- 4-space indent per depth, `<kind attrs>`, no closing tags, every line ends `\n`.
- Universal lists (backrefs/classes/dupnames/ids/names, empty suppressed) and `extra`
  merge into ONE alphabetical sequence (`:50-74`).
- **No XML escaping anywhere**; list items `serial_escape`d (`\`→`\\`, space→`\ `).
- `#text`: one output line per Python `splitlines()` line; empty text prints nothing
  (`:32-47`, uses `crate::utils::py_splitlines`).
- This is the oracle format; HTML output needs its own escaping (docutils `encode`).

### 1.7 `ids.rs` — parse-time id machinery

- `make_id` (docutils, `:70`), `sphinx_make_id` (sphinx fork: keeps case, `.`/`_`, `:122`),
  `fully_normalize_name` (lower + ws collapse, `:177`), `whitespace_normalize_name` (`:182`).
- `IdRegistry` (`:207-500`): `set_id_implicit` (sections, phrase-ref targets), `set_id_explicit`
  (targets, `:name:`, footnotes/citations), `set_id_anonymous`, `allocate_auto_id`
  (problematic/system_message pairs, footnote/citation refs), `sphinx_make_id(prefix, term)`
  with per-prefix serials, `note_explicit_id`, `new_index_serialno` (`index-N`).
  Duplicate handling produces the INFO/WARNING `system_message`s and deferred dupname fixups
  (`apply_dupname_fixups`, `:504-524`). The registry dies with the parser except for
  `nameids_snapshot()` and `index_serial()`.

### 1.8 `kinds.rs` coverage

Consts exist for: `#text`, document, section, title, paragraph, transition, bullet_list,
enumerated_list, list_item, definition_list(+_item), term, classifier, definition,
block_quote, attribution, literal_block, doctest_block, line_block, line, comment, target,
system_message, emphasis, strong, literal, problematic, reference, title_reference,
footnote_reference, citation_reference, substitution_reference, subscript, superscript,
abbreviation, acronym, math, footnote, citation, label, field_list/field/field_name/field_body,
option_list/_item/option_group/option/option_string/option_argument, description, subtitle,
image, compact_paragraph, compound, only, pending_xref, toctree, table, tgroup, colspec,
thead, tbody, row, entry.

Emitted **only as string literals** (no const): `admonition`, `note`, `warning`, `tip`,
`hint`, `important`, `caution`, `danger`, `error`, `attention`, `topic`, `sidebar`, `rubric`,
`container`, `caption`, `legend`, `figure`, `raw`, `pending`, `substitution_definition`,
`math_block`, `inline`, `index`, `glossary`, `hlist`, `hlistcol`, `highlightlang`, `seealso`,
`versionmodified`, `desc`, `desc_signature`, `desc_name`, `desc_addname`,
`desc_annotation`, `desc_content`, `desc_parameterlist`, `desc_parameter`, `desc_optional`,
`desc_returns`, `desc_type_parameter_list`, `desc_type_parameter`, `desc_sig_space`,
`desc_sig_name`, `desc_sig_operator`, `desc_sig_punctuation`, `desc_sig_keyword`,
`desc_sig_literal_number`, `desc_sig_literal_string`, `pending_xref_condition`,
`literal_strong`, `literal_emphasis`, `number_reference`. Recommend adding consts for all of
them before the writer's big `match node.kind` (typos compile silently today).

---

## 2. Traversal and mutation patterns in the codebase

There is no visitor abstraction. Every consumer writes a recursive function over
`&Node`/`&mut Node`:
- read-only pre-order: `std_domain.rs:633-645 flatten` builds a `Vec<FlatNode{node, parent_kind,
  subtree_end}>` — the closest thing to docutils `findall`/`next_node(ascend, descend)`.
- mutation by rebuilding a child list: `resolve.rs:850-887 resolve_children` (recurse first,
  then `std::mem::take(&mut node.children)` and push replacements — a `pending_xref` expands
  to 0..n nodes). This is the idiom for `replace_self`.
- node identity for side tables: `std_domain.rs:621-623` uses the node's address while a
  borrow is held (`PropagatedIds`).
- ancestor-dependent logic threads state down the recursion (`resolve.rs:857` passes the
  inherited `Location`).

For the writer: implement a `walkabout`-style traversal with an explicit ancestor stack
(`&[&Node]`) and child index, because `HTML5Translator` consults `node.parent` constantly
(`visit_title` section depth/topic/sidebar/table/admonition, `visit_caption` parent
container `literal_block` flag, `visit_paragraph` compactness via siblings,
`visit_reference` image child, `visit_term`, `visit_desc_signature`…).

Cross-subtree mutations needed before writing (PropagateTargets moves ids from a target to
the *next node in document order*, possibly a cousin; Footnotes numbering pairs references
and footnotes anywhere) are best done in two passes: compute over a pre-order index list,
then apply with a mutable pre-order walk that counts indices. `std_domain.rs:541-693`
already computes the PropagateTargets donations (read-only) and can be reused.

---

## 3. Pipeline: where trees come from and what shape reaches the writer today

1. **Read** (`builder.rs:767-915`): `Parser::parse_full` → `rst::parse_rst_full` with
   `sphinx: true` (`src/parser.rs:136-145`) → `BlockParser::parse_document_full`
   (`block.rs:398-424`). Only "transforms" run at parse time: dupname fixups
   (`block.rs:788-799`), `ClassAttribute` effect inline in sphinx mode (`:952-968`,
   `:7623-7627`), `GlossarySorter` (`:4706-4723`), `DocFieldTransformer`
   (`:9644-9971`), `filter_meta_fields` (py), literalinclude's AutoNumbering id
   (`:4263-4283`), `PropagateDescDomain` (at resolve, below).
2. **Merge** into `BuildEnvironment` (tocs via `env/toctree.rs:424 build_toc`, titles via
   `:944 document_title`, metadata via `env/metadata.rs:65`, std/py registries, index
   entries). Doctree persisted.
3. **Resolve** (`builder.rs:1193-1246`): numbering → consistency → `xref_phase` (clone +
   `env/resolve.rs:777 resolve_document` = `ReferencesResolver` for std/py/any/intersphinx +
   `propagate_desc_domain`) → genindex → py-modindex. Result pformat stored in
   `self.resolved`, the tree dropped.
4. **Write** (`builder.rs:1459-1479`): writes `Document.html` (the placeholder).

So the "resolved doctree" = parse output + pending_xref replacement + desc_signature domain
class. Nothing else.

---

## 4. Census of emitted tagnames

Legend for **Resolve**: `=` unchanged by `resolve_document`; `R` replaced/rewritten there.
**Sphinx@write** = what the HTML writer receives in real Sphinx (probe-verified unless noted),
i.e. what the crate's tree must become before/while rendering. **HTML** = the
`HTML5Translator` dispatch (MRO-resolved; `@html5` = `SP/writers/html5.py`, `@base` =
`DU/writers/_html_base.py`, `@poly` = `DU/writers/html5_polyglot/__init__.py`).

### 4.1 Structural / body elements (docutils)

| kind | producers (file:line) | attrs the crate sets | Resolve | Sphinx@write / gaps | HTML |
|---|---|---|---|---|---|
| `document` | `block.rs:749-758` | `source` (path string as passed in ParseOptions) | = | + `translation_progress` (i18n; oracle strips it); `title` attr from `.. title::` (not supported) | `document@base` (Sphinx uses body parts only) |
| `section` | `block.rs:874-893` (`open_section`) | `ids` (make_id of title text, `idN` fallback), `names`/`dupnames` (fully normalized title) | = | + ids donated by preceding targets (PropagateTargets) and `SortIds` (`id\d+` moved last, sections only) | `section@poly` (`<section id=…>`; extra ids as `<span id>`) |
| `title` | `block.rs:865` (section), `:6165` (table), `:6784` (topic/sidebar), `:7090` (admonition); `env/toctree.rs:945` | none | = | section titles get `auto="1"`/`refid` only with sectnum/contents (unsupported); toctree caption becomes a `title` inside `compact_paragraph toctree` | `title@html5` (headerlink `¶`) |
| `subtitle` | `block.rs:6790` (sidebar `:subtitle:`) | none | = | same | `subtitle@base` |
| `paragraph` | `block.rs:1364`; `:5779` (versionmodified, `translatable=0`); `:7821` (problematic-subst msg); doc fields `:9790,:9864,:9915,:9960`; `messages.rs:29,47` | `translatable` Int 0 only on versionmodified | = | smartquotes rewrite text (`"` → `“”`, `--`→`–`, `...`→`…`) | `paragraph@base` (compact rules) |
| `transition` | `block.rs:1101-1104` | none | = | docutils `Transitions` (830) may add ERROR system_messages / hoist | `transition@base` (`<hr class="docutils" />`) |
| `bullet_list` | `block.rs:1498` (`bullet` Str `*`,`-`,`+`…), `:4501` (hlist column, no attr), `:9902,:9934` (doc fields, no attr); `env/toctree.rs:427,488,552` | `bullet` | = | same | `bullet_list@html5` |
| `enumerated_list` | `block.rs:1668-1676` | `enumtype` (`arabic`,`loweralpha`,`upperalpha`,`lowerroman`,`upperroman`), `prefix`, `suffix`, `start` Int (only when ≠1) | = | same | `enumerated_list@base` |
| `list_item` | `block.rs:1567,1597`; doc fields `:9904,:9926`; toc `env/toctree.rs:458,595` | none | = | toctree-resolved items get `classes="toctree-lN"` (+`current`, `iscurrent`) | `list_item@base` |
| `definition_list` | `block.rs:1747`; glossary `:4564` (`classes=glossary`) | classes | = | same | `definition_list@base` (`<dl class="simple">` etc.) |
| `definition_list_item` | `block.rs:1753`, `:4642` | none | = | same | `@base` |
| `term` | `block.rs:1776`; glossary `:4671` (`ids=term-…` via `sphinx_make_id`, last child an `index`) | ids | = | same | `term@html5` (glossary term permalink) |
| `classifier` | `block.rs:1782` | none | = | same | `classifier@html5` |
| `definition` | `block.rs:1788`, `:4701` | none | = | same | `definition@html5` |
| `block_quote` | `block.rs:1907` (+classes `epigraph`/`highlights`/`pull-quote` via `:6847-6851`), `:7820` | classes | = | same | `block_quote@html5` |
| `attribution` | `block.rs:1913`, `:13040` | none | = | same | `attribution@base` |
| `literal_block` | see §6.4 | `xml:space="preserve"` always + variant attrs | = | + `language`/`force`/`linenos` stamped (HighlightLanguageTransform) | `literal_block@html5` (Pygments unless parsed-literal) |
| `doctest_block` | `block.rs:1941` | `xml:space` | = | + `classes="doctest"` (DoctestTransform, read 500) | `doctest_block@html5` |
| `line_block` / `line` | `block.rs:13045-13075` (`build_line_block`), used by `:2000` (`|` syntax) and `:7583` (directive) | classes/ids/names on directive form | = | same | `@base` |
| `comment` | `block.rs:2255` | `xml:space`; `#text` child only when non-empty | = | same; `only` exclusion also inserts empty `comment` | `comment@html5` → SkipNode |
| `target` | §6.9 | ids/names/refuri/refname/refid/anonymous/ismod | = | PropagateTargets: block target → `refid`, ids moved; refname→refuri (External/IndirectTargets) | `target@base` (`<span class="target" id>` only when no refuri/refid/refname) |
| `system_message` | §6.1 | level/line/source/type/ids/backrefs | = | removed entirely unless `keep_warnings` (then level ≥ 2 kept) | `system_message@base` (`<aside class="system-message">`) |
| `problematic` | `inline.rs:473-478`, `:822-827` | ids, refid | = | same (+ new ones from Dangling/Substitutions transforms) | `problematic@base` (`<a href="#refid">`) |
| `field_list` / `field` / `field_name` / `field_body` | `block.rs:2392-2426`; confval `:5722-5736`; doc fields `:9853-9969` | none (field_list may keep ids/names/classes through doc-field rebuild, `:9666-9672`) | = | a **leading** document field list is converted to `docinfo` and popped by `MetadataCollector` (§6.2) | `field_list@html5`, `field@html5` |
| `option_list` … `option_argument` | `block.rs:2480-2514` | `option_argument[delimiter]` | = | same | `@base` |
| `description` | `block.rs:2512` | none | = | same | `@base` |
| `table` | grid `:2815`, simple `:3191`, directive `:6679` | classes (`colwidths-auto`/`colwidths-given` + user), `align`, `width`, ids/names | = | captioned (titled) tables get `ids="idN"` (AutoNumbering) | `table@html5` |
| `tgroup` | `:2816,:3192,:6689` | `cols` Int | = | same | `@base` |
| `colspec` | `:2819,:3195,:6692` | `colwidth` Int, `stub` Int(1) | = | same | `@base` |
| `thead`/`tbody`/`row` | grid/simple/directive builders | none | = | same | `row@html5`, others `@base` |
| `entry` | `:2768,:3142,:6455,:6614` | `morecols`, `morerows` Int | = | same | `@base` |
| `footnote` | `block.rs:2341` | `names` (manual/`#name`), `auto` (`Int 1` for `#`, `Str "*"`), ids; `label` child only for manual numbers | = | Footnotes transform: auto numbering, `label` child with number, `names` set to number for anonymous auto, `backrefs`, + `docname` (FootnoteDocnameUpdater) | `footnote@base` |
| `citation` | `block.rs:2341` | names, ids, `label` child | = | + `backrefs`, `docname`, `label[support_smartquotes=0]` | `citation@base` |
| `label` | `block.rs:2366` | none | = | footnote/citation labels (numbered) | `label@base` |
| `substitution_definition` | `block.rs:7733` | `names` (whitespace-normalized, **case kept**), `dupnames`, `ltrim`/`rtrim` Int | = | stays | SkipNode (`@base`) |
| `pending` | `block.rs:7628` (**docutils mode only**; sphinx mode stamps classes instead) | `#text` child with `.. internal attributes:` dump | n/a in builds | never in Sphinx HTML | unknown_visit |
| `raw` | `block.rs:7536` | `format` (lowercased/ws-normalized), `xml:space`, `source` (`:file:`), classes | = | same | `raw@base` (emitted iff `'html' in format.split()`) |
| `compound` | `block.rs:6866` (`.. compound::`), `:6042` (toctree wrapper, `classes=toctree-wrapper`) | classes, ids/names | = | toctree wrapper keeps; child toctree replaced (§6.3) | `compound@base` |
| `container` | `block.rs:6911` (`.. container::`), `:5882` (code-block caption), `:4251` (literalinclude caption) | classes (`literal-block-wrapper`), `literal_block` Int 1, ids/names | = | code-block caption container gets `ids="idN"` (AutoNumbering) | `container@poly` |
| `caption` | `block.rs:7270` (figure), `:5896`, `:4257` | none | = | same (inside figure; code captions) | `caption@html5` (`code-block-caption` div) |
| `legend` | `block.rs:7290` | none | = | same | `legend@poly` |
| `figure` | `block.rs:7232` | `width`, `align`, classes (figclass), ids/names (`:figname:`; sphinx-mode `:name:` moved here) | = | unnamed figure gets `ids="idN"` (AutoNumbering) | `figure@html5` |
| `image` | `block.rs:7163` | `uri`, `alt`, `height`, `width`, `align`, `loading`, `scale` (Int or Str), classes, ids/names | = | + `candidates="{'*': 'path'}"`, `uri` rewritten srcdir-relative (+`original_uri`), wrapped in `reference internal=1 refuri=_images/x` when scaled/sized (`post_process_images`) | `image@html5` |
| `topic` / `sidebar` | `block.rs:6777` | classes, ids/names; children title, [subtitle], messages, content | = | `contents` topics (`classes="contents local"`) not supported | `topic@poly`, `sidebar@poly` |
| `rubric` | `block.rs:6813` | classes, ids/names | = | Sphinx `:heading-level:` → `heading-level` attr (option rejected here) | `rubric@html5` |
| admonitions `note`,`warning`,`tip`,`hint`,`important`,`caution`,`danger`,`error`,`attention` | `block.rs:7042` (`run_admonition`) | classes, ids/names | = | Sphinx adds `collapsible="open|closed"` via `:collapsible:` (option rejected here) | per-kind `@html5` |
| `admonition` | `block.rs:7069` | classes (`admonition-<make_id(title)>` or `:class:`), title child | = | same | `admonition@html5` |

### 4.2 Inline elements (docutils)

| kind | producers | attrs | Resolve | Sphinx@write / gaps |
|---|---|---|---|---|
| `emphasis`, `strong`, `literal`, `subscript`, `superscript`, `title_reference`, `abbreviation`, `acronym`, `math` | `inline.rs:459-465` (`emit_inline`), `:1494-1502` (roles), `:497-503` (markup) | none (`literal classes=code` for `:code:`, `:1503-1510`) | = | Sphinx `:code:` adds `language=""`; `:abbr:` gives `abbreviation[explanation]`; text smartquoted except in literal/not_smartquotable |
| `reference` | `inline.rs:440` (standalone URI/email → `refuri`), `:590` (`word_`/`word__`: `name`, `refname` or `anonymous=1`), `:974` (pep/rfc/cve/cwe sphinx: `classes`, `internal=0`, `refuri`, `strong` child), `:1521,:1553` (docutils pep/rfc), `:1597` (phrase refs), `:1781` (`|sub|_` wrapper); `block.rs:7151` (image `:target:`) | `name`, `refname`, `refuri`, `anonymous`, `internal`, classes | R-created: `resolve.rs:1697 reference_node` (`internal=1`, `refid`|`refuri`, `reftitle`, `title`), `:1571 intersphinx_node` (`internal=0`, `refuri`, `reftitle`); toc refs `env/toctree.rs:450,587` (`anchorname`, `internal=1`, `refuri=docname`) | refname-only references must become `refuri`/`refid` (External/Internal/Indirect/AnonymousHyperlinks); unknown names → ERROR `Unknown target name: "x".` + `problematic` (DanglingReferences). None of that exists. |
| `footnote_reference` | `inline.rs:1731-1752` | ids, `refname`, `auto` (1/`"*"`), text child only for manual numbers | = | Footnotes: `refid`, number text, `docname` |
| `citation_reference` | `inline.rs:1723-1729` | ids, `refname`, label text | = | Sphinx converts to `pending_xref refdomain=citation reftype=ref` (read 619) then resolves to `<reference ids=idN internal=1 refid=…><inline>[LABEL]` |
| `substitution_reference` | `inline.rs:1770-1791` | `refname` (ws-normalized, case kept) | = | replaced by the definition's children (Substitutions 220; `|version|`/`|release|`/`|today|` from DefaultSubstitutions 210); undefined → ERROR `Undefined substitution referenced: "x".` + problematic |
| `problematic` | `inline.rs:473,822` | ids, refid | = | same |
| `target` (inline) | `inline.rs:651` (`` _`x` ``, text child), `:968,:1398` (index targets, `ids=index-N`), `:1623,:1666` (phrase-ref named targets: `refname`/`refuri`) | | = | same (inline targets don't propagate) |
| `inline` | `block.rs:5781` (versionmodified lead-in `classes="versionmodified added|changed|deprecated|removed"`), `:11855` (`classes=ln` line numbers), `inline.rs:1337` (xref content node, `classes="xref std std-ref"` / `"xref std std-term"` / `"xref std std-doc"`), `arglist.rs:610` (`default_value`, `support_smartquotes=0`) | classes | R: `resolve.rs:1719` builds `inline classes="std std-ref"` / `"std std-numref"` / `"doc"` inside resolved references | Sphinx guilabel/menuselection produce `inline classes=… rawtext=…` (not here) |
| `literal_strong`, `literal_emphasis`, `emphasis` (doc fields) | `block.rs:9460 doc_field_inline` via `:9867-9882`, `:9955` | none | = | same (`literal_strong`/`literal_emphasis` are `not_smartquotable`; HTML `<strong>`/`<em>`) |

### 4.3 Sphinx-specific elements

| kind | producers | attrs | Resolve | Sphinx@write / gaps |
|---|---|---|---|---|
| `toctree` | `block.rs:6004-6041` inside `compound.toctree-wrapper` | `caption` (Str or `"True"`=None), `entries` (List of `"(None, 'a')"`/`"('Title', 'a')"`), `glob`, `hidden`, `includefiles` (List), `includehidden`, `maxdepth` (Int, -1), `numbered` (Int; bare flag = 999), `parent` (docname), `rawentries` (**always `Str("")`**, `:6037`), `titlesonly`; `:class:`/`:name:` options ignored | = (**unresolved**) | read-phase divergence (probe4, unpinned by any oracle case): Sphinx's `PreserveTranslatableMessages` (prio 10, `SP/addnodes.py:60-70`) sets `rawentries` = List of the explicit entry titles (`rawentries="Custom\ Title"`) and `rawcaption="My Caption"` when a caption is set; `:class:`/`:name:` land on the wrapper (`<compound classes="toctree-wrapper extra" ids="tocname" names="tocname">`). At write time the toctree is replaced by a `compact_paragraph toctree="1"` tree or removed when hidden (§6.3) |
| `compact_paragraph` | only env: `env/toctree.rs:456,592` (`skip_section_number=1` for object entries) | | n/a in body | body: toctree resolution output (`toctree=1`, `classes=toctree-lN`) |
| `only` | `block.rs:5938` | `expr` Str | = | **removed**: children hoisted if tags match, else replaced by an empty `comment` (`SP/util/nodes.py:719-729`, post 50); toc copies wrapped (`env/toctree.rs:464-474`) |
| `index` | `block.rs:4464` (directive, `inline=0`), `:4686` (glossary term), `:4901` (desc), `:5679` (py:module); `inline.rs:956,1383` (pep/rfc/cve/cwe/envvar roles) | `entries` List of 5-tuple reprs (`block.rs:10077 index_entry_tuple`), `inline` Int only on the directive | = | Sphinx reorders index/target runs (ReorderConsecutiveTargetAndIndexNodes 220); HTML SkipNode (`visit_index@html5:810`) |
| `highlightlang` | `block.rs:5921` | `force` Int, `lang` Str, `linenothreshold` Int (`i64::MAX` default = `sys.maxsize`) | = | consumed + **removed** by HighlightLanguageTransform (post 400) |
| `hlist` / `hlistcol` | `block.rs:4496,4508` | `hlist[ncolumns]` **Str** (`str(ncolumns)`), each col one `bullet_list` (no `bullet` attr) | = | same | 
| `glossary` | `block.rs:4559` | `sorted` Int; child `definition_list classes=glossary` | = | same; PropagateTargets may add ids (index target before it) |
| `seealso` | `block.rs:5811` | classes, ids/names | = | same (+`collapsible`) |
| `versionmodified` | `block.rs:5771` | `type` (`versionadded`,`versionchanged`,`deprecated`,`versionremoved`), `version`; one `paragraph translatable=0` starting with the lead-in `inline` | = | **multi-paragraph content and `argument + content` diverge** (§6.12) |
| `pending_xref` | `inline.rs:1226` (roles), `:1051` (failed `:external:`), `annotations.rs:138` (py annotations), `block.rs:9515` (doc-field xrefs) | §6.8 | **R: replaced** (reference / contnode / `*` condition children) except `refdomain ∉ {"", std, py}` (math/c/cpp/js/rst: counted, contnode kept after intersphinx miss) | never reaches the writer |
| `pending_xref_condition` | `annotations.rs:160`, `block.rs:9556` | `condition` (`resolved`/`*`) | R: dissolved (`resolve.rs:1460-1545`) | never reaches the writer |
| `number_reference` | only `resolve.rs:263-273` → `:1697` | `internal=1`, `refid`/`refuri`, `title` (the numfig *format*), child `inline classes="std std-numref"` | R-created | same |
| `math_block` | sphinx `block.rs:4424` (§6.10); docutils mode `:7428` | | = | same |
| `desc*` family | §6.7 | | `desc_signature` classes += domain (`resolve.rs:831-847`) | same |

### 4.4 HTML5Translator dispatch facts (MRO-resolved, probed with `HTML5Translator`)

`SphinxTranslator.dispatch_visit` walks the node class MRO for the first `visit_<cls>`:
- `desc_sig_space|name|operator|punctuation|keyword|keyword_type|literal_number|literal_string|literal_char`
  → `visit_inline@poly` (a `<span class="…">`), via `desc_sig_element > inline`.
- `desc_returns` → its own `visit_desc_returns` (subclass of `desc_type`);
  `desc_classname` → `visit_desc_addname`.
- `compact_paragraph`, `glossary`, `tabular_col_spec`, `index`, `toctree`, `comment`,
  `substitution_definition` → pass/SkipNode (`html5.py:369` comment, `:728` compact_paragraph
  pass, `:805` toctree SkipNode — "only happens when formatting a toc from env.tocs", i.e.
  the local-TOC render of `env.tocs` must skip the toctree copies `build_toc` put there —
  `:810-816` index/tabular_col_spec/glossary; `_html_base.py:1601` substitution_definition).
- `number_reference`, `download_reference` have their own visitors (reference subclasses).
- **No visitor at all** (`unknown_visit` → NotImplementedError in Sphinx): `pending_xref`,
  `pending_xref_condition`, `only`, `highlightlang`, `pending`. These must be gone before
  writing — in the crate `only` and `highlightlang` survive resolve today.
- Node categories that matter for SmartQuotes: `not_smartquotable` = `desc_addname`,
  `desc_classname`, `desc_inline`, `desc_name`, `desc_signature`, all `desc_sig_*`,
  `literal_emphasis`, `literal_strong` (plus docutils' own exclusions and any
  `support_smartquotes=0` node).

---

## 5. Encodings shared across kinds

- **None sentinel `"True"`** (decode with `crate::env::std_domain::is_none_sentinel`,
  `std_domain.rs:332`): `toctree[caption]` (`block.rs:6005-6009`), `math_block[label]` and
  `[number]` for unlabelled sphinx math (`:4446-4447`), `pending_xref[py:module|py:class]`
  outside a scope (`inline.rs:1231-1238`, `annotations.rs:141-148`, `block.rs:9531-9538`),
  `pending_xref[std:program]` outside `.. program::` (`inline.rs:1324-1329`),
  `desc_signature[module]` with no module (`block.rs:5229-5232`). Known ambiguity: a real
  value `"True"` (e.g. `:caption: True`) is indistinguishable (`std_domain.rs:328-331`).
- **Python bool → `Int(0|1)`**: `refexplicit`, `refwarn`, `refspecific`, `internal`,
  `anonymous`, `glob`, `hidden`, `titlesonly`, `includehidden`, `sorted`, `force`,
  `linenos`, `literal_block` (container), `nowrap`/`no-wrap`, `no-index*`,
  `multi_line_parameter_list`, `multi_line_trailing_comma`, `support_smartquotes`,
  `translatable`, `ismod`, `skip_section_number`, `inline` (index), `intersphinx`.
- **Python-repr strings** the writer must parse:
  - `literal_block[highlight_args]`: code-block `"{}"` or `"{'hl_lines': [1, 3]}"`
    (`block.rs:5852-5863`); literalinclude always has `linenostart`:
    `"{'linenostart': 5}"` or `"{'hl_lines': [2], 'linenostart': 5}"` (`:4174-4185`).
    HTML5Translator passes `hl_lines`, `linenostart` (+`force`) to the highlighter.
  - `desc_signature[_toc_parts]` `"()"`, `"('mod', 'func')"`, `"('a',)"` (`block.rs:9159`).
  - `index[entries]` items: `"('single', 'value', 'index-0', '', None)"` — inverse parser
    `env/genindex.rs parse_index_entries`.
  - `toctree[entries]` items `"(None, 'doc')"` / `"('Title', 'doc')"` — but the structured
    form is on `ToctreeRecord` (`src/rst/mod.rs:131-153`) and `ResolvedEntries`
    (`env/toctree.rs:134-167`).
- **`scale`** on image: `Int` normally, `Str` for arbitrary-precision digits
  (`block.rs:7171-7173`).

---

## 6. Deep dives

### 6.1 `system_message`

Construction (`src/doctree/messages.rs:23-51`):
```
<system_message level="2" line="3" source="<path>" type="WARNING">   (+ ids, backrefs when paired)
    <paragraph>
        Title underline too short.
    [<literal_block xml:space="preserve">        raw source block (directive errors, title errors)]
    [<paragraph>        Established title styles: = -   (inconsistent-title only)]
```
- `level` Int 1..4 ↔ `type` `INFO|WARNING|ERROR|SEVERE` (`messages.rs:5-17`).
- `line` Int = absolute 1-based line within `source`; `source` = the path string of the
  source-table entry the message was raised in (`block.rs:712-724`, `msg`/`msg_sm`;
  `msg_sm` adds `line_bias` inside table cells). Paths are what the builder passed as
  `ParseOptions.source_path` for the document (`src/parser.rs:137`,
  `file_path.display()`), and **srcdir-relative** for included files (divergence ledgered
  in IMPLEMENTATION_STATUS "Provenance path spelling"; Sphinx prints absolute).
- `span` is `Span::ZERO` — do not use spans for these.
- Pairing with `problematic`: message id allocated first, problematic second:
  `<problematic ids="id2" refid="id1">` + `<system_message ids="id1" backrefs="id2" …>`
  (`inline.rs:469-488`, `:810-832`). Duplicate-name INFO/WARNING carry `backrefs=[new id]`
  for internal targets (`ids.rs:353-359`).
- Placement: inline messages are returned in `InlineResult.messages` and placed by the
  block parser: after the paragraph (`block.rs:1367`), after the title inside the section
  (`:884-892`), inside `definition` before content for term/classifier messages
  (`:1789-1791`), after rubric/parsed-literal/line-block, etc. Directive errors replace the
  directive's output. Glossary misformat warnings precede the glossary node and report
  **one line low** (`:4543-4548`, `:4748-4755`).
- Nothing prints them today (IMPLEMENTATION_STATUS "Diagnostics"). Sphinx's
  `LoggingReporter` (`SP/util/docutils.py:385-421`) writes `msg.astext()` =
  `"{source}:{line}: ({TYPE}/{level}) {children joined with '\n\n'}"` for level ≥
  report_level (2 → WARNING/ERROR/SEVERE, INFO not printed), and `WarningStream.write`
  re-logs it with `type='docutils'` (the `[docutils]` suffix). Level names map through
  `SP/util/logging.py:30-35` (`'SEVERE' → logging.CRITICAL`) and the rendered prefix is
  `SphinxWarningLogRecord.prefix` (`:116-126`): `WARNING: `, `ERROR: `, `CRITICAL: ` — so a
  SEVERE prints as `path:line: CRITICAL: text [docutils]`. The crate helper `block.rs:11423 system_message_astext` already renders that
  exact string. Note Sphinx prints at *creation time* (parse order), which is not always
  tree pre-order, and transform-created messages (Footnotes, DanglingReferences,
  Substitutions, Transitions) print too.
- HTML: `visit_system_message@base` (`DU/writers/_html_base.py:1637-1664`):
  `<aside class="system-message" id=…><p class="system-message-title">System Message:
  TYPE/level (<span class="docutils literal">source</span>, line N); <em><a
  href="#backref">backlink</a></em></p>…</aside>` — reached only with `keep_warnings=True`,
  where `FilterSystemMessages` keeps level ≥ 2; default builds drop all of them
  (`SP/transforms/__init__.py:337-347`).

### 6.2 `docinfo` / `field_list` "orphan"

- The crate never builds `docinfo` or bibliographic nodes (author, date, …). A leading
  field list (`:orphan:`, `:tocdepth:`, `:nocomments:`) stays a plain `field_list` child of
  `document` (or wherever it is).
- `env/metadata.rs:65 document_metadata` reads the first non-PreBibliographic *document
  child* if it is a `field_list` (`is_pre_bibliographic`, `:52-61`: comment, target,
  system_message, title, subtitle, substitution_definition, pending, raw, meta, decoration).
- Sphinx: docutils `DocInfo` (340) converts it to `docinfo`, then
  `MetadataCollector.process_doc` **pops it** (`SP/environment/collectors/metadata.py:40-73`).
  Probe (`index.writer.pformat`): the `:orphan:\n:tocdepth: 2` list is gone at write time.
  → Before writing, remove exactly the node `document_metadata` read (this closes the
  `orphan_doc/orphan` KNOWN_RESOLVED_GAPS entry, `tests/env_differential.rs:647-653`).
- Known gap (metadata.rs:21-32): a field list *below* a single section title is not
  metadata here; with `doctitle_xform=False` in Sphinx that is also not docinfo (only a
  document-level list is), so no action needed beyond the pop.

### 6.3 `toctree` after resolve (today: unresolved)

Crate resolved tree (unchanged from parse), e.g. `toctree_numbered/index`:
```
<compound classes="toctree-wrapper">
    <toctree caption="True" entries="(None,\ 'a') (None,\ 'b')" glob="0" hidden="0"
             includefiles="a b" includehidden="0" maxdepth="-1" numbered="999"
             parent="index" rawentries="" titlesonly="0">
```
Sphinx oracle (`tests/fixtures/env_differential.json`, dummy builder so `refuri=""`):
```
<compound classes="toctree-wrapper">
    <compact_paragraph toctree="1">
        <bullet_list>
            <list_item classes="toctree-l1">
                <compact_paragraph classes="toctree-l1">
                    <reference anchorname="" internal="1" refuri="" secnumber="1">
                        A
                <bullet_list>
                    <list_item classes="toctree-l2">
                        <compact_paragraph classes="toctree-l2">
                            <reference anchorname="#sub" internal="1" refuri="#sub" secnumber="1 1">
```
With the html builder (probe): `refuri="other.html"`, `refuri="other.html#sub-a"`, and a
`:caption:` becomes `<title>Contents</title>` as the first child of the
`compact_paragraph toctree="1"`. `secnumber` is a space-joined list (`"1 1"`). Hidden
toctrees: `_resolve_toctree` returns None and the toctree node is replaced by `[]`
(the wrapper compound stays, empty). Implementation reference:
`SP/environment/adapters/toctree.py:119 _resolve_toctree`, `:223 _entries_from_toctree`,
`:309 _toctree_entry`, `:455 _toctree_add_classes`, `:485 _toctree_copy`;
`SP/environment/__init__.py:668-720 get_and_resolve_doctree`. The inputs already exist:
`env.tocs[docname]` (`env/toctree.rs:424 build_toc` — entries are
`list_item > compact_paragraph > reference[anchorname, internal=1, refuri=<docname>]`,
nested `bullet_list`, and copied `toctree` nodes for nesting), `env.toc_secnumbers`
(`env/numbers.rs`), `env.toctree_includes`, `env.titles`. `KNOWN_RESOLVED_GAPS` lists 27
documents blocked only by this (`tests/env_differential.rs:611-694`).

### 6.4 `literal_block` variants

| origin | producer | attributes |
|---|---|---|
| `::` / quoted literal | `block.rs:1424`, `:1457` | `xml:space` only |
| docutils `code` (and **`.. code::` in sphinx mode** — `sphinx_directive_spec` has no `code`) | `block.rs:7382` | `classes=["code", …user]`, `xml:space`, optional `source`; `:number-lines:` → children alternate `inline classes=ln` + `#text` (`:11844 push_number_lines`); with a language argument: **WARNING "Cannot analyze code. Pygments package not found."** instead of a node (`:7349-7358`) |
| `parsed-literal` | `block.rs:6939` | `xml:space`, classes, ids/names; **children are inline nodes** |
| `code-block`/`sourcecode` | `block.rs:5864-5912` | `force` Int, `highlight_args` Str repr, `language` (argument → `.. highlight::` state → `"default"`), `linenos=1` only with `:linenos:`, classes, ids/names (or on the caption container) |
| `include :literal:` | `block.rs:3981` | `source` (display path), classes, `xml:space`, ids/names; `:number-lines:` children |
| `include :code:` | via `run_code_with_lines` `:4075` | as docutils `code` + `source` |
| `literalinclude` | `block.rs:4128-4193` | `force`, `language` (only if `:language:` or `udiff` for `:diff:`; **no highlight_language fallback**), `linenos=1` iff `:linenos:`/`:lineno-start:`/`:lineno-match:`, classes, `highlight_args` (always `linenostart`), `source` (path), `xml:space`; text child omitted when empty |
| system_message payload | `messages.rs:38` | `xml:space` |

Sphinx write-time differences:
- `HighlightLanguageTransform` (post 400, `SP/transforms/post_transforms/code.py:30-81`)
  stamps missing `language`+`force` from the innermost `highlightlang` (or config
  `highlight_language`, default `'default'`) and missing `linenos` =
  `astext().count('\n') >= linenothreshold - 1`, then deletes all `highlightlang` nodes.
  Probe: `::` block → `force="0" language="default" linenos="0"`; after
  `.. highlight:: c :linenothreshold: 2`, a 2-line `::` block → `language="c" linenos="1"`.
- Sphinx's `.. code::` is `sphinx.directives.patches.Code` (`SP/directives/patches.py:232`):
  probe `.. code:: python` → `literal_block force="0" highlight_args="{}" language="python"
  linenos="0"`. The crate emits the docutils Pygments-less shape/warning instead.
- `TrimDoctestFlagsTransform` (post 401) rewrites pycon/`>>>` blocks and every
  `doctest_block` (removes `# doctest:` flags, `<BLANKLINE>`) when `trim_doctest_flags`
  (default True). It tests `node.rawsource != node.astext()` to skip parsed-literals.
- `HTML5Translator.visit_literal_block` (`SP/writers/html5.py:603-630`): **if
  `node.rawsource != node.astext()` → plain `<pre>` (no highlighting)**, else highlights
  `node.rawsource`. The crate has no `rawsource`. Equivalence rule for the writer: a
  literal_block whose children are not exactly one `#text` (parsed-literal with markup, or
  `:number-lines:` `inline.ln` children) is "rawsource ≠ astext". Edge: a parsed-literal
  whose content has no markup and no backslash escapes has `rawsource == astext` in docutils
  and **is highlighted**; one with a backslash escape is not (rawsource keeps the
  backslash). The crate cannot distinguish the latter case today (the unescaped text is all
  it keeps) — needs a flag/attribute or `rawsource` field (open question §11).

### 6.5 Images and figures

- `image` (`block.rs:7163-7196`): attrs in option order, `uri` from `uri_from_argument`
  (`:12692`), optional wrapping `reference` for `:target:` (`refuri` or `name`+`refname`).
- Missing vs Sphinx: `ImageCollector.process_doc` (`SP/environment/collectors/asset.py:48-104`)
  sets `candidates` (`"{'*': 'sub/pic.png'}"`; `'?'` key for `data:`/`://` URIs; glob
  `*` URIs map mimetypes), rewrites `uri` to srcdir-relative (and sets `original_uri` when
  changed), records `env.images` (for `_images/` copying), warns `image file not readable:
  %s [image.not_readable]` (3 KNOWN_WARNING_GAPS projects). At write time
  `StandaloneHTMLBuilder.post_process_images` wraps sized/scaled images not already in a
  reference in `reference internal="1" refuri="_images/pic.png"` (probe), and
  `visit_image@html5` maps `uri` through `builder.images` → `_images/<unique name>`.
- `figure` (`block.rs:7201-7307`): `image` first, then targets, `caption` (from the first
  paragraph's children), `legend`; sphinx mode puts `:name:` on the figure after the
  caption parse. Missing: AutoNumbering id for an unnamed captioned figure (`ids="id1"`).

### 6.6 Footnotes and citations

Crate parse-time shapes (no transform):
```
[#]_        → <footnote_reference auto="1" ids="id1">                    (no child)
[#named]_   → <footnote_reference auto="1" ids="id2" refname="named">
[1]_        → <footnote_reference ids="id3" refname="1">1
[*]_        → <footnote_reference auto="*" ids="id4">
[CIT]_      → <citation_reference ids="id5" refname="cit">CIT
.. [#] x    → <footnote auto="1" ids="id6">            (no label)
.. [#n] x   → <footnote auto="1" ids="n" names="n">
.. [1] x    → <footnote ids="id7" names="1"><label>1
.. [*] x    → <footnote auto="*" ids="id8">
.. [CIT] x  → <citation ids="cit" names="cit"><label>CIT
```
Sphinx write-time (probe, same markup): footnote refs gain `refid`, a number/symbol text
child and `docname="index"`; footnotes gain `backrefs`, a `label` child, `docname`, and
anonymous auto footnotes get `names="2"` (their number); the citation reference becomes
`<reference ids="id5" internal="1" refid="cit2002"><inline>[CIT2002]` and the citation
gets `backrefs`, `docname`, `label support_smartquotes="0"`. Transforms: docutils
`Footnotes` (620, numbering + symbol footnotes, errors like `Too many autonumbered footnote
references: only N corresponding footnote available.`), Sphinx
`CitationDefinitionTransform`/`CitationReferenceTransform` (619), `FootnoteDocnameUpdater`
(700), `UnreferencedFootnotesDetector` (622, warns). The std-domain harvest already skips
footnote labels (`env/std_domain.rs:230-232`).

### 6.7 `desc*` (object descriptions)

Anatomy (`block.rs:4762-4920`, py signature `:5153-5448`, std `:4927-5024`):
```
<index entries="('single', 'func() (in module mod)', 'mod.func', '', None)">
<desc classes="py function" desctype="function" domain="py" no-contents-entry="0" no-index="0"
      no-index-entry="0" no-typesetting="0" nocontentsentry="0" noindex="0" noindexentry="0"
      objtype="function">
    <desc_signature _toc_name="func()" _toc_parts="('mod', 'func')" class="" classes="sig sig-object"
                    fullname="func" ids="mod.func" module="mod">          (resolve adds class "py")
        <desc_annotation xml:space="preserve">  prefix keywords: desc_sig_keyword + desc_sig_space
        <desc_addname classes="sig-prename descclassname" xml:space="preserve">mod.
        <desc_name classes="sig-name descname" xml:space="preserve">func
        <desc_type_parameter_list multi_line_parameter_list=… multi_line_trailing_comma=… xml:space>
            <desc_type_parameter xml:space>  [*|**] desc_sig_name [: desc_sig_space desc_sig_name(ann)] [ = default]
        <desc_parameterlist multi_line_parameter_list="0" multi_line_trailing_comma="1" xml:space="preserve">
            <desc_parameter xml:space="preserve">
                <desc_sig_name classes="n">a  <desc_sig_punctuation classes="p">:  <desc_sig_space classes="w">
                <desc_sig_name classes="n">  (wrapper around parse_annotation output: pending_xref / text / desc_sig_*)
            <desc_parameter>  desc_sig_name b, desc_sig_operator =, <inline classes="default_value" support_smartquotes="0">1
            [<desc_parameter><desc_sig_operator classes="positional-only-separator o"><abbreviation explanation="…PEP 570)">/]
            [<desc_optional xml:space>…]           (pseudo_parse_arglist only)
        <desc_returns xml:space="preserve">  parse_annotation nodes
        <desc_annotation xml:space>  `:annotation:` / `:type:` / `:value:` / py:type `= canonical` tails
    <desc_content>   nested parse + doc-field-transformed field_list
```
Sources: `desc_name`/`desc_addname` `block.rs:10011-10032`; `desc_annotation`
`:9176-9180`; `desc_parameterlist`/`desc_type_parameter_list` attrs `arglist.rs:557-571`
(bare list without attrs when `needs_arglist` and no parens, `block.rs:5341-5347`; pseudo
fallback's total-failure list has only `xml:space`, `arglist.rs:444-455`);
`desc_parameter`/`desc_optional` `arglist.rs:573-583`; `desc_sig_*` leaves (class `w`,
`n`, `o`, `p`, `k`, `m`, `s`) `annotations.rs:444-485`; separators `arglist.rs:622-649`;
`default_value` `arglist.rs:609-617`.
- std kinds: `describe`/`object` (`domain=""`, no ids, classes just objtype),
  `envvar`/`confval` (`fullname` on confval, `:type:`/`:default:` field list prepended,
  `block.rs:5721-5741`), `option`/`cmdoption` (`allnames` List; multiple
  `desc_name`+`desc_addname` pairs separated by `desc_addname ", "`, `:4967-5024`).
  `_toc_parts="()" _toc_name=""` for std except confval under `toc_object_entries`.
- `:no-typesetting:` replaces the desc by a bare `target` with all collected ids
  (`:4905-4918`).
- `desc_sig_*` are docutils `inline` subclasses → `visit_inline@poly` → `<span class="n">`.
  Never emitted (Sphinx-only, C/C++/JS): `desc_sig_keyword_type`, `desc_sig_literal_char`,
  `desc_signature_line`, `desc_inline`, `desc_type`, `desc_sig_element` (abstract).
- Resolved shape: pending_xrefs inside annotations become `reference` (internal, `reftitle`)
  or plain text (builtins silenced: probe shows `str`/`int` as bare text in `desc_returns`
  and `desc_parameter`), and `propagate_desc_domain` appends the domain class to each
  `desc_signature` (`resolve.rs:831-847`).
- Doc fields (`block.rs:9644-9971`): `field_name` "Parameters"/"Variables"/"Raises"/
  "Returns"/"Return type"; typed items `literal_strong` name ` (` `literal_emphasis`/xref
  type `)` ` -- ` content. **Smartquotes turns ` -- ` into ` – ` in Sphinx output** (probe:
  `" – "`).

### 6.8 `pending_xref` (never reaches the writer)

Role producer (`inline.rs:1107-1404`): attrs `refdoc`, `refdomain` (`std`, `py`, `""` for
`:any:`, `math` for `:eq:`, or explicit `d:role` prefix), `refexplicit`, `reftarget`,
`reftype`, `refwarn` (only the seven std warn_dangling roles + `any` + `eq`), optional
`refspecific`, `py:module`/`py:class` (py roles; `:any:` copies existing ref_context keys
incl. `py:classes=""`/`py:modules=""`), `std:program` (`:option:`), `intersphinx`/
`inventory` (`:external:`) or `intersphinx_role_error`. Child: `inline` (std ref/term/doc)
or `literal` with classes `xref <domain> <domain>-<type>` (`xref <type>` for `:any:`/`:eq:`).
`:envvar:` additionally emits `index` + `target ids=index-N` before it (`:1377-1402`).
Annotation xrefs (`annotations.rs:126-167`) have no `refdoc`/`refexplicit`/`refwarn`,
`refspecific` Int 0/1, and either a `#text` child or two `pending_xref_condition`s.

Resolution (`resolve.rs:891-1134`) replaces it with: `reference` (resolved),
`number_reference` (numref), the contnode (Kept / failed / builtin-silenced), the `*`
condition's children (failed with conditions), or an intersphinx reference. **Domains other
than std/py/"" are left as their contnode** after an intersphinx miss (`:1018-1029`) —
includes all `:eq:`, `:c:*`, `:cpp:*`, `:js:*`, `:rst:*`, and every Sphinx non-xref role
wrongly parsed as a std xref (§9.2), whose std lookup via `resolve_obj` finds nothing.

### 6.9 `target` shapes (crate, parse time)

| markup | shape | producer |
|---|---|---|
| `.. _name: https://x` | `target ids names refuri` | `block.rs:2114-2152` |
| `.. _name:` | `target ids names` (internal) | same |
| `.. _name: other_` | `target ids names refname` | same |
| `.. __: uri` / `__ uri` | `target anonymous=1 ids=idN refuri|refname` | `:2124`, `:7906-7926` |
| `` _`inline` `` | `target ids names` + `#text` child | `inline.rs:651-670` |
| `` `txt <uri>`_ `` | `reference name refuri` + `target ids names refuri` | `inline.rs:1660-1676` |
| `` `txt <alias_>`_ `` | `reference name refname` + `target ids names refname` | `:1617-1633` |
| pep/rfc/cve/cwe/envvar roles | `target ids=index-N` | `inline.rs:968-973`, `:1398-1401` |
| `.. index::` | `target ids=index-N`; with `:name:` `target names=[normalized]` **no id** | `block.rs:4467-4475` |
| `.. math:: :label:` | `target refid=equation-<label>` before the `math_block` | `:4440-4443` |
| `.. py:module::` | `target ids=module-<name> ismod=1` (pre-propagation) | `:5652-5654` |
| `:no-typesetting:` desc | `target ids=[all desc ids]` | `:4913-4915` |

Sphinx write time: block-level internal targets donate `ids`/`names` to the next node and
keep `refid` (PropagateTargets 260; probe `.. _label-a:` → `<target refid="label-a">` +
`<section ids="section-two label-a" names="section\ two label-a">`); within a run of
consecutive sibling `target`/`index` nodes, every `index` is moved before the first
`target` (ReorderConsecutiveTargetAndIndexNodes, 220, `SP/transforms/__init__.py:446-491`
— "MUST run before PropagateTargets"): probe3 turns `index, target, index, target` from two
`.. index::` directives into `index, index, target, target`, after which the chained
targets collapse onto the final node (`<target refid="my-name">`, `<target
refid="index-0">`, `<paragraph ids="index-0 my-name" names="My\ Name">`).
External/indirect targets resolve references' `refname` → `refuri`. The crate replays
PropagateTargets read-only for label lookups (`env/std_domain.rs:485-693`) but never
applies it (KNOWN_RESOLVED_GAPS "PROPAGATE_TARGETS"/"PROPAGATE_MODULE_TARGETS").

### 6.10 `math` / `math_block`

- Inline `:math:` → `math` with the raw text (backslashes restored, `inline.rs:1502`).
- Sphinx `.. math::` (`block.rs:4408-4451`): blank-line blocks are **not** split (Sphinx
  keeps one node; argument prepended as `"arg\n\n"`), attrs `docname`, `no-wrap`/`nowrap`
  Int, `xml:space`, classes; labelled: `ids=[make_id("equation-<label>")]`, `label` Str,
  `number` Int (per-document `equation_serial`), preceded by `target refid`; unlabelled:
  `label="True" number="True"` (None).
- Docutils-mode `math` (`:7412-7446`) splits on blank lines into sibling `math_block`s.
- `:eq:` never resolves (math domain absent); Sphinx gives `<reference internal="1"
  refid="equation-eq1">(1)` (text node directly, per `math_eqref_format`).
- HTML: `visit_math_block@html5` with MathJax-style `\[...\]` in `div.math`, equation
  number span + permalink when `number` is set (writer research).

### 6.11 `only`

`only expr="html"` with nested children (`block.rs:5937-5943`). Resolve leaves it. Sphinx
post-transform `OnlyNodeTransform` (50) evaluates `expr` against builder tags (`html`,
`format_html`, `builder_html`, plus `-t` tags) and replaces the node by its children (or an
empty `comment` if it had none) or by an empty `comment` when false (probe: latex-only block
became `<comment xml:space="preserve">`). Tags: the crate has `-t` CLI parsing
(IMPLEMENTATION_STATUS CLI table) but `tags` config is "decorative".

### 6.12 `versionmodified`

Crate (`block.rs:5747-5797`): `versionmodified type version > paragraph translatable=0 >
[inline classes="versionmodified <label>" "<lead>: " | "<lead>."] + inline-parsed text`;
the text is `arguments[1]` **or** the whole content joined and inline-parsed as ONE
paragraph. Sphinx (`SP/domains/changeset.py:64-120`): argument[1] → its own paragraph;
content is *nested-parsed* and appended; the lead-in inline is inserted into the first
paragraph if the first child is a paragraph, else a new paragraph is inserted. Divergences
(probe `src3`, `src2`): `.. versionadded:: 1.0 Arg text.` + content → two paragraphs in
Sphinx, content dropped by the crate; multi-paragraph content → Sphinx keeps separate
paragraphs, crate merges them into one inline-parsed paragraph (with `\n\n` in the text);
content starting with a non-paragraph (list, code) → Sphinx inserts a lead paragraph.
Also hyphenated aliases `version-added|version-changed|version-deprecated|version-removed`
(`SP/domains/changeset.py:178-185`) are unknown directives here.

### 6.13 `glossary`, `hlist`, `seealso`, `index`, `highlightlang`

See the §4.3 table. Glossary term ids come from `sphinx_make_id("term", text)`
(`block.rs:4683-4685`), each term ends with an `index` child
(`('single', term, 'term-X', 'main', key)`). HTML: `visit_glossary` is a no-op wrapper,
`visit_term@html5` adds the permalink; `hlist` renders as a table (`visit_hlist@html5`).

### 6.14 `compact_paragraph`, `number_reference`, `download_reference`

- `compact_paragraph` only exists in env tocs (and must be produced by toctree resolution).
  HTML `visit_compact_paragraph@html5` is a pass (no `<p>`).
- `number_reference` (resolve only): writer renders like reference (`visit_number_reference`
  → `visit_reference`).
- `download_reference`: **never emitted** (§9.2).

### 6.15 `productionlist`, `centered`, `acks`, `tabular_col_spec`

Never emitted — the directives are unknown here (`Unknown directive type "…".` ERROR +
INFO). Sphinx shapes (probe): `<centered>Centered text`; `<acks><bullet_list bullet="*">…`;
`<tabular_col_spec spec="|l|l|">` (HTML SkipNode); `<productionlist><production
tokenname="rule" xml:space="preserve"><literal_strong ids="grammar-token-rule">rule` + text
`" ::= "` + `"a" | "b"` + `"\n"` (tokens become `pending_xref reftype=token`). `sectionauthor`/
`moduleauthor`/`codeauthor` produce nothing unless `show_authors`.

---

## 7. What the resolve pass changes (complete list)

`env/resolve.rs:777-801` + helpers:
1. Every `pending_xref` child is replaced (`resolve_children`, `:850-887`) — see §6.8.
2. `propagate_desc_domain` (`:831-847`): `desc_signature.classes += [desc.domain]` for
   non-empty domains.
Nothing else: no ids, no numbering stamps (`secnumber`, figure numbers are read from env at
write time by Sphinx's translator), no toctree, no `only`, no highlight stamps.
`relative_uri` is the dummy `''` (`builder.rs:1309`), so cross-doc `refuri`s are `""` or
`"#id"` — the HTML writer needs `get_relative_uri(from, to)` (e.g. `other.html`,
`../a/b.html`) injected into `Resolver.relative_uri` (`resolve.rs:103`). The env oracle
was generated with the dummy builder, so keep the dummy policy available for
`snapshot_env`/tests while the HTML path uses real URIs — resolve once per URI policy, or
emit warnings from one pass only.

---

## 8. Transform map (what must run before/at writing)

From `transforms.py` (Sphinx 9.1.0 registry; the docutils `SmartQuotes` at 855 is replaced
by Sphinx's at 750 in Sphinx's reader).

| prio | transform | effect on nodes | crate status |
|---|---|---|---|
| 10-25 | i18n (Locale, PreserveTranslatableMessages, TranslationProgressTotaliser) | `document[translation_progress]` | not modelled (oracles strip it) |
| 100 | RefOnlyBulletListTransform | inert with default `html_compact_lists=True` | n/a |
| 210 | AutoIndexUpgrader, AutoNumbering, DefaultSubstitutions, HandleCodeBlocks, MoveModuleTargets | implicit `idN` on captioned figure/table/code-block container; `|version|`/`|release|`/`|today|`; code-block unwrapping; module target moves | AutoNumbering only for literalinclude (`block.rs:4280-4283`); others missing |
| 220 | Substitutions | resolve `substitution_reference` | **missing** |
| 220 | ReorderConsecutiveTargetAndIndexNodes | reorder target/index runs | **missing** |
| 260 | PropagateTargets | block targets → `refid`, ids moved | replayed read-only only |
| 261 | SortIds | section `idN` ids last | **missing** |
| 340 | DocInfo | leading field list → `docinfo` | read by metadata.rs, never converted/popped |
| 440/460 | AnonymousHyperlinks / IndirectHyperlinks | `refuri` on anonymous refs; indirect chains | **missing** |
| 500 | DoctestTransform / GlossarySorter | `doctest_block classes=doctest` / sort | Doctest missing; GlossarySorter inline (`block.rs:4721`) |
| 619 | Citation{Definition,Reference}Transform | citation refs → pending_xref(citation) | **missing** |
| 620 | Footnotes | numbering, refid/backrefs/labels | **missing** |
| 622 | UnreferencedFootnotesDetector | warnings | **missing** |
| 640/660 | ExternalTargets / InternalTargets | refname → refuri / refid | **missing** |
| 700 | FootnoteDocnameUpdater | `docname` attr | **missing** |
| 750 | SphinxSmartQuotes | text rewrite (`smartquotes=True` default) | **missing** (oracles set `smartquotes=False`) |
| 830 | Transitions | edge-transition errors/hoisting | **missing** |
| 850 | DanglingReferences / SphinxDanglingReferences / SphinxDomains | unknown-target errors + problematic; `process_doc` of domains | dangling missing; domain collection done in env |
| 880 | DoctreeReadEvent (collectors: title, toctree, metadata, asset/images, dependencies, domains) | `candidates`, docinfo pop, … | partially (no ImageCollector, no pop) |
| 999 | FilterSystemMessages, RemoveTranslatableInline | drop messages; unwrap translatable inlines | not run (crate never creates the inlines) |
| post 10 | ReferencesResolver | pending_xref | done (`resolve.rs`) |
| post 50 | OnlyNodeTransform | only | **missing** |
| post 100/150 | ImageDownloader / DataURIExtractor | remote/data images | **missing** |
| post 200 | PropagateDescDomain / SigElementFallbackTransform | desc class; fallback inert for html | domain done |
| post 400 | HighlightLanguageTransform | literal_block stamps, drop highlightlang | **missing** (KNOWN_HIGHLIGHT_STAMP_GAPS) |
| post 401 | TrimDoctestFlagsTransform | strip doctest flags | **missing** |
| write | `_resolve_toctree` | toctree → compact_paragraph tree | **missing** |
| write | `post_process_images` | scaled-image link wrapper, `_images` mapping | **missing** |

---

## 9. Gaps: node kinds and constructs never emitted

### 9.1 docutils node classes never emitted
`address`, `author`, `authors`, `contact`, `copyright`, `date`, `organization`,
`revision`, `status`, `version`, `docinfo` (bibliographic — only via DocInfo, popped in
Sphinx anyway), `decoration`, `header`, `footer` (`.. header::`/`.. footer::` unknown),
`meta` (`.. meta::` unknown; Sphinx emits `<meta content name>` and the HTML head gets it),
`generated` (sectnum), `topic classes=contents` (`.. contents::` unknown), `sectnum`
auto titles (`title auto=1 refid`), `document[title]` (`.. title::` unknown). Also
docutils directives missing: `contents`, `sectnum`/`section-numbering`, `header`,
`footer`, `meta`, `title`, `role`, `default-role`, `target-notes`.

### 9.2 Sphinx nodes/roles never emitted (probe-verified shapes)
| construct | Sphinx write-time shape | crate today |
|---|---|---|
| `:kbd:`Ctrl+C`` | `<literal classes="kbd">Ctrl</literal>` `+` `<literal classes="kbd">C</literal>` (compound keys wrapped in `literal classes="kbd compound"`) | pending_xref std/kbd → `literal classes="xref std std-kbd"` |
| `:guilabel:`&Cancel`` | `<inline classes="guilabel" rawtext=":guilabel:`&Cancel`"><inline classes="accelerator">C</inline>ancel` | same wrong xref fallback |
| `:menuselection:`A --> B`` | `<inline classes="menuselection" rawtext=…>A ‣ B` | same |
| `:file:`/usr/{lib}`` / `:samp:` | `<literal classes="file" role="file">/usr/<emphasis>lib` | same |
| `:command:` `:program:` `:makevar:` | `literal_strong classes=<role>` | same |
| `:dfn:` | `emphasis classes="dfn"` | same |
| `:mimetype:` `:mailheader:` `:newsgroup:` | `literal_emphasis classes=<role>` | same |
| `:regexp:` | `literal classes="regexp"` | same |
| `:abbr:`LIFO (…)`` | `abbreviation explanation="last-in, first-out"` | same |
| `:manpage:`ls(1)`` | `<manpage classes="manpage" page="ls" path="ls(1)" section="1" xml:space="preserve">ls(1)` (`manpages_url` makes it a reference) | same |
| `:download:`pic <pic.png>`` | `<download_reference filename="<md5>/pic.png" refdoc refdomain="" refexplicit="1" reftarget="pic.png" reftype="download" refwarn="0"><literal classes="xref download">pic` | same |
| `:code:`x`` (Sphinx code_role) | `literal classes="code" language=""` | docutils shape (no `language`) |
| `:index:` role | `index entries=… ` + `target ids=index-N` + text (explicit title: `process_index_entry(target)`; `!x` → main) | ERROR `Interpreted text role "index" not implemented.` + problematic (`inline.rs:1445`, `:1579`) |
| `:eq:` | `reference internal=1 refid=equation-x` + `(1)` | contnode `literal classes="xref eq"` |
| `.. code::` | Sphinx `Code` literal_block (§6.4) | docutils code / Pygments WARNING |
| `.. centered::`, `.. acks::`, `.. tabularcolumns::`, `.. productionlist::`, `.. sectionauthor::`/`moduleauthor`/`codeauthor`, `.. cssclass::` alias, `.. version-*::` aliases, `.. default-role::`, `.. role::`, `.. meta::` | §6.15 / §9.1 | Unknown directive ERROR |
| admonition/seealso `:collapsible:` | `collapsible="open"` attr | "unknown option" error |
| `.. rubric:: :heading-level:` | `heading-level` attr | "unknown option" error |
| `.. index:: :name: X` | target `names=["X"]` (unnormalized), `ids=[make_id(X)]`, entries point at that id, **no `index-N` serial consumed** | `names=[normalized]`, no id, entries point at `index-N`, serial consumed (`block.rs:4455-4476`) |
| `.. toctree:: :class:/:name:` | on the `compound` wrapper (`classes="toctree-wrapper extra" ids="tocname" names="tocname"`) | ignored |
| `toctree[rawentries]` / `[rawcaption]` | List of explicit entry titles / the caption string (probe4) | `rawentries=""` constant, no `rawcaption` (`block.rs:6037`) |
| `start_of_file` | singlehtml only | n/a |
| `desc_signature_line`, `desc_inline`, `desc_type`, `desc_sig_keyword_type`, `desc_sig_literal_char` | C/C++/JS domains | domains not implemented |

---

## 10. Probe excerpts (writer-time Sphinx trees)

From `probe-doctree/index.writer.pformat` (html builder, default config + keep_warnings):
```
<paragraph>
    … “quotes” – dashes…                                  (smartquotes)
<paragraph>
    A <reference name="named" refuri="https://example.com/named">named
     link, an <reference anonymous="1" name="anonymous" refuri="https://example.com/anon">anonymous
    … <footnote_reference auto="1" docname="index" ids="id1" refid="id7">2
    … <reference ids="id5" internal="1" refid="cit2002"><inline>[CIT2002]
    . Sub <emphasis>replaced and TODAY .                   (substitutions applied)
<target ids="named" names="named" refuri="https://example.com/named">
<target anonymous="1" ids="id6" refuri="https://example.com/anon">
<footnote auto="1" backrefs="id1" docname="index" ids="id7" names="2"><label>2 …
<target refid="label-a">
<section ids="section-two label-a" names="section\ two label-a">
    <container classes="literal-block-wrapper" ids="id12" literal_block="1">
        <caption>Code caption
        <literal_block force="0" highlight_args="{'hl_lines': [1]}" language="python" linenos="1" xml:space="preserve">
    <literal_block force="0" language="default" linenos="0" xml:space="preserve">literal
    <doctest_block classes="doctest" xml:space="preserve">
    <target refid="equation-eq1">
    <math_block docname="index" ids="equation-eq1" label="eq1" no-wrap="0" nowrap="0" number="1" xml:space="preserve">
    <figure ids="id13"><image alt="alt text" candidates="{'*': 'pic.png'}" uri="pic.png"> <caption> <legend>
    <reference internal="1" refuri="_images/pic.png"><image candidates="{'*': 'pic.png'}" uri="pic.png" width="50%">
    <paragraph>Only html.
    <comment xml:space="preserve">                         (excluded `only latex`)
    <index … inline="0"> <target refid="index-3"> <glossary ids="index-3" sorted="0">
    <table ids="id14"><title>LT …
    <reference internal="1" refid="term-Term"><inline classes="xref std std-term">Term
    <reference internal="1" refid="mod.func" reftitle="mod.func"><literal classes="xref py py-func">mod.func()
    <reference internal="1" refid="equation-eq1">(1)
    <problematic ids="id11" refid="id10">*
    <system_message backrefs="id11" ids="id10" level="2" line="176" source="…/index.rst" type="WARNING">
```
The leading `:orphan:`/`:tocdepth:` field list is absent; the `toctree` is the resolved
`compact_paragraph toctree="1"` with a `<title>Contents` caption child and
`refuri="other.html"`/`"other.html#sub-a"`, `secnumber="1"`/`"1 1"`.

---

## 11. Risks / open questions for the design

1. **No `rawsource`**: needed for `visit_literal_block`'s parsed-literal test and
   `TrimDoctestFlagsTransform`. Options: derive (children ≠ single text), store a marker
   attribute that pformat must not print (the IR has no hidden-attr channel), or add a
   `rawsource: Option<String>` field (bincode shape change → DOCTREE_FORMAT_VERSION bump).
2. **Auto-id counter not persisted**: AutoNumbering/Footnotes/Dangling transforms allocate
   `idN` after the parse; to be byte-exact either run them inside the parser (registry
   alive) or export `IdRegistry.ids` + counter in `RegistryExport` (cache-shape rule: no
   `#[serde(default)]`).
3. **Where transforms live**: read transforms that change stored shape and feed env
   collectors (Footnotes, PropagateTargets, AutoNumbering, docinfo pop, smartquotes,
   substitutions, image candidates) belong at the end of the parse (so the doctree cache and
   env oracle see them); post-transforms (only, highlight stamping, toctree resolution,
   image post-processing) belong in a per-document write-time pass over the resolved clone.
   Every read-transform landing will change `sphinx_doctree_differential` expectations —
   that corpus deliberately EXCLUDES transform-visible cases, so new cases become
   admissible.
4. **Resolve twice vs once**: HTML needs real relative URIs; the env oracle compares
   dummy-builder `''` URIs. Warnings must not be duplicated.
5. **Smartquotes is on by default in Sphinx** but both oracles disable it; HTML parity with
   default projects requires a SphinxSmartQuotes port (language-dependent quote tables,
   `not_smartquotable` kinds: `desc_*`, `literal_*`, `desc_sig_*`, plus
   `support_smartquotes=0` nodes, literal/literal_block/math/raw/code…).
6. **The `"True"` None sentinel** cannot distinguish a literal `True`; an HTML-visible case
   is `.. toctree:: :caption: True` (no caption would be rendered).
7. **Sphinx-mode role table**: fixing §9.2 at parse time changes pending_xref counts and
   role records (`RoleRecord`) — check `directives/validation` consumers.
8. **`Node::astext` separator** differs from docutils for non-TextElements; writer code
   that ports `node.astext()` calls must use a faithful helper.
9. **Nested sections inside directives** (`allow_section_headings`) are not parsed
   (IMPLEMENTATION_STATUS, "Known gap"); writer section-depth logic will never see sections
   inside `desc_content`/`py:module` content.
