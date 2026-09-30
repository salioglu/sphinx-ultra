# M2 wave 5 research: the built-but-not-wired write-side stack. What to keep, what to rewrite

Research key: `deadstack`. Scope: `src/html_builder.rs`, `src/template.rs`, `templates/*`,
`static/*`, `src/search.rs`, `src/inventory.rs` (writer half), `src/directives.rs` (HTML
processor registry), `examples/`, `benches/builder_benchmark.rs`, and `Cargo.toml`. It also
answers one question in detail: **can minijinja 2.x render Sphinx's real `basic` and
`alabaster` templates byte-for-byte?**

Upstream paths used throughout:

* `SPHINX = /root/.cache/uv/archive-v0/b4dBDAdEzskuqge1iT52j/lib/python3.12/site-packages/sphinx` (9.1.0)
* `ALABASTER = …/site-packages/alabaster` (1.0.0)
* `MJ = /root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/minijinja-2.24.0/src` (the version `Cargo.lock` pins)
* Jinja2 3.1.6 and markupsafe 3.0.3 are the versions in the Sphinx oracle env. Pygments is 2.21.0.

Probe artifacts (all reproducible) live in
`/tmp/claude-0/-home-user-sphinx-ultra/46bf5e6b-694f-5b8e-ba0d-36f1851a8974/scratchpad/probe-deadstack/`:

* `compile_probe.py` compiles every basic, alabaster and static template verbatim with minijinja.
* `proj/` is a tiny 3-document project. `proj/mjhook.py` is a Sphinx extension that, during a real
  `sphinx-build`, **re-renders every page with minijinja** (through the preprocessor prototyped
  below) and writes the result to `out/mj/`. It also renders the templated static files at
  `build-finished`.
* `run_themes.sh` builds `proj` with each of 12 themes and `cmp`s the minijinja output against
  Sphinx's own output.
* `sem_probe.py` checks expression semantics, minijinja against Jinja2.
* `ast_scan.py` inventories every Jinja construct in the theme templates through Jinja2's own AST.
* `buildinfo_probe.py` dumps the `.buildinfo` input values and checks the hash algorithm by hand.
* `crate_tpl_probe.py` renders the crate's own `templates/*` the way `template.rs` would.
* `contrib/minijinja-contrib-2.24.0/` is the downloaded `minijinja-contrib` source (not a
  dependency today).

Run any Python probe with: `PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' --with minijinja python <script>`.
The `minijinja` PyPI package is the official binding and is version **2.24.0**, the same core
crate as ours. No cargo was run.

---

## 0. TL;DR: verdicts

| Artifact | Lines | Verdict | One-line reason |
|---|---|---|---|
| `src/inventory.rs` writer (`InventoryFile::dump`) | 347-432 | **KEEP as-is, wire it** | Bytewise-correct mirror of `InventoryFile.dump`. Wave 5 only has to produce its input: std + py `get_objects()` equivalents, `quote()`-exact `get_target_uri`, and `config.version`. |
| `src/html_builder.rs` | 788 | **Rewrite the bodies; keep the method skeleton at most** | Method names track `StandaloneHTMLBuilder`, but nearly every body is wrong or a TODO. `css_tag` gets structs and renders nothing, `.buildinfo` is JSON, the relative URIs are wrong, the global context is wrong. Async tokio I/O has no place in the rayon write phase. |
| `src/template.rs` | 387 | **Rewrite** (new theme + template module) | Default HTML autoescape is left on, so `{{ body }}` gets escaped. `pathto` ignores kwargs and depth. `css_tag`/`js_tag` accept strings only. `Null` maps to `UNDEFINED`, which breaks `is none`. User templates are never loaded. `e` is non-markupsafe. `striptags` is wrong. No `_`/trans. |
| `templates/*.html`, `opensearch.xml` | 8 files | **Delete** | Handwritten imitations, not Sphinx's templates. 4 of 7 fail at render (`_` unknown). Replace with **verbatim vendored `basic` + `alabaster`** and a load-time preprocessor. |
| `templates/base.hbs`, `document.hbs` | 287 | **Delete** | Orphan Handlebars files. The `handlebars` dependency was removed in 2026-08 and nothing references them. |
| `static/*` (5 shims) | 295 | **Delete** | Fake jQuery, fake doctools, RTD-ish CSS. Vendor Sphinx's `basic/static` + alabaster static instead. Page bytes depend on them through `?v=<crc32>` checksums. |
| `src/search.rs` | 473 | **Delete when the IndexBuilder port starts** (M3, or wave 5 if `searchindex.js` is in scope). Nothing to salvage. | The output schema is not `Search.setIndex`. The 3-rule stemmer is not Snowball `english`. `HashMap` order is nondeterministic. `add_object` desyncs the parallel vectors. `objtypes` is filled with type→type. |
| `src/directives.rs` HTML processor registry | 1-482 | **Delete** (keep only `pub mod validation;`) | Processes raw text, not doctrees. 28 of ~40 processors are comment stubs. Zero call sites. Its `DirectiveRegistry` name collides with `directives::validation::DirectiveRegistry`. |
| `examples/*.rs` | n/a | No action | Neither example touches the write stack (both are validation demos). `examples/README.md` advertises 4 example dirs that do not exist. |
| `benches/builder_benchmark.rs` | 234 | **Rewrite** | Panics at line 69, because `Parser::parse` calls `utils::get_file_mtime` on a nonexistent `test.rst` (`src/parser.rs:93`, `src/utils.rs:447`). The builder benches time the placeholder write. The cache bench is `black_box(42)`. |
| minijinja 2.24 as the engine | n/a | **Viable. Proven byte-exact with a preprocessor** | Verbatim `basic/layout.html` fails to compile (`block tags in macros are not allowed`). `trans` is unknown. Escaping differs. After two source rewrites plus markupsafe-exact filters, **218/218 files byte-identical** across 12 builtin themes (see §4). |

The single most important finding: `docs/research/themes.md:54` says "minijinja handles
everything except `{% trans %}`". **That is wrong.** Sphinx's `basic/layout.html` puts `{% block %}`
tags inside the `relbar()` and `sidebar()` macros, and minijinja rejects that at parse time
(`MJ/compiler/parser.rs:1087-1090`). agogo does the same inside `agogo_sidebar()`. A
macro-to-block rewrite is required as well, and it is specified and proven in §4.3.

---

## 1. Where the stack sits today

* `src/lib.rs:14,22-23` declares `html_builder`, `search`, `template`. `src/lib.rs:31-46`
  re-exports `Directive, DirectiveRegistry` (the **HTML** one, from `directives.rs`), `HTMLBuilder`,
  `InvObject, Inventory, InventoryFile, InventoryItem, posix_join`, `SearchIndex`, and `TemplateEngine`.
  Deleting or renaming any of them is a public-API change, so it needs a CHANGELOG entry.
* Call sites outside the modules themselves are all tests:
  * `tests/inventory_roundtrip.rs:21,340,423` uses `InventoryFile::dump`, `InvObject`.
  * Nothing outside its own file uses `HTMLBuilder`, `TemplateEngine`, `TemplateContext`, `SearchIndex`,
    `SearchIndexBuilder`, `DirectiveRegistry` (HTML), `DirectiveProcessor`, or `parse_directive`
    (grep over `src tests examples benches`).
  * Unit tests: `template.rs:362-387` (2 tests), `search.rs:391-473` (5 tests), `html_builder.rs` (none).
* The live write path is in `src/builder.rs`:
  * `read_one_file` (`builder.rs:874-883`) renders `"<html><body>{escaped raw source}</body></html>"`
    **in the read phase** and stores it in `Document.html` (`src/document.rs:47-48`). The
    incremental cache persists it (`src/cache.rs:305,430-490`). A real writer depends on global
    state (relations, other docs' titles, the toctree), so it **must move to after resolve**. The
    cached `Document.html` has to go, or be ignored.
  * `write_phase`/`write_one` (`builder.rs:1459-1480`) writes `document.html` to
    `source_path.with_extension("html")`.
  * `generate_indices` (`builder.rs:1545-1549`) and `generate_search_index`
    (`builder.rs:1767-1771`) are TODO no-ops.
  * `copy_static_assets` (`builder.rs:1551-1613`) copies the 5 `static/` shims. It locates them
    via `exe_dir/../static`, `../../static`, `../../../static`, `rust-builder/static`, falling
    back to `include_str!` copies (`builder.rs:1615-1638`). It then copies `srcdir/_static`
    **and `srcdir/_templates`** into the output. Sphinx never copies `_templates`, and copies
    `_static` only through `html_static_path` with the `exclude_patterns` + `**/.*` exclusions
    (`SPHINX/builders/html/__init__.py:877-893`).
* Config defaults that break parity before a template is even rendered:
  * `OutputConfig::default().html_theme = "sphinx_rtd_theme"` and `ThemeConfig::default().name = "sphinx_rtd_theme"`
    (`src/config.rs:479,492`). **Sphinx's default is `'alabaster'`**
    (`SPHINX/builders/html/__init__.py:1459`).
  * `html_use_opensearch: Option<bool>` (`config.rs:99`). In Sphinx it is a **str**, the base URL
    (default `''`). `opensearch.xml` renders `{{ use_opensearch }}/…`.
  * `html_css_files`/`html_js_files: Vec<String>` (`config.rs:60-63`). Sphinx accepts
    `str | (str, {attrs})`, converted by `convert_html_css_files`/`convert_html_js_files`
    (`SPHINX/builders/html/__init__.py:1295-1327`).
  * `copyright: Option<String>` (`config.rs:48`). Sphinx allows `str | list | tuple`, and
    `layout.html:181` branches on `copyright is iterable and copyright is not string`.
  * `html_title`/`html_short_title` default to `project` in `html_builder.rs:331-332`. Sphinx:
    `'%s %s documentation' % (project, release)`, and `html_short_title` defaults to
    `html_title` (`SPHINX/builders/html/__init__.py:1462-1470`).

---

## 2. `src/html_builder.rs`: method-by-method against `StandaloneHTMLBuilder`

Upstream reference: `SPHINX/builders/html/__init__.py` (class at line 109). The upstream
write order is `Builder.write` (`SPHINX/builders/__init__.py:705-750`):
`prepare_writing(docnames)` → `copy_assets()` (downloads, static, extra) → `write_documents()`
(sorted docnames, each `write_doc_serialized` + `write_doc`) → `finish()`. `finish()` runs
`gen_indices`, `gen_pages_from_extensions`, `gen_additional_pages` (search, opensearch),
`copy_image_files`, `write_buildinfo`, then `handle_finish` → `dump_search_index`,
`dump_inventory` (`__init__.py:675-683,1264-1270`). Order matters for bytes: **static files must
exist before any page is rendered**, because `css_tag`/`js_tag` hash them for `?v=`.

### 2.1 API surface (crate)

```rust
pub const INVENTORY_FILENAME: &str = "objects.inv";               // :15  (== upstream :77)
pub struct HTMLBuilder { name, format, epilog, out_suffix, link_suffix, searchindex_filename,
  allow_parallel, copysource, use_index, embedded, search, download_support,
  supported_image_types: Vec<String>, supported_remote_images, supported_data_uri_images,
  outdir, srcdir, confdir, static_dir, sources_dir, downloads_dir, images_dir: PathBuf,
  config: BuildConfig, current_docname: String, secnumbers: HashMap<String, Vec<u32>>,
  imgpath, dlpath: String, css_files: Vec<CSSFile>, js_files: Vec<JSFile>,
  template_engine: TemplateEngine, global_context: serde_json::Map<String, Value>,
  relations: HashMap<String, DocumentRelation>, domain_indices: Vec<DomainIndex> }   // :18-67
pub struct CSSFile { filename, priority: i32, media: Option<String>, id: Option<String>, rel, type_ } // :70-77
pub struct JSFile { filename, priority: i32, loading_method: String, async_: bool, defer: bool }      // :80-86
pub struct DocumentRelation { parent, prev, next: Option<String> }                                   // :89-93
pub struct DomainIndex { name, localname, shortname: Option<String>, content: Vec<IndexEntry>, collapse } // :96-102
pub struct IndexEntry { name, subentries: Vec<IndexEntry>, uri, display_name }                       // :105-110
impl HTMLBuilder {
  pub fn new(config, srcdir, outdir) -> Result<Self>            // :113
  pub async fn init(&mut self) -> Result<()>                    // :170
  pub async fn write_doc(&mut self, docname: &str, doctree: &Document) -> Result<()>  // :359
  pub fn get_target_uri(&self, docname: &str) -> String         // :530
  pub async fn gen_indices(&mut self) -> Result<()>             // :535
  pub async fn copy_static_files(&self) -> Result<()>           // :593
  pub async fn copy_image_files(&self, images: &HashMap<String,String>) -> Result<()>     // :647
  pub async fn copy_download_files(&self, downloads: &HashMap<String,String>) -> Result<()> // :673
  pub async fn dump_search_index(&self, &SearchIndex) -> Result<()>  // :699
  pub async fn write_build_info(&self) -> Result<()>            // :731
  pub async fn finish(&mut self, &SearchIndex) -> Result<()>    // :759
}
impl PartialEq for CSSFile / JSFile  // compare filename only        :778-788
```

### 2.2 Method comparison

| Crate (lines) | Upstream (lines) | Divergences | Salvage |
|---|---|---|---|
| constants in `new` (:122-142) | class attrs :112-137 | Match: `name='html'`, `format='html'`, suffixes, `searchindex_filename`, `supported_image_types` order, the remote/data-URI flags, `copysource`, `allow_parallel`, `embedded=False`, `search=True`, `use_index=False`, `download_support=True`. Missing: `html_scaled_image_link=True`, `add_permalinks=True`, `allow_sharp_as_current_path=True`, `indexer_dumps_unicode=True`. `epilog` is unused. | Keep the constants. |
| `new` (:113-167) | `__init__` :139-160 | Constructs `TemplateEngine::new(&config)`, which reads `templates` relative to the **CWD** (`template.rs:29`). | Rewrite. |
| `init` (:170-191) | `init` :162-186 | Creates `_static/_sources/_downloads/_images` eagerly. Sphinx creates `_sources` in `prepare_writing` (:559-561) and the others lazily, and only if needed. **An empty `_downloads`/`_images` dir is a tree divergence.** No `BuildInfo`, no theme, no highlighter. Ignores `html_file_suffix`/`html_link_suffix` (:176-184). `use_index` comes from `html_use_index` (OK). | Rewrite. |
| `init_css_files` (:194-213) | :260-280 | Upstream order: `pygments.css`@200 → `pygments_dark.css`@200 (if the theme has a dark style, with `media`/`id` attributes) → `_get_style_filenames()`@200. That is `html_style` if set, else **the theme's `stylesheets`** (alabaster: `basic.css`, `alabaster.css`) → registry css → user `html_css_files`@800 default. The crate has no theme stylesheets and no attrs. | Rewrite. |
| `init_js_files` (:216-236) | :289-302 | Upstream: `documentation_options.js`, `doctools.js`, `sphinx_highlight.js` @200, then registry js, then user js @800, then `translations.js` @**500 default**. The crate matches the first three. The translation lookup is wrong: it looks only in `confdir/locale/<lang>/LC_MESSAGES/sphinx.js`. Upstream `_get_translations_js` (:191-209) checks every `locale_dirs`, then Sphinx's package locale dir, then `sys.prefix/share/sphinx/locale`. | Rewrite. |
| `add_css_file`/`add_js_file` (:239-295) | :282-310 | `posixpath.join('_static', f)` unless `'://'` in the name: matches. **Equality**: upstream `_CascadingStyleSheet.__eq__` compares `filename`, `priority` **and** `attributes` (`SPHINX/builders/html/_assets.py:40-47`). The crate compares filename only (:778-788). The crate has no attribute dict. Upstream CSS attrs are `{'rel': rel, 'type': type} \| extra` (`_assets.py:28-31`), JS attrs are arbitrary (`body`, `async`, `defer`, `type`, `id`, …). | Rewrite as Sphinx-exact `Asset { filename, priority, attributes: IndexMap<String, Option<String>> }`. |
| `has_translations` (:298-304) | `_get_translations_js` :191-209 | See above. | Rewrite. |
| `init_global_context` (:307-356) | `prepare_writing` :427-562 | Wrong or missing values: `release` should be `return_codes_re.sub('', release)` (:521). `version` should be `''` when unset. `last_updated` needs `format_date(fmt or _('%b %d, %Y'), language, local_time=not html_last_updated_use_utc)` with `SOURCE_DATE_EPOCH` support (`SPHINX/util/i18n.py:263-310`); the crate's `utils::format_date` (`src/utils.rs:532-548`) ignores the language, `SOURCE_DATE_EPOCH` and the `''` default, and uses chrono formats. `docstitle`/`shorttitle` defaults are wrong (see §1). `use_opensearch` should be a str. **`sphinx_version` is the crate's `CARGO_PKG_VERSION`; upstream is `__display_version__`, i.e. `'9.1.0'`, and it is rendered into every alabaster footer.** `language` needs `convert_locale_to_language_tag` (`en_US`→`en-US`, :98-106). `logo_url`/`favicon_url` need `os.path.basename` unless a URL (:484-490). Missing keys: `show_search_summary`, `sphinx_version_tuple`, `docutils_version_info`, `rellinks`, `logo_alt` (`_('Logo of %s') % project`), `theme_*` (theme options flattened, :553-557), and `html_context` merged last (:558). | Rewrite. |
| `write_doc` (:359-383) | :650-665 | The crate escapes `Document.content` (raw source) as the body. Upstream walks the resolved doctree with `HTML5Translator`: `body = ''.join(visitor.fragment)`, `metatags = ''.join(visitor.meta[2:])`, and adds `ctx['has_maths_elements']`. `imgpath`/`dlpath` are computed via `utils::relative_uri(docname, "_images", ".html")`, which yields **`_images.html`** (it appends the link suffix to a directory). Upstream: `relative_uri(get_target_uri(docname), '_images')` → `_images` or `../_images`. `secnumbers` are never set. Upstream sets them from `env.toc_secnumbers[docname]` plus `fignumbers`; the crate env has both (`src/env/mod.rs:177-179`). | Rewrite. The translator is a separate work item. |
| `get_doc_context` (:386-467) | :564-642 | Upstream `prev`/`next`/`parents` **titles are `render_partial(env.titles[doc])['title']`** (HTML); the crate uses docnames (TODOs :411,445,454). Upstream `parents.pop()` removes the root doc before `reverse()` (:600-604); the crate keeps it. `rellinks` is missing: upstream copies `globalcontext['rellinks']`, then appends `(next, title, 'N', _('next'))` and `(prev, title, 'P', _('previous'))`, **next before prev** (:571-588). `title` should be `render_partial(env.longtitles[docname])['title']`, not the docname. `sourcename` should be `docname + source_suffix + (html_sourcelink_suffix if different)`, e.g. `intro.rst.txt`; the crate gives `intro.txt`. `page_source_suffix` is hardcoded to `.rst`. `display_toc` should be `env.toc_num_entries[docname] > 1`, not `true`. `toc` should be `render_partial(document_toc(env, docname, tags))['fragment']`, not an empty div. `meta` (`env.metadata.get(docname)`) is missing. | Rewrite. The shape of the returned map is a useful checklist. |
| `generate_local_toc` (:470-473) | `document_toc` + `render_partial` | Stub. | Delete. |
| `handle_page` (:476-517) | :1070-1257 | See §4.6 for the upstream context assembly. The crate does not merge `global_context` into non-doc pages (genindex/domainindex get **only** their own 3 keys, :554-585). It sets no `pagename`, `encoding`, `pageurl`, `pathto`, `hasdoc`, `toctree`, `sidebars`, `content_root`, `css_tag`, or `js_tag`, and does no priority sort of css/js (:1197-1206). The source copy guesses `{pagename}.rst`; upstream copies `env.doc2path(pagename)` to `_sources/<sourcename>` (:1251-1257). | Rewrite. |
| `get_output_path` (:520-522) | :1037-1038 | Matches for `html`. | Keep the logic. |
| `get_relative_uri` (:525-527) | `Builder.get_relative_uri` = `relative_uri(get_target_uri(from), get_target_uri(to))` | `utils::relative_uri` (`src/utils.rs:563-581`) uses `pathdiff` filesystem semantics plus a suffix append. Same-doc gives `index.html`; upstream gives `''` (and `pathto` then yields `'#'`). Directories gain `.html`. `#fragment` handling is missing: upstream **drops** the `#…` part of both (`SPHINX/util/osutil.py:46-66`). | Replace with a port of `osutil.relative_uri` (21 lines). Drop `pathdiff`. |
| `get_target_uri` (:530-532) | :1067-1068 | Upstream is `quote(docname) + link_suffix` (`urllib.parse.quote`, safe=`/`, UTF-8 percent-encoding). The crate does not quote. | Fix. `percent-encoding` is already in the lockfile via ureq; a hand-rolled version is also fine. |
| `gen_indices` (:535-547) | :685-692 | Structure matches. | Keep the shape. |
| `write_genindex` (:550-568) | :722-750 | Empty data. Upstream: `genindex = IndexEntries(env).create_index(self)`, `indexcounts = [sum(1 + len(subitems) …)]`, and `html_split_index` produces `genindex` (split), `genindex-all`, and `genindex-<KEY>` pages. The crate's data already exists: `src/env/genindex.rs:355` `create_index(env, rel_uri, messages) -> Vec<IndexGroup>` maps 1:1 onto the template shape (§4.6). | Rewrite. The data layer is done. |
| `write_domain_indices` (:571-590) | :752-760, and `prepare_writing` :446-467 | `domain_indices` is never populated. Upstream builds it in `prepare_writing` from `html_domain_indices` (bool or name list) × `env.domains.sorted()` × `domain.indices`, keeping those with non-empty `content`. Its template context is `{indextitle: index_cls.localname, content, collapse_index}`, where `content` is a list of `(letter, [IndexEntry(name, subtype, docname, anchor, extra, qualifier, descr)])`. The crate's `DomainIndex`/`IndexEntry` shapes are wrong. `src/env/py_domain.rs:544-566` (`ModindexEntry`, `ModindexGroup`, `PyModindex{groups, collapse}`) is exactly right. | Delete the crate types and use `PyModindex`. |
| `copy_static_files` (:593-616) | :913-933 | Upstream order: create `_static` → `create_pygments_style_file` → `copy_translation_js` → `copy_stemmer_js` → `copy_theme_static_files(context)` (theme dirs **reversed**, i.e. basic first then the child; `_t`/`.jinja` rendered with `globalcontext` + `indexer.context_for_searchtool()`, excluding dotfiles) → `copy_static_dirs` (extensions) → `copy_html_static_files(context)` (`html_static_path` resolved against **confdir**, excluded `exclude_patterns + '**/.*'`, also template-rendered) → `copy_html_logo` → `copy_html_favicon`. The crate has a no-op theme copy and a 2-line fake pygments.css. | Rewrite. |
| `copy_image_files` (:647-670) | :762-784 | Close in shape (`images: src→dest` under `_images`). Upstream's map is `self.images`, filled by `Builder.post_process_images` (candidate choice by `supported_image_types`, unique basenames from `env.images` `FilenameUniqDict`). **The crate env has no `images`/`candidates` registry** (`src/env/dependencies.rs:31-32` says candidates are unresolved). Warning text upstream is `cannot copy image file '%s': %s`. | Keep the loop shape. The input needs new env data. |
| `copy_download_files` (:673-696) | :786-810 | Upstream uses `env.dlfiles` (`src → (docnames, unique_dest)`), writes to `_downloads/<hash>/<name>`, and warns `cannot copy downloadable file %r: %s`. The crate env has no dlfiles. | Same as images. |
| `dump_search_index` (:699-728) | :1272-1288 | Writes pretty JSON `{docnames: [], …}` with no `Search.setIndex(` prefix. Upstream writes compact sorted JSON inside `Search.setIndex(…)` via a `.tmp` + rename (§3.5). | Rewrite, or omit until M3. |
| `write_build_info` (:731-752) | `write_buildinfo` :950-954 plus `BuildInfo.dump` (`SPHINX/builders/html/_build_info.py:69-77`) | **Wrong format.** The upstream file is 4 lines of text with md5 hashes (§5.3). | Rewrite. The algorithm is in §5.3. |
| `finish` (:759-775) | :675-683 plus `handle_finish` :1264-1266 | The order differs: static copy belongs **before** documents (in `copy_assets`), not in finish. No `gen_additional_pages` (search.html, opensearch.xml, `html_additional_pages`). No image copy. No inventory dump (removed in wave 4, :754-758). | Rewrite. |

**Salvage summary for `html_builder.rs`:** keep the file and type name if that helps continuity, and
the method list as a porting checklist, the `INVENTORY_FILENAME` constant and the builder constants.
Everything else is a rewrite. Make it **synchronous**: the write phase already runs on rayon
(`builder.rs:1459`), Sphinx's own write is sequential per doc, and tokio buys nothing here. The
struct should hold `Arc<minijinja::Environment<'static>>` (which is `Send + Sync`), the global
context as a `BTreeMap`/`IndexMap<String, minijinja::Value>`, `relations` taken from the env
(the crate already computes Sphinx relations), the asset lists, and a checksum memo.

---

## 3. The other modules

### 3.1 `src/template.rs`: API and defects

API: `TemplateEngine { env: Environment<'static>, template_dirs: Vec<PathBuf>, global_context: HashMap<String, Value> }`,
`new(&BuildConfig)`, `render(name, &serde_json::Map) -> Result<String>`,
`set_global_context`, `update_global_context`, `newest_template_mtime`, `newest_template_name`, and the
`TemplateContext` builder (`insert<T: Serialize>`, `extend`, `build`).

Defects, each confirmed:

1. **Autoescape is left at minijinja's default.** `Environment::new()` uses
   `default_auto_escape_callback` (`MJ/defaults.rs:31-45`), which is **HTML autoescape for `.html`,
   `.htm`, `.xml`**. Sphinx's `SandboxedEnvironment` has `autoescape=False` (confirmed). Rendering
   `page.html` the way `template.rs` does it turns `{{ body }}` into `&lt;p&gt;hi &amp;amp; bye&lt;&#x2f;p&gt;`:
   double-escaped, because `html_builder.rs:367-370` already escaped it (probe `crate_tpl_probe.py`).
   Required: `env.set_auto_escape_callback(|_| AutoEscape::None)`.
2. **User templates are never loaded.** `load_templates_from_dir` (:56-76) reads files and discards
   them ("lifetime issues"). With the `loader` feature already enabled, `add_template_owned`
   (`MJ/environment.rs:198`) and `set_loader` (:234) solve this. The built-in dir is the CWD-relative
   `"templates"` (:29).
3. **`pathto`** (:116-148) takes `&[Value]`. Kwargs arrive as a trailing kwargs map, so
   `pathto(x, resource=true)` silently becomes non-resource. It ignores page depth (always
   `x.html`) and never quotes. Upstream `pathto` is per page (`__init__.py:1095-1108`, §4.6).
4. **`css_tag`/`js_tag`** (:151-192) accept only strings. `html_builder` passes serialized
   `CSSFile` maps, so **no `<link>` or `<script>` is ever emitted**. They also lack the attribute
   rendering and `?v=` checksums (§4.6).
5. **`toctree()`** (:195-201) returns a constant empty div.
6. **`e` override** (:204-210) uses `html_escape::encode_text`, which escapes only `& < >`. It does
   not honour safe strings, so it double-escapes. markupsafe escapes `& < > ' "` as
   `&amp; &lt; &gt; &#39; &#34;` (verified: `escape('<a href="x">\'&/</a>')` gives
   `&lt;a href=&#34;x&#34;&gt;&#39;&amp;/&lt;/a&gt;`).
7. **`striptags`** (:213-224) is a regex `<[^>]*>` with no comment handling, no whitespace collapse
   and no entity unescape. It also compiles the regex on every call.
8. **`json_to_value`** (:252-279) maps JSON `Null` to `Value::UNDEFINED`. Sphinx passes Python
   `None` (`pageurl`, `last_updated`, `language`, `prev`/`next`), and templates test
   `language is not none` (`basic/layout.html:94`). Undefined **is not none**, so `lang=""`
   would render where Sphinx omits the attribute. Objects become `HashMap` then `from_serialize`,
   which sorts keys and loses Python dict insertion order.
9. `newest_template_mtime`/`name` (:292-333) scan only the top level of each dir and do not filter
   `.html`. Upstream `_newest_template_mtime_name` walks `os.walk` over `pathchain` with `.html`
   only (`SPHINX/jinja2glue.py:227-234`). Its consumer is `get_outdated_docs` (:360-376).
10. No `_`/`gettext`/`ngettext`, no `trans`, no `tobool`/`toint`/`todim`/`slice_index`,
    `accesskey`, `idgen`, `warning`, or `debug` (upstream globals and filters: `jinja2glue.py:200-213`).

Verdict: **rewrite.** None of the code is reusable. The replacement is specified in §4.

### 3.2 `templates/*`: per file

All are handwritten approximations. None is a Sphinx template, and each diverges structurally.

* `layout.html` (120 lines): RTD-flavoured. `<html class="writer-html5">`, a
  `<div class="sidebar">` layout, invented `Related Topics` and `Navigation` lists, a
  "Built with Sphinx using a custom Rust builder version" footer, `pathto(favicon_url, resource=true)`.
  Compare `basic/layout.html` (209 lines). It has no relbar, no `rellinks`, no `hasdoc`, no
  `content_root`, and no sidebars loop.
* `page.html`: `{% block body %}{{ body }}{% endblock %}`. Upstream page.html is 5 lines with
  **2-space indentation**: `{% block body %}\n  {{ body }}\n{% endblock %}`. The whitespace is
  byte-significant.
* `genindex.html`, `genindex-single.html`, `genindex-split.html`: fail at render with
  `unknown function: _ is unknown`. Beyond that, they use `count.append(count.pop() + 1)`, a Python
  list method that minijinja lacks without pycompat, and a different column layout (no
  `slice_index(2)`).
* `search.html`: fails on `_`. Uses `|tojson`, which is **not compiled in** (minijinja `json`
  feature off, `Cargo.toml` enables only `loader`). Uses jQuery `$('#fallback').hide()`.
* `domainindex.html`: a different table layout and inline JS. Renders, but is wrong.
* `opensearch.xml`: a different namespace set and URL template.
* `base.hbs` (286 lines) and `document.hbs` (`{{> base}}`): Handlebars, with **zero references**
  since `handlebars` was dropped (`docs/IMPLEMENTATION_STATUS.md:112`).

Verdict: **delete all ten files.** Vendor, verbatim:

* From `SPHINX/themes/basic/`: `layout.html`, `page.html`, `genindex.html`, `genindex-single.html`,
  `genindex-split.html`, `domainindex.html`, `search.html`, `opensearch.xml`, `localtoc.html`,
  `globaltoc.html`, `relations.html`, `sourcelink.html`, `searchbox.html`, `searchfield.html`,
  `defindex.html` (deprecated), `changes/*` (only for the `changes` builder), `theme.toml`, and `static/*`.
* From `ALABASTER`: `layout.html`, `about.html`, `navigation.html`, `relations.html`, `donate.html`,
  `theme.conf`, and `static/{alabaster.css_t, custom.css, github-banner.svg}`.

Licenses: Sphinx is BSD-2-Clause (`sphinx-9.1.0.dist-info/licenses/LICENSE.rst`), alabaster is
BSD-3 (`alabaster-1.0.0.dist-info/LICENSE.rst`). A THIRD-PARTY-NOTICES entry is needed.

### 3.3 `static/*`

| File | What it is | Upstream counterpart |
|---|---|---|
| `jquery.js` (61 lines) | A fake `$` built on `querySelectorAll` | None. Sphinx ≥6 ships no jQuery. `sphinxcontrib-jquery` is separate. |
| `doctools.js` (34) | Calls `$(document).ready`, logs to console | `basic/static/doctools.js` (4332 bytes) |
| `sphinx_highlight.js` (33) | Custom | `basic/static/sphinx_highlight.js` (5325 bytes) |
| `pygments.css` (75) | Body font CSS plus an old default-style table | Generated: `PygmentsBridge('html', style).get_stylesheet()` (`__init__.py:812-821`). Alabaster: style `alabaster.support.Alabaster`; basic: `none` |
| `theme.css` (92) | `.wy-*`/`.rst-content` RTD CSS | None. Alabaster has `alabaster.css` (from `alabaster.css_t`) + `basic.css` (from `basic.css.jinja`) + `custom.css` |

Verdict: **delete all five.** They are referenced only by `builder.rs:1617-1632` and by
`base.hbs`. Page bytes depend on the exact static bytes: every `<link>`/`<script>` for a local
asset gets `?v=%08x` of `zlib.crc32(file_bytes with b'\r' removed)`, and nothing if the file is
missing or empty (`SPHINX/builders/html/_assets.py:111-139`). Example head from a real
alabaster build:

```
    <link rel="stylesheet" type="text/css" href="_static/pygments.css?v=5ecbeea2" />
    <link rel="stylesheet" type="text/css" href="_static/basic.css?v=b08954a9" />
    <link rel="stylesheet" type="text/css" href="_static/alabaster.css?v=27fed22d" />
    <script src="_static/documentation_options.js?v=292eb321"></script>
    <script src="_static/doctools.js?v=fd6eb6e6"></script>
    <script src="_static/sphinx_highlight.js?v=6ffebe34"></script>
```

So `documentation_options.js`, which is templated with `release`, `language` and so on, and
`basic.css` and `alabaster.css`, which are templated with `theme_*` options, must be rendered
byte-exactly **before** pages are written. The CRC32 is available as `flate2::Crc` /
`crc32fast` (already in the lockfile). The full `_static` of a default alabaster build is:
`alabaster.css basic.css custom.css doctools.js documentation_options.js file.png github-banner.svg
language_data.js minus.png plus.png pygments.css searchtools.js sphinx_highlight.js base-stemmer.js english-stemmer.js`.
The last two are copied from `SPHINX/search/non-minified-js/` by `copy_stemmer_js`
(`__init__.py:832-849`). `language_data.js` embeds the **minified** stemmer JS and the stopword list
(`SPHINX/search/__init__.py:552-590`).

### 3.4 `src/search.rs`

API: `SearchIndex { docnames, filenames, titles: Vec<String>, terms: HashMap<String, Vec<DocumentMatch>>, objects: HashMap<String, ObjectReference>, objnames, objtypes: HashMap<String,String>, language }`,
`new`, `add_document`, `add_object`, `search`, `prune`, `to_json`, `SearchIndexBuilder { add_or_update_document, remove_document, build }`.

The upstream format is `SPHINX/search/__init__.py:160-186` (`_JavaScriptIndex`) and `freeze`
(:425-462). The file is `'Search.setIndex(' + json.dumps(data, separators=(',', ':'), sort_keys=True) + ')'`,
with `ensure_ascii` on (the default), so non-ASCII becomes `\uXXXX`. Keys: `alltitles`, `docnames`,
`envversion`, `filenames`, `indexentries`, `objects`, `objnames`, `objtypes`, `terms`, `titles`, `titleterms`.
`terms`/`titleterms` values are an **int or a list of ints**. `objects` is
`{prefix: [[docidx, objtypeidx, prio, anchor_or_'-'_or_'', name], …]}`. `envversion` is
`{"sphinx":66,"sphinx.domains.c":3,…}`. A real example from the probe:

```
Search.setIndex({"alltitles":{"API":[[0,null]],…},"docnames":["api","index","intro"],"envversion":{"sphinx":66,"sphinx.domains.c":3,…},
"filenames":["api.rst","index.rst","intro.rst"],"indexentries":{"alpha":[[1,"index-0",false]],…},
"objects":{"":[[0,0,0,"-","mymod"]],"mymod":[[0,1,1,"","f"],[0,0,0,"-","sub"]]},
"objnames":{"0":["py","module","Python module"],"1":["py","function","Python function"]},
"objtypes":{"0":"py:module","1":"py:function"},"terms":{"More":1,"Some":1,"api":1,"doe":0,"entri":1,…},
"titles":["API","Test “Project” & <Docs>","Introduction"],"titleterms":{"api":0,…}})
```

English stemming is `snowballstemmer.stemmer('english')` (Porter2) with stopwords
(`SPHINX/search/en.py:11-22`). Crate defects:

* The whole schema differs: terms map to `DocumentMatch` objects.
* `HashMap` ordering is nondeterministic. It serializes stably only because upstream uses `sort_keys`.
* `normalize_english` has 3 suffix rules.
* `add_object` (:62-91) pushes a docname **without** pushing filename or title, which desyncs the
  parallel vectors, and fills `objtypes` with `type → type`.
* `title_score` is always 0.
* `search()`/`generate_excerpt` are client-side concerns that Sphinx implements in `searchtools.js`.

Verdict: nothing to salvage beyond the idea of `prune(keep)` (upstream
`IndexBuilder.prune`/`load_indexer` :989-1010 does incremental index maintenance). **Delete when
the `IndexBuilder` port begins.** If wave 5 does not emit `searchindex.js`, the output tree differs
by that one file. `search.html` still references it (`basic/search.html:10`), so the page
bytes are unaffected.

### 3.5 `src/inventory.rs` writer: keep, and here is the input contract

`pub async fn dump<P: AsRef<Path>>(path: P, project: &str, version: &str, domains: &[(&str, Vec<InvObject>)], get_target_uri: impl Fn(&str) -> String) -> Result<()>`
(`src/inventory.rs:347-432`). `InvObject { name, objtype, priority: i32, docname, anchor, dispname }` (:103-111).

It checks out against `SPHINX/util/inventory.py:174-207`:

* The header escapes with `re.sub(r'\s+', ' ', …)` (:437-439).
* Domains are sorted by name. Upstream sorts with `_DomainsContainer.sorted()` =
  `sorted(self._domain_instances.items())` (`SPHINX/domains/_domains_container.py:284-286`).
* Objects are sorted by the tuple `(name, dispname, objtype, docname, anchor, prio)`, which is
  `sorted(domain.get_objects())`. Rust `String` byte order equals Python code-point order for valid UTF-8.
* The `$` anchor suffix, the `#` only for a non-empty anchor, and the `-` dispname all match.
* Level-9 zlib.

`tests/inventory_roundtrip.rs:373-440` verifies the header bytes plus the decompressed lines against
real `sphinx-build` inventories.

Caveat: **the compressed bytes are not byte-identical** to CPython's zlib. flate2 1.1.10 resolves
with `miniz_oxide` and `zlib-rs` in the lock, and the module notes this at :410-415. The HTML
differential oracle must compare `objects.inv` as header plus decompressed payload, not raw
bytes. (Linking the same system zlib that CPython uses, 1.3 here, via flate2's `zlib` feature
*might* reproduce bytes, but that was not verified. Treat it as out of scope.)

What the wave-5 finish task must supply (new code):

1. `version = config.version`, not `release`. The probe header shows `# Version: 1.0` with `release='1.0.1'`.
2. `get_target_uri = |d| quote(d) + link_suffix` (§2.2).
3. `domains = [("py", py_objects), ("std", std_objects)]` for the domains wave 5 has. Every other
   builtin domain's `get_objects` is empty or M5-only (math returns `[]`, `SPHINX/domains/math.py:153`).
   * **std** (`SPHINX/domains/std/__init__.py:1332-1357`), in yield order (the order is irrelevant because `dump` sorts):
     * for each `doc in env.all_docs`: `(doc, clean_astext(env.titles[doc]), 'doc', doc, '', -1)`.
       `clean_astext` exists at `src/env/numbers.rs:514`.
     * `progoptions`: `(prog.option or option, same, 'cmdoption', docname, anchor, 1)`.
     * `objects[(type, name)]`: `(name, name, type, docname, anchor, searchprio)`, with searchprio
       `term`:-1, `token`:-1, `label`:-1, `doc`:-1, and `confval`/`envvar`/`cmdoption`:1 (default)
       (`std/__init__.py:729-736`). Note that the crate keeps glossary terms in a separate
       `StdDomainData.terms` map (`src/env/std_domain.rs:38`). Check that its keys and values give the
       original-case term name that upstream stores in `objects[('term', name)]`.
     * `labels`: `(name, sectionname, 'label', docname, labelid, -1)`, including the builtin labels
       `genindex`, `modindex`, `py-modindex`, `search` (`src/env/std_domain.rs:49-52`). Real output:
       `genindex std:label -1 genindex.html Index`, `modindex std:label -1 py-modindex.html Module Index`,
       `search std:label -1 search.html Search Page`.
     * `anonlabels` not in `labels`: `(name, name, 'label', docname, labelid, -1)`.
   * **py** (`SPHINX/domains/python/__init__.py:1056-1065`): modules
     `(modname, modname, 'module', docname, node_id, 0)`, then objects with `objtype != 'module'`
     `(refname, refname, objtype, docname, node_id, -1 if aliased else 1)`. `PyDomainData.modules`
     and `objects` (`src/env/py_domain.rs:74-82`) carry exactly this.
4. Real probe output, to use as a unit-test vector:
   ```
   # Sphinx inventory version 2
   # Project: Probe & "Q"
   # Version: 1.0
   # The remainder of this file is compressed using zlib.
   mymod py:module 0 api.html#module-$ -
   mymod.f py:function 1 api.html#$ -
   mymod.sub py:module 0 api.html#module-$ -
   api std:doc -1 api.html API
   genindex std:label -1 genindex.html Index
   index std:doc -1 index.html Test “Project” & <Docs>
   intro std:doc -1 intro.html Introduction
   modindex std:label -1 py-modindex.html Module Index
   py-modindex std:label -1 py-modindex.html Python Module Index
   search std:label -1 search.html Search Page
   ```
5. Optional: make `dump` synchronous (`std::fs::write`), or keep it async since `SphinxBuilder::build`
   is async. Either works.

### 3.6 `src/directives.rs`, the HTML processor registry

`DirectiveProcessor` trait (`process(&Directive) -> String`, `get_name`, `get_option_spec`).
`DirectiveRegistry` registers ~40 processors at :86-147:

* Admonitions emit `<div class="admonition X"><p class="admonition-title">…</p>…</div>`. That is not
  Sphinx's HTML5 output, which is `<div class="admonition note">\n<p class="admonition-title">Note</p>\n…`.
* code-block emits `<div class="highlight-X"><pre><code class="language-X">`. Sphinx emits
  `<div class="highlight-X notranslate"><div class="highlight"><pre>` with Pygments spans.
* literalinclude is a placeholder comment.
* 28 `stub_directive!`s emit `<!-- X directive: args -->`.

`parse_directive` (:151-179) is a line regex. `process_directive` has zero callers; the Parser
field that held the registry was removed in wave 4 (`IMPLEMENTATION_STATUS.md:72`).

Verdict: **delete** everything except `pub mod validation;`, for example by moving to
`src/directives/mod.rs` with just that line. Drop the `Directive, DirectiveRegistry` re-exports
from `lib.rs:35`, which also removes the confusing name clash with
`directives::validation::DirectiveRegistry`. The HTML writer is a doctree visitor (`HTML5Translator`
parity), so a text-level registry has no role.

### 3.7 `examples/`

`examples/constraint_validation.rs` and `examples/directive_validation.rs` use only
`sphinx_ultra::validation::*` and `directives::validation::*`. **No example touches
the write stack.** `examples/README.md` lists `api-docs/`, `multi-lang/`, `custom-theme/`,
`plugin-example/`, `benchmarks/`, none of which exist; only `basic/` does. That is stale docs, not
wave-5 work. The `IMPLEMENTATION_STATUS.md` line saying the stack "runs only from examples/ and unit tests"
is inaccurate for the write stack: only unit tests exercise it.

### 3.8 `benches/builder_benchmark.rs`

* `bench_parser` (:9-72) calls `Parser::parse(&PathBuf::from("test.rst"), …)`, which calls
  `parse_full`, which calls `utils::get_file_mtime(file_path)?` (`src/parser.rs:93`). The file does
  not exist, so the `.unwrap()` at **:69** panics. This is the known break.
* `bench_builder_small` (:74-122) builds 10 files and `bench_builder_parallel_jobs` (:124-206) builds
  100 files × jobs {1,2,4,8}, both through `SphinxBuilder::build()`. Today they time the
  read/resolve pipeline plus the escaped-text placeholder write.
* `bench_cache_performance` (:208-216) is `black_box(42)`.

Verdict: rewrite in wave 5. Write the parse fixture to a `TempDir`, keep the two builder benches
(they become meaningful once the writer is real, and should split read/resolve/write timings if
exposed), and delete the fake cache bench.

### 3.9 `Cargo.toml`

| Dependency | State | Wave-5 relevance |
|---|---|---|
| `minijinja = { version = "2.12", features = ["loader"] }` | Locked **2.24.0** (MSRV 1.70, fine for 1.85). Default features on: `builtins, debug, deserialization, macros, multi_template, adjacent_loop_items, std_collections, serde`. Off: `json` (`tojson`), `urlencode`, `loop_controls`, `preserve_order`, `custom_syntax`, `fuel`, `unicode`, `speedups` (`MJ/../Cargo.toml [features]`). | Keep `loader`. **Add `preserve_order`** so maps built from Python-ordered data (`html_context`, `html_theme_options`, alabaster `extra_nav_links.items()`) iterate in insertion order; it pulls `indexmap`, already in the registry at 2.14. Keep `loop_controls` **off**, because Sphinx does not enable `jinja2.ext.loopcontrols`. Do **not** rely on `json`/`urlencode`: their output differs from Jinja2 (§4.4). Write Jinja2-exact `tojson`/`urlencode` if a theme needs them. |
| `minijinja-contrib` | Not a dependency | Optional: its `pycompat::unknown_method_callback` gives `.items()`, `.keys()`, `.values()`, `.get()`, and string methods. **Do not** call `minijinja_contrib::add_to_environment` wholesale: its `striptags`/`truncate` are not Jinja2-exact. A 30-line own callback is an alternative. |
| **syntect** | **Absent** (pruned 2026-08, `IMPLEMENTATION_STATUS.md:112`) | ROADMAP wave 5 says "syntax highlighting via syntect with Pygments-compatible classes". syntect tokenizes with Sublime grammars, and **cannot reproduce Pygments token boundaries or classes byte-for-byte**. Under the byte-exact standard, highlighted code needs either a Pygments lexer port or an explicit oracle exemption. Flag this to the design owner. |
| `bincode 2.0.1 (serde)` | Live (env + doctree persistence) | None. |
| `flate2 1.1.10` | Live (inventory reader), plus the writer | Provides `Crc` for the `?v=` checksums (`crc32fast` is in the lock). |
| `html-escape 0.2.15` | Used by the placeholder (`builder.rs:878`), `html_builder.rs`, `template.rs`, `directives.rs` | Neither docutils' `encode` (`& < " > @` → `&amp; &lt; &quot; &gt; &#64;`) nor markupsafe `escape` matches it. Both are 10-line hand-rolls. Can be dropped with the placeholder. |
| `pathdiff 0.2` | Only in `utils::relative_uri` (wrong semantics) | Drop with the `relative_uri` port. |
| `chrono` | `utils::format_date` | Sphinx `format_date` uses babel patterns plus `SOURCE_DATE_EPOCH`. Default `html_last_updated_fmt=None` means no date, so this is low priority. |
| md5 | **Absent** | Needed for `.buildinfo` `stable_hash` (§5.3). Add `md-5` (RustCrypto) or a small inline implementation. |
| `tokio (full)` | Live (async build/copy) | The writer should not use it. |
| `criterion 0.7` (dev) | Broken bench | See §3.8. |

---

## 4. Can minijinja 2.x render Sphinx's real `basic`/`alabaster` templates?

### 4.1 Empirical answer

**Verbatim, no.** `compile_probe.py` adds each file with `env.add_template(name, src)` and gets:

```
basic/defindex.html: COMPILED OK
basic/domainindex.html: COMPILED OK
basic/genindex-single.html: ERROR SyntaxError line=26 'syntax error: unknown statement trans'
basic/genindex-split.html: COMPILED OK
basic/genindex.html: COMPILED OK
basic/globaltoc.html: COMPILED OK
basic/layout.html: ERROR SyntaxError line=26 'syntax error: block tags in macros are not allowed'
basic/localtoc.html, page.html, relations.html, searchbox.html, searchfield.html, sourcelink.html: OK
basic/opensearch.xml: ERROR SyntaxError line=4 'syntax error: unknown statement trans'
basic/search.html: ERROR SyntaxError line=20 'syntax error: unknown statement trans'
alabaster/{about,donate,layout,navigation,relations}.html: OK
static/{basic.css.jinja, documentation_options.js.jinja, language_data.js.jinja}, alabaster.css_t: OK
```

`layout.html` would also fail on `trans` (lines 115, 183, 189, 201, 204) after the block issue is fixed.

**With a load-time preprocessor (two rewrites, §4.3) plus Jinja2-exact filters and globals (§4.4), yes,
byte-for-byte.** `run_themes.sh` builds `proj/` under a real `sphinx-build` 9.1.0 with
`-D html_split_index=1 -D html_use_opensearch=https://example.org -D html_last_updated_fmt=%Y -D html_baseurl=https://example.org/`,
and re-renders every `html-page-context` with minijinja 2.24 (`pycompat=False`, autoescape off).
The same Python helper objects are passed in (`pathto`, `css_tag`, `js_tag`, `hasdoc`, `toctree`
closures, css/js asset objects). Results:

* Themes: alabaster, basic, classic, nature, agogo, pyramid, sphinxdoc, haiku, scrolls, bizstyle,
  traditional, nonav.
* Pages per theme: index, intro, api, genindex, genindex-all, genindex-{A,D,F,G,I,M}, py-modindex,
  search, opensearch.xml.
* Templated static files per theme: `basic.css`, `documentation_options.js`, `language_data.js`, plus
  the theme's own (`alabaster.css`, `classic.css`, `sidebar.js`, `nature.css`, `agogo.css`, `pyramid.css`, `epub.css`,
  `sphinxdoc.css`, `haiku.css`, `scrolls.css`, `bizstyle.css`, `bizstyle.js`, `traditional.css`, `nonav.css`).
* **Result: 218 files compared, 218 byte-identical, 0 different.**
* A second run added `templates_path=['_templates']` with a `layout.html` that does
  `{% extends "!layout.html" %}` and overrides `extrahead` (with `super()`), `rootrellink` (with `super()`),
  `relbaritems`, `sidebarlogo` and `footer` (with `super()`). The first three are blocks that live
  **inside** the `relbar()`/`sidebar()` macros upstream. It also used a custom `html_sidebars` template
  with `toctree(maxdepth=1, titles_only=True)` and `namespace()`. 18/18 pages were identical on basic,
  alabaster and classic.
* The only difference seen during development was a probe artifact. The Python binding hands an
  *undefined* value to a Python filter as `None`, so `{{ link|e }}` (`link` is never defined in
  Sphinx's context, `layout.html:32`) printed `None`. A Rust `e` filter receives `Value::UNDEFINED`
  and must return `''` for undefined and `'None'` for none, as markupsafe's `escape(Undefined)` = `''`
  and `escape(None)` = `'None'` do.

Scope limit: this proves the **template engine layer**. The helper functions (`toctree`, `pathto`,
`css_tag`, …) were Sphinx's own Python closures, so the Rust ports of those functions and of the
context values are separate work items (§4.6). The body HTML comes from the translator.

### 4.2 Construct inventory and minijinja support

Constructs were collected with Jinja2's own parser (`ast_scan.py`) over `basic/` (excluding `changes/`),
`alabaster/`, and the 11 other builtin themes.

**Statements.**

| Construct | Where | minijinja 2.24 |
|---|---|---|
| `{% extends "layout.html" %}` / `"basic/layout.html"` / `"!layout.html"` | every page template; alabaster `layout.html:1` | Yes. The name is an expression passed to the loader, which implements the chain and `!` (§4.5). |
| `{% extends %}` **after** a macro definition | `genindex-single.html:2-21` | Yes (compiled and rendered identically). The macro stays visible, because `LoadBlocks` lets the child's top level keep executing with output discarded (`MJ/vm/mod.rs:714-747`). |
| `{% block %}…{% endblock %}`, nested blocks, `super()` | everywhere; `super()` ×6 basic, ×2 alabaster | Yes. `super()` compiles to `FastSuper` (`MJ/compiler/codegen.rs:568-571`). |
| **`{% block %}` inside `{% macro %}`** | `basic/layout.html:26,33` (`rootrellink`, `relbaritems` in `relbar()`); `:42,56,59,62,65,70` (`sidebarlogo`, `sidebartoc`, `sidebarrel`, `sidebarsourcelink`, `sidebarsearch`, `sidebarextra` in `sidebar()`); `agogo/layout.html:28,32` | **No.** Parse error `block tags in macros are not allowed` (`MJ/compiler/parser.rs:1087-1090`). Macros also run in a `State` with **empty `blocks`** (`MJ/vm/mod.rs:141`), so `self.x()` inside a macro cannot work either. Requires the rewrite in §4.3.2. |
| `{% macro name(args) %}` / call | `relbar`, `sidebar`, `script`, `css`, `copyright_block`, `indexentries(firstname, links)`, alabaster `rellink_markup`, haiku `nav`, agogo `agogo_sidebar` | Yes (feature `macros`). `{{- rellink_markup () }}` with a space before the parens works. |
| `{% set x = … %}` at top level in a child template, read by the parent | `genindex*.html:3` `set title = _('Index')`, `domainindex.html:3`, `search.html:3` | Yes. Same Jinja2 semantics: child top-level executes before the parent root (`MJ/vm/mod.rs:714-735`). |
| `{% set %}` inside a macro, `{% set ns.n = … %}` | `layout.html:177-179`; custom template | Yes (`namespace()` is a builtin global). |
| `{% for a, (b, c, _) in x %}` (nested unpack) | `genindex.html:40`, `domainindex.html:30` (7-tuple over 2 lines) | Yes (`parse_assignment` recursion, `parser.rs:919-956`; verified). |
| `{% for x in y if cond %}` | `genindex.html:38`, `genindex-single.html:29` | Yes (`filter_expr`, `parser.rs:963`; verified). |
| `{%- include sidebartemplate %}` (dynamic name) | `layout.html:52` | Yes. Included templates see the active context (`MJ/syntax.rs` include docs). |
| `{% trans [trimmed] k=expr, … %}…{{ k }}…{% endtrans %}` | `layout.html:115,183-185,189-191,201,204`; `search.html:20-21,28-29`; `genindex-single.html:26`; `opensearch.xml:4` | **No.** `unknown statement trans` (`parser.rs:855-898` has no `trans` arm). Requires the rewrite in §4.3.1. |
| Whitespace control `{%- -%}`, `{{- -}}`, `{#- #}` | everywhere | Yes (verified: `a  {#- c #}  b` gives `a  b` in both engines). |
| `{% do %}`, `{% break %}`/`{% continue %}` | not used by builtin themes | `do` is native in minijinja (Sphinx does not enable `jinja2.ext.do`, so minijinja is a superset). `break`/`continue` need the `loop_controls` feature; leave it off to match Sphinx. |
| `{% trans %}…{% pluralize %}` | not used by builtin themes | Needs support in the preprocessor, or an explicit error. |

**Filters.** Counts are basic / alabaster / others.

| Filter | Uses | minijinja core | Action |
|---|---|---|---|
| `e` / `escape` | 46 / 6 / 30 | Present, but **different escaping**: `&quot; &#x27; &#x2f;` (`MJ/utils.rs:329-337`). markupsafe uses `&#34; &#39;` and does not escape `/`. Verified: minijinja `a&quot;b&#x27;c&#x2f;d…`, Jinja2 `a&#34;b&#39;c/d…`. | **Override** with markupsafe-exact escape: pass safe strings through unchanged, undefined → `''`, none → `'None'`, other values via their Python `str()` form. |
| `striptags` | 4 / 0 / 4 | **Absent** from core (`MJ/defaults.rs:65-138`). The contrib version differs. | Custom. markupsafe 3.0.3 algorithm: remove `<!--…-->` pairs (stop at the first unterminated one), remove `<…>` pairs (stop at the first `<` without a following `>`), `' '.join(value.split())` with **Python** whitespace (which includes U+001C..U+001F, unlike Rust `char::is_whitespace`), then `html.unescape` (the full HTML5 entity table plus numeric refs; unknown entities stay). Examples: `'x &#64; y'`→`'x @ y'`, `'a < b'`→`'a < b'`, `'A &amp;amp; B'`→`'A &amp; B'`, `'a b'`→`'a b'`, `'a&nbsp;b'`→`'a\xa0b'`. Titles contain `&#64;` for `@` because docutils `encode` escapes `@`. |
| `safe` | 3 / 0 / 0 | Present | OK (`" &#8212; "|safe + docstitle|e` verified identical). |
| `lower` | 1 / 13 / 0 | Present; bool gives `true` (verified same as Jinja2) | OK. |
| `attr("filename")` | 1 | Present. **Semantic difference**: on a *map* minijinja returns the item, while Jinja2's `attr` does `getattr` and a dict has no such attribute, so Undefined (verified). | Represent css/js assets as `Object`s exposing `filename`. Then both engines agree. |
| `slice(2)` | 1 (`genindex-single.html:29`) | Present, same distribution (`[1,2,3][4,5]`; `[1][]` verified) | OK. |
| `tobool`, `toint`, `todim`, `slice_index` | 3+2+26 / 3+19 / 1 | Absent (Sphinx-specific, `jinja2glue.py:33-83`) | Port. `_tobool`: str → `lower() in {'true','1','yes','on'}`, else `bool()`. `_toint`: `int(val)` or 0 on ValueError (note Python `int(' 3 ')`==3 and `int('3.0')` raises). `_todim`: None→`'initial'`, all-digit `str(val)` → `'0'` or `'%spx'`, else unchanged. `_slice_index(values, slices)`: column split counting `1+len(subitems)`. |
| `reverse` | others 2 | Present | OK. |
| `tojson` | not in builtin themes (the crate's own `search.html` used it) | Behind the `json` feature, and **format differs**: `serde_json` compact, no `sort_keys`, no `ensure_ascii` (`MJ/filters.rs:1136-1176`). Jinja2: `json.dumps(obj, sort_keys=True)` (policies `json.dumps_kwargs`) with `, `/`: ` separators and ASCII escapes, then `<`,`>`,`&`,`'` → `<` etc. | Custom, when a theme needs it (RTD/pydata-class themes). |
| Jinja2 builtins absent from minijinja core | n/a | `center`, `filesizeformat`, `forceescape`, `random`, `striptags`, `tojson`\*, `truncate`, `urlencode`\*, `urlize`, `wordcount`, `wordwrap`, `xmlattr` (\* feature-gated) | Out of scope for basic/alabaster. Add as themes need them. |

**Tests.** `defined` ×2, `none` ×1, `iterable` ×1, `string` ×1 (basic); `defined` ×1 (others). All
are present in minijinja (`MJ/defaults.rs:148-218`); semantics verified for `x is not defined and ' &#187;' or x`,
`copyright is iterable and copyright is not string`, and `language is not none`. Jinja2's `callable`
test is absent in minijinja; it is unused.

**Globals and functions called.**

| Name | Uses | Source | minijinja plan |
|---|---|---|---|
| `_`, `gettext` | 47 + 9 (the latter from compiled `trans`) | `jinja2.ext.i18n`, always installed because `builder._translator` is never None (`SPHINX/locale/__init__.py:109-144` returns at least `NullTranslations`; `jinja2glue.py:200-213`) | Global functions. English: identity. Other languages: the `sphinx` catalog. |
| `pathto` | 33 + 9 + 16 | per page, `__init__.py:1095-1108` | Per-page closure (§4.6). |
| `hasdoc` | 5 | per page, `:1112-1119` | Per-page closure. |
| `toctree(**kw)` | 1 + 1 + 1 | per page, `:1121` → `_get_local_toctree` :1022-1035 | Per-page closure. Kwargs follow **Python truthiness** (§4.6). |
| `css_tag`, `js_tag` | 1, 1 | per page, `:1130-1180` | Per-page closure. |
| `accesskey` | 2 (basic relbar) + 2 (others) | `jinja2glue.py:86-93` | Global function over `State::get_or_set_temp_object` (temps are `Arc<Mutex<…>>`, shared into macros: `MJ/vm/state.rs:49,399`). The first call per key returns `accesskey="K"`, later calls `''`. The second relbar of a basic page therefore has **no** accesskeys (verified byte-identical). |
| `idgen()` then `.current()` / `.next()` | domainindex | `jinja2glue.py:96-107` | An `Object` with interior counter and `call_method` for `current`, `next`, `__next__`. |
| `super` | 6 / 2 / 6 | builtin | Yes. |
| `warning`, `debug` | `warn` in deprecated `defindex.html` (an undefined name even upstream) | `jinja2glue.py:110-117`, `pformat(context)` | `warning`: log `in rendering <page><suffix>: <msg>`, return `''`. `debug`: minijinja has its own builtin `debug()`; override it or ignore. |
| `namespace`, `range`, `dict` | custom templates | Jinja2 builtins | Present in minijinja. `cycler`/`joiner`/`lipsum` are contrib-only. |
| Method calls | `.current()`, `.next()` (idgen); alabaster `navigation.html:6` `theme_extra_nav_links.items()` | Python | `.items()` needs `set_unknown_method_callback` (`MJ/environment.rs:352`), from pycompat or your own code. The idgen methods go through the Object. |

**Expressions** (all verified identical): `and`/`or` return operand values
(`' &#187;'`), `not`, `==`/`!=` including `sidebars != []` and `sidebars != None`, `+` string concat,
`~` concat with a number, `//`, `%` on numbers, subscripts `rellink[0]`, slices `links[1:]`, inline
`'true' if x else 'false'`, attribute access on maps (`parent.link`), attribute of none gives `''`,
`True`/`False`/`None` literals in either case (`parser.rs:716-718`), `loop.index/first/last`.

**Value rendering.** `None` prints `None` and bools print `True`/`False` in minijinja 2.24, as in
Python (`MJ/value/mod.rs:790-811`), and undefined prints `''`. **Sequences and maps render in
Rust-debug style** (`["a", 1]`, `MJ/value/object.rs:293-319`), where Python prints `['a', 1]`. That
only matters if a template prints a raw list or dict, e.g. `{{ copyright }}` when copyright is a
list. Basic avoids this with the iterable branch. If needed, `set_formatter` (`MJ/environment.rs:567`)
can implement Python `str()`/`repr()`.

### 4.3 The two required source rewrites (spec plus proven prototype)

Apply both **in the loader**, to every template source before minijinja compiles it: HTML pages,
sidebars, includes, and the `_t`/`.jinja` static templates. The vendored files stay verbatim, so
upgrading Sphinx or alabaster is a copy.

#### 4.3.1 `trans` → a gettext call, with Jinja2 i18n extension semantics

Jinja2 3.1.6 `jinja2/ext.py` `InternationalizationExtension`, as Sphinx configures it: old-style
gettext (`newstyle_gettext=False`), policy `ext.i18n.trimmed=False` (confirmed from
`env.policies`). Semantics to reproduce:

1. Tag args: an optional `trimmed`/`notrimmed` first, then comma-separated `name=expr` or bare `name`
   (bare means `name=name`). Expressions may contain filters (`copyright=copyright_line|e`).
2. Body: the literal text parts get `%` → `%%`. Each `{{ name }}` placeholder (simple names only;
   Jinja2 rejects anything else) becomes `%(name)s`, and a referenced name that was not declared is
   added to the variables as `name=name`.
3. `trimmed`: `re.sub(r'\s*\n\s*', ' ', msg.strip())`.
4. If **no placeholder was referenced**, replace `%%` with `%` in the msgid again.
5. Output: `gettext(msgid)`, and if the variable dict is non-empty, `% {vars}` (Python `%`-mapping
   formatting of `str(value)`). Autoescape is off, so `MarkSafeIfAutoescape` is a no-op.
6. Whitespace markers: an outer `{%-` on `trans` and an outer `-%}` on `endtrans` map to `{{-` / `-}}`
   of the emitted expression. An inner `-%}` on the `trans` tag left-strips the body; an inner `{%-`
   on `endtrans` right-strips it.

The prototype emits `{{ __trans("msgid", k=(expr), …) }}`, with `__trans(msg, **kw) = gettext(msg) % kw`
when kw is non-empty, else `gettext(msg)`. Use a reserved global name: templates may shadow `_`,
e.g. genindex loops bind `_` as a loop target (`genindex.html:40`), and Jinja2's trans calls
`gettext` by name anyway. The Rust implementation of `__trans` needs a small `%(name)s`/`%%`
formatter. Only `%(name)s` occurs.

Pitfalls a regex-based rewrite ignores; a proper implementation should tokenize:
* `{% raw %}` regions.
* `trans` inside comments.
* `{% pluralize %}`.
* Jinja2 errors for non-name placeholders.
* Nested `{% %}` tags in the body. Jinja2 forbids control structures inside trans.

#### 4.3.2 Block-bearing macros → hidden blocks called through `self.<name>()`

For every `{%L1 macro NAME() R1%}BODY{%L2 endmacro R2%}` whose `BODY` contains a `{% block`,
emit:

```
{%L1 if false %}{% block __macro_NAME R1%}BODY{%L2 endblock %}{% endif R2%}
```

Then rewrite every call `NAME()` (and `NAME ()`), where not preceded by `\w` or `.`, into
`self.__macro_NAME()`. Apply the call rewrite with the **theme-wide** set of converted names
(`relbar`, `sidebar` from basic; `agogo_sidebar` in agogo, file-local), because children call the
parent's macros: alabaster `layout.html:47` calls `{{ sidebar() }}`, and user layouts call
`relbar()`/`sidebar()`.

Why it works in minijinja:

* Blocks may be nested in blocks. A block defined under an `{% if false %}` is still registered,
  because blocks are compiled into the template's block table and the definition site only emits
  `CallBlock`.
* `self.X()` compiles to `CallBlock(X)` (`MJ/compiler/codegen.rs:583-585`, and `:813-817` inside
  expressions, via capture).
* `call_block` runs the most-derived override with `super()` available, in a fresh frame on the same
  context (`MJ/vm/mod.rs:991-1015`). That is Jinja2's non-scoped-block semantics, so overrides of
  `rootrellink`/`sidebarlogo` from child templates and `templates_path` layouts apply, which was verified.

Why it is safe for the builtin themes:

* Every block-bearing macro there has **zero parameters**.
* Macro bodies reference only context and template-level variables (`rellinks`, `reldelim1/2`,
  `render_sidebar`, `title`, …).

Known semantic deltas, all absent from the builtin themes:

* A macro *with parameters* containing blocks cannot be converted. Reject it with a clear
  error, or bind the args with `{% with %}`.
* A converted block sees the **caller's local variables** (the frame is pushed on the current
  context). A Jinja2 macro sees only its closure and the context. They differ only if a call site
  has a local that shadows a name the macro reads.
* `caller()`/`varargs`/`kwargs` inside a converted macro are not supported.

Prototype (Python, `proj/mjhook.py`; this exact code produced the 218/218 result):

```python
TRANS_RE = re.compile(r'\{%(-?)\s*trans\b(.*?)(-?)%\}(.*?)\{%(-?)\s*endtrans\s*(-?)%\}', re.S)
PLACEHOLDER_RE = re.compile(r'\{\{-?\s*([A-Za-z_][A-Za-z0-9_]*)\s*-?\}\}')
def rewrite_trans(src):
    def repl(m):
        lstrip_outer, args, rstrip_inner, body, lstrip_endinner, rstrip_outer = m.groups()
        args = args.strip(); trimmed = False
        if args.startswith('trimmed'): trimmed = True; args = args[7:].strip().lstrip(',').strip()
        elif args.startswith('notrimmed'): args = args[9:].strip().lstrip(',').strip()
        variables = {}
        for a in split_args(args):                      # top-level comma split, quote/paren aware
            if '=' in a and not a.startswith('='): k, v = a.split('=', 1); variables[k.strip()] = v.strip()
            else: variables[a] = a
        if rstrip_inner: body = body.lstrip()
        if lstrip_endinner: body = body.rstrip()
        referenced, msg = [], ''
        for i, p in enumerate(PLACEHOLDER_RE.split(body)):
            if i % 2 == 0: msg += p.replace('%', '%%')
            else: referenced.append(p); msg += '%(' + p + ')s'; variables.setdefault(p, p)
        if trimmed: msg = re.sub(r'\s*\n\s*', ' ', msg.strip())
        if not referenced: msg = msg.replace('%%', '%')
        l = '{{-' if lstrip_outer else '{{'; r = '-}}' if rstrip_outer else '}}'
        if variables:
            kw = ', '.join(f'{k}=({v})' for k, v in variables.items())
            return f'{l} __trans({json.dumps(msg)}, {kw}) {r}'
        return f'{l} __trans({json.dumps(msg)}) {r}'
    return TRANS_RE.sub(repl, src)

MACRO_RE = re.compile(r'\{%(-?)\s*macro\s+([A-Za-z_]\w*)\s*\(\s*\)\s*(-?)%\}(.*?)\{%(-?)\s*endmacro\s*(-?)%\}', re.S)
def rewrite_block_macros(src, names_out):
    def repl(m):
        l1, name, r1, body, l2, r2 = m.groups()
        if not re.search(r'\{%-?\s*block\b', body): return m.group(0)
        names_out.add(name)
        return (f'{{%{l1} if false %}}{{% block __macro_{name} {r1}%}}{body}'
                f'{{%{l2} endblock %}}{{% endif {r2}%}}')
    return MACRO_RE.sub(repl, src)

def rewrite_calls(src, names):
    for n in names:
        src = re.sub(r'(?<![\w.])' + n + r'\s*\(\s*\)', f'self.__macro_{n}()', src)
    return src

def preprocess(src):   # order: trans first (copyright_block macro contains trans), then macros
    names = set()
    src = rewrite_trans(src)
    src = rewrite_block_macros(src, names)
    return rewrite_calls(src, {'relbar', 'sidebar'} | names)
```

Environment configuration used, to translate to Rust:

```python
minijinja.Environment(loader=load /* Sphinx chain + '!' + preprocess */, pycompat=False,
    auto_escape_callback=lambda name: None,
    filters={'e': markupsafe_escape, 'escape': markupsafe_escape, 'striptags': markupsafe_striptags,
             'tobool': _tobool, 'toint': _toint, 'todim': _todim, 'slice_index': _slice_index},
    globals={'_': gettext, 'gettext': gettext, '__trans': trans, 'accesskey': accesskey, 'idgen': idgen})
```

Rust equivalents:

* `Environment::new()`
* `set_loader(move |name| …)` (`MJ/environment.rs:234`)
* `set_auto_escape_callback(|_| AutoEscape::None)` (:508)
* `add_filter`/`add_function`/`add_global` (:717-781)
* `set_unknown_method_callback` (:352) for `.items()`
* keep `UndefinedBehavior::Lenient` (the default, equivalent to Jinja2's default `Undefined`:
  printing gives `''`, iteration gives empty, attribute access on undefined errors; `MJ/utils.rs:193-225`)
* keep `keep_trailing_newline=false`, `trim_blocks=false`, `lstrip_blocks=false` (the Jinja2 defaults Sphinx uses; confirmed).

### 4.4 Divergences that need a deliberate override (checklist)

1. Autoescape off for **all** names, including `.xml` (`opensearch.xml`) and string templates.
2. `e`/`escape`: markupsafe-exact, safe-aware, undefined → `''`.
3. `striptags`: markupsafe-exact (§4.2).
4. No `json` feature. Write a Jinja2-exact `tojson` only when needed.
5. Map ordering: enable `preserve_order` and build maps with `Value::from_iter`/`IndexMap`,
   not `from_serialize(HashMap)`.
6. Python `None` must become `Value::from(())`, never `UNDEFINED`.
7. Lists/dicts printed raw: use `set_formatter` if parity is needed.
8. Method calls on dicts/strings need an unknown-method callback.
9. The minijinja `debug()` builtin differs from Sphinx's `debug` = `pformat(context)`.
10. Error texts: Sphinx wraps render exceptions as `ThemeError('An error happened in rendering the page %s.\nReason: %r')`
    (`__init__.py:1234-1238`). Message parity with minijinja errors is not achievable. Plan for
    error-shape parity (fail the build), not text parity.
11. Output write: upstream uses `Path.write_text(output, encoding=ctx['encoding'], errors='xmlcharrefreplace')`
    (`__init__.py:1247-1249`). With utf-8 there are no replacements. Text mode on **Windows translates `\n`
    to CRLF** in Sphinx's output. Decide whether the Windows oracle cares.

### 4.5 Loader contract (port of `BuiltinTemplateLoader`, `SPHINX/jinja2glue.py:161-252`)

* `pathchain = theme.get_theme_dirs()`: the theme dir, then each base theme dir, e.g.
  `[alabaster/, sphinx/themes/basic/]`.
* `loaderchain = pathchain + [p.parent for p in pathchain]`. The parent dirs make
  `"basic/layout.html"` resolve, because `sphinx/themes/` is the parent of `basic/`.
* `templates_path` entries, relative to **confdir**, are prepended to both chains.
  `templatepathlen = len(templates_path)`.
* A name starting with `!`: strip it and search only `loaders[templatepathlen:]`.
* Each loader: `search_path/template`. If the name ends with `.jinja` and that is missing, try the
  legacy `name[:-6] + '_t'` (`SphinxFileSystemLoader`, :120-158).
* Not found: `TemplateNotFound(f'{template!r} not found in {pathchain}')`.
* minijinja caches by name, and `layout.html`, `!layout.html` and `basic/layout.html` are distinct
  names, as in Jinja2.
* Theme inheritance (`theme.conf` INI vs `theme.toml`) is parsed by `SPHINX/theming.py`. Basic's
  `theme.toml`: `inherit="none"`, `stylesheets=["basic.css"]`, `sidebars=[localtoc, relations, sourcelink, searchbox]`,
  `pygments_style={default="none"}`, options `nosidebar="false"`, `sidebarwidth="230"`, … (strings).
  Alabaster `theme.conf`: `inherit = basic`, `stylesheet = basic.css, alabaster.css`,
  `sidebars = about.html, searchfield.html, navigation.html, relations.html, donate.html`,
  `pygments_style = alabaster.support.Alabaster`, and ~100 string options.
* Static templating: `copy_asset` renders `*_t`/`*.jinja` with `render_string(src, context)` and strips
  the suffix (`SPHINX/util/fileutil.py:24-34,72`). The context is `globalcontext.copy()` +
  `indexer.context_for_searchtool()`: `search_language_stemming_code`, `search_language_stop_words`
  (as `json.dumps(sorted(stopwords))`), `search_scorer_tool`, `search_word_splitter_code`
  (`SPHINX/search/__init__.py:552-563`). All four templated static files render byte-identically
  through the preprocessor (§4.1).

### 4.6 Context contract (from a real dump, alabaster, page `intro`)

`handle_page` (`__init__.py:1070-1257`) assembles the context in this order:

1. `ctx = globalcontext.copy()`
2. `pagename`, `current_page_name`
3. `encoding = html_output_encoding`
4. `pageurl = posixpath.join(html_baseurl, get_target_uri(page))` if `html_baseurl`, else None
5. `pathto`, `hasdoc`, `toctree`, `sidebars = list(_get_sidebars(page))`
6. `ctx.update(addctx)`
7. `content_root = ('../' * default_baseuri.count('/')) or './'`
8. `css_tag`, `js_tag`
9. `update_page_context`, then the `html-page-context` event. Builtin handlers that wave 5 must
   replicate natively:
   * `setup_resource_paths` (`__init__.py:1329-1347`) rewrites a non-URL `logo_url`/`favicon_url`
     to `pathto('_static/'+x, resource=True)`.
   * alabaster `update_context` adds `alabaster_version='1.0.0'`, `alabaster_version_info=(1,0,0)`,
     and maps `html_theme_options['show_powered_by']` to `show_sphinx`.
   * `sphinx.ext.mathjax.install_mathjax` is always loaded. It adds the MathJax JS only when
     `has_maths_elements` (or the assets policy is `always`) (`SPHINX/ext/mathjax.py:81-173`).
10. Stable-sort `script_files`/`css_files` by `priority`.
11. Render.

Real `intro` context (`theme_*` omitted):

```
alabaster_version='1.0.0'  alabaster_version_info=(1, 0, 0)  body=<str>  builder='html'  content_root='./'
copyright='2026, Me'  css_files=[_CascadingStyleSheet×3]  current_page_name='intro'  display_toc=True
docstitle='Probe & "Q" 1.0.1 documentation'  docutils_version_info=(0, 22, 4, 'final', 0)  embedded=False
encoding='utf-8'  favicon_url=''  file_suffix='.html'  has_maths_elements=False  has_source=True
html5_doctype=True  language='en'  last_updated=None  link_suffix='.html'  logo_alt='Logo of Probe & "Q"'
logo_url=''  master_doc='index'  meta={}  metatags='<meta name="viewport" content="width=device-width, initial-scale=1" />\n'
next={'link': 'api.html', 'title': 'API'}  page_source_suffix='.rst'  pagename='intro'  pageurl=None  parents=[]
prev={'link': 'index.html', 'title': 'Test “Project” &amp; &lt;Docs&gt;'}  project='Probe & "Q"'  release='1.0.1'
rellinks=[('genindex','General Index','I','index'), ('py-modindex','Python Module Index','','modules'),
          ('api','API','N','next'), ('index','Test “Project” &amp; &lt;Docs&gt;','P','previous')]
root_doc='index'  script_files=[_JavaScript×3]  shorttitle='Probe & "Q" 1.0.1 documentation'  show_copyright=True
show_search_summary=True  show_source=True  show_sphinx=True
sidebars=['about.html','searchfield.html','navigation.html','relations.html','donate.html']
sourcelink_suffix='.txt'  sourcename='intro.rst.txt'  sphinx_version='9.1.0'  sphinx_version_tuple=(9, 1, 0, 'final', 0)
styles=['basic.css','alabaster.css']  title='Introduction'  toc=<str>  use_opensearch=''  version='1.0'
+ callables: pathto, hasdoc, toctree, css_tag, js_tag
```

Page-specific additions:

* genindex: `genindexentries = [(key, [(name, (links:[(main:''|'main', uri)], subitems:[(subname, links)], category_key|None))])]`
  and `genindexcounts=[…]`, `split_index=False`. This maps directly from
  `src/env/genindex.rs:313-332` (`IndexGroup{group, entries: [IndexEntry{name, targets:[(main, uri)], subitems:[IndexSubItem{name, targets}], category_key}]}`).
  Build the tuples as `Value` sequences.
* domainindex: `indextitle='Python Module Index'`, `collapse_index`,
  `content=[('m', [IndexEntry(name='mymod', subtype=1, docname='api', anchor='module-mymod', extra='', qualifier='', descr='My module.'), …])]`.
  This is a NamedTuple, so provide it as an Object that supports both 7-element sequence
  unpacking and `.name`-style attributes.

Helper semantics to port exactly:

* **`pathto(otheruri, resource=False, baseuri=default_baseuri)`**. If `resource` and `'://'` in
  the uri, return it unchanged. If not `resource`: `otheruri = get_target_uri(otheruri)`
  (quote + suffix). Then `uri = relative_uri(baseuri, otheruri) or '#'`, where
  `default_baseuri = get_target_uri(pagename).rsplit('#',1)[0]`. Positional truthy `1` means
  resource (`pathto(x, 1)`). Accept `resource`/`baseuri` both as kwargs and positionally.
* **`hasdoc(name)`**: `name in env.all_docs`, or `name == 'search' and self.search`, or
  `name == 'genindex' and html_use_index`.
* **`toctree(**kw)`** → `_get_local_toctree(pagename, collapse=True default, includehidden=False default, maxdepth pop if '', titles_only)`
  → `global_toctree_for_doc` (`SPHINX/environment/adapters/toctree.py:70-120`) → `render_partial(...)['fragment']`.
  **Pitfall:** the kwargs arrive as theme-option **strings**, and are used with *Python truthiness*.
  basic's `globaltoc.html:3` passes `includehidden=theme_globaltoc_includehidden`, which is `"false"`,
  a non-empty string, so **True**. alabaster passes `collapse="true"` (True) and `includehidden="true"`.
  `maxdepth=int(maxdepth)`, so a non-numeric string raises. Do not run these through `tobool`.
* **`css_tag(css)`**: `attrs = [f'{k}="{html.escape(v, quote=True)}"' for k, v in css.attributes.items() if v is not None]`.
  `html.escape` with `quote=True` escapes `'` as `&#x27;` and `"` as `&quot;`, which is not markupsafe.
  `uri = pathto(filename, resource=True)`, plus `?v={crc32}` if `_file_checksum` is non-empty.
  The result is `<link {' '.join(sorted(attrs))} href="{uri}" />`. Attrs are **sorted**, so `rel` comes before `type`.
* **`js_tag(js)`**: if `js` is a plain str (old style), `<script src="{pathto(js,1)}"></script>`. Else `body`
  is taken from the attributes and the other attrs are sorted. With no filename:
  `<script {attrs}>{body}</script>` or `<script>{body}</script>`. With a filename, add the checksum unless
  `'MathJax.js?'` is in the name. Output: `<script {attrs} src="{uri}"></script>` or `<script src="{uri}"></script>`.
* `_file_checksum(outdir, filename)`: `''` for `://`. `ThemeError` if `?` is in the filename. Otherwise
  `crc32(read_bytes(outdir/filename).replace(b'\r', b''))` as `%08x`, and `''` if the file is missing or
  empty. The value is memoized per resolved path.

---

## 5. Byte-exactness dependencies that live outside the template engine

1. **Static asset bytes** (§3.3). Vendor the upstream files and render the `.jinja`/`_t` templates.
   `pygments.css` must be the exact `PygmentsBridge` output for the configured style and
   Pygments 2.21.0. Vendoring pre-generated CSS per supported style is the only practical route
   without a Pygments port. `pygments_style` default resolution: `config.pygments_style`, else the
   theme's `pygments_style_default`, else `'none'`, else `'sphinx'` with no theme (`__init__.py:237-258`).
2. **Version strings rendered into pages**: `sphinx_version` = `'9.1.0'` (alabaster footer
   `Powered by … Sphinx 9.1.0`), `alabaster_version` = `'1.0.0'`. Mimicking these is a product
   decision. Byte parity requires emitting Sphinx's version, not the crate's.
3. **`.buildinfo`** (`SPHINX/builders/html/_build_info.py`). Exact bytes:
   ```
   # Sphinx build info version 1
   # This file records the configuration used when building these files. When it is not found, a full rebuild will be done.
   config: <md5 hex>
   tags: <md5 hex>
   ```
   `stable_hash` (`SPHINX/util/_serialise.py:13-28`) is:
   * scalar leaf: `md5(python_str(leaf))`. `str` itself for strings (utf-8), `'None'`, `'True'`/`'False'`, decimal ints.
   * list/tuple/set: `md5(str(sorted(h(x) for x in obj)))`, where `str(list_of_hex)` is `"['h1', 'h2']"`.
   * dict: items are hashed as 2-tuples, `obj = sorted(h((k, v)))`, **and then the list branch applies
     again** (it is `if` then `if`, not `elif`), so it re-hashes the item hashes:
     `md5(str(sorted(md5(x) for x in sorted_item_hashes)))`.
   * `config` = `stable_hash({name: value for every config value whose rebuild category is 'html'})`.
     That is **every** html-category value registered by all loaded builtin builders and extensions: 61
     names for a plain project, including the `htmlhelp_*`, `qthelp_*`, `singlehtml_sidebars`,
     `mathjax*`, `modindex_common_prefix`, `pygments_style`, `templates_path`, `template_bridge`,
     `project_copyright` and `copyright` values, with computed defaults like `html_title` and
     `qthelp_basename='ProbeQ'`. The full list is in `buildinfo_probe.py` output. User extensions add more.
   * `tags` = `stable_hash(sorted(tags))`. For a plain html build the tags are `['builder_html','format_html','html']`,
     giving `645f666f9bcd5a90fca523b33c5a78b7` (hand-verified).
   * The config hash was hand-verified with a Python re-implementation over the real values
     (`35d093899274355a61200d2fa124e33a` for `proj/`).
   * Upstream also reads `.buildinfo` in `get_outdated_docs` (`__init__.py:332-357`). A mismatch
     means a full rewrite and a `.buildinfo.bak` backup with the log line
     `building [html]: build_info mismatch, copying .buildinfo to .buildinfo.bak`.
4. **`searchindex.js`**: the full IndexBuilder port (§3.4).
5. **`objects.inv`**: compare decompressed (§3.5).
6. **`metatags`**: `''.join(visitor.meta[2:])`, which includes docutils' `<meta name="viewport" …>`.
   That produces the duplicated viewport line in every Sphinx page (`layout.html:98-99`
   concatenated with `{{- metatags }}`). This is translator territory; it is noted here because it
   crosses into the template context.
7. **Directory set**: Sphinx creates `_downloads/` and `_images/` only when needed, and never copies
   `_templates/`. The crate's `html_builder.rs:174-178` and `builder.rs:1602-1609` diverge.

---

## 6. Recommended wave-5 layout (salvage plan)

1. **Delete**:
   * `templates/` (10 files) and `static/` (5 files).
   * `src/template.rs`, `src/search.rs` (or keep until M3, clearly marked), and the HTML half of `src/directives.rs`.
   * the `include_str!` block in `builder.rs:1615-1638` and the `_templates` copy.
   * `Document.html` from the read phase and the cache, or bump the cache format.

   Update the `lib.rs` re-exports and the CHANGELOG (public API).
2. **Vendor** (verbatim, plus a notices file) the upstream `themes/basic/**`, `alabaster/**`,
   `search/minified-js/{base,english}-stemmer.js` and `search/non-minified-js/{base,english}-stemmer.js`,
   plus stopwords. Add the pre-generated `pygments.css` per style. Embed with `include_bytes!`, or
   ship a data dir. Keep the upstream directory structure so the loader chain semantics hold.
3. **New `src/html/templating.rs`**, replacing `template.rs`:
   * the loader chain with `!` and `_t`/`.jinja` handling (§4.5)
   * the preprocessor (§4.3), ideally a small tokenizer rather than regex
   * markupsafe `escape`/`striptags`, `tobool`/`toint`/`todim`/`slice_index`
   * `_`/`gettext`/`ngettext`/`__trans`, `accesskey`, `idgen`, `warning`
   * a `.items()`-capable unknown-method callback
   * autoescape off, `preserve_order`

   Unit-test it against the Jinja2 outputs committed as fixtures: 12 themes × pages from `run_themes.sh`,
   with the Python helper outputs pinned.
4. **New `src/html/theme.rs`**: parse `theme.conf`/`theme.toml`, the inheritance chain,
   stylesheets, sidebars and `pygments_style`, and `get_options(html_theme_options)` flattened to `theme_*`
   (strings from the theme files; user overrides keep their types).
5. **Rewrite `html_builder.rs`** as a synchronous port of `StandaloneHTMLBuilder`:
   `prepare_writing` (global context + domain indices + rellinks + relations), `copy_assets`, `write_doc`
   (translator + `get_doc_context`), `handle_page` (§4.6), `finish` (genindex incl. split,
   `py-modindex`, search.html, opensearch.xml, `copy_image_files`, `.buildinfo`, `objects.inv` via
   `InventoryFile::dump`).
6. **Port utilities**: `osutil.relative_uri`, `urllib.parse.quote`, `_file_checksum`, `stable_hash`,
   `convert_locale_to_language_tag`, `format_date`, and Sphinx's `copyfile` semantics (no-op if identical).
7. **Config fixes**: default `html_theme='alabaster'`; `html_use_opensearch: String`;
   `html_css_files`/`html_js_files` with attrs; `copyright` as a list; `html_title`/`html_short_title`
   computed defaults; `html_file_suffix`/`html_link_suffix`, `html_baseurl`, `html_sidebars`,
   `html_additional_pages`, `html_domain_indices`, `html_split_index`, `html_extra_path`,
   `html_permalinks(_icon)`, `html_secnumber_suffix`, `html_compact_lists`, `html_output_encoding`,
   `html_scaled_image_link`, `html_codeblock_linenos_style`, `html_last_updated_use_utc`,
   `html_show_search_summary` (all 61 html-category names matter for `.buildinfo` anyway).
8. **Benchmarks**: §3.8.

---

## 7. Risks and open questions

* **Blocks in macros.** Prior research missed this (`docs/research/themes.md:54`). The rewrite
  is proven for all 12 builtin themes and for `templates_path` overrides. Third-party themes (RTD,
  furo, pydata, book) were **not** tested. Any theme with a *parameterized* macro containing blocks
  needs another strategy. The best long-term fix may be an upstream minijinja change so that
  macro states carry `blocks`; `MJ/vm/mod.rs:141` sets `blocks: BTreeMap::default()`.
* **Converted blocks see caller locals.** A theoretical semantic delta (§4.3.2).
* **Regex preprocessing** can misfire inside `{% raw %}`, comments or strings. Tokenize instead.
* **syntect vs Pygments.** Byte-exact code highlighting is not achievable with syntect. This is a
  design decision for wave 5: exempt it, or port Pygments lexers for the default highlight language.
* **`pygments.css` for arbitrary styles** needs a vendored table per style (or a Pygments style port).
  Alabaster's default style is a Python class in `alabaster/support.py`.
* **`.buildinfo` parity** requires the full list of html-category config values, including those of any
  extension a project loads. It is feasible but brittle, because unknown extensions mean an unknown hash
  set. One option is to write Sphinx-format `.buildinfo` and exclude its hash lines from the oracle.
* **`sphinx_version`** is displayed in footers. Emitting `9.1.0` from sphinx-ultra is a product decision.
* **The minijinja Python binding differs from the Rust crate**, so the evidence carries two caveats.
  The probe used Python-side values and helper callables, so it proves the template-engine layer only.
  The Rust port must still (a) produce equivalent `Value`s (None vs undefined, dict order, Objects for
  css/js/IndexEntry), and (b) implement the helpers byte-exactly. Also, minijinja-py passes undefined to
  Python filters as `None`, hence the probe's `esc` special case.
* **Compressed `objects.inv` bytes** will not match CPython zlib output. The oracle must compare decompressed.
* **Translations** (`language != 'en'`): `_()`/trans need Sphinx's `sphinx.mo` catalogs.
  Deferred, but keep `gettext` a real function hook.
* **Windows**: Sphinx's text-mode writes produce CRLF pages. Decide the oracle policy.
