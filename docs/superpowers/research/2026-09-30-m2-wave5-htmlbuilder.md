# Research note — `htmlbuilder`: Sphinx 9.1 StandaloneHTMLBuilder / DirectoryHTMLBuilder / DummyBuilder and the `basic` theme

Scope: upstream spec, part 3, for M2 wave 5 (HTML writer v1). Everything below was read from
Sphinx 9.1.0 / docutils 0.22.4 / Jinja2 3.1.6 / markupsafe 3.0.3 / alabaster 1.0.0 as installed at

```
SITE=/root/.cache/uv/archive-v0/b4dBDAdEzskuqge1iT52j/lib/python3.12/site-packages
SPHINX=$SITE/sphinx      DOCUTILS=$SITE/docutils      ALABASTER=$SITE/alabaster
```

and every behavioural claim marked **[probed]** was confirmed by running real `sphinx-build` 9.1.0
(`PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' python -m sphinx ...`)
on throwaway projects under
`/tmp/claude-0/-home-user-sphinx-ultra/46bf5e6b-694f-5b8e-ba0d-36f1851a8974/scratchpad/probe-htmlbuilder/`
(`basic/`, `feat/`, `alab/`, `misc/`, `deep/`, `uni/`, `incr/`, `ctx/`, `warn/` — their `src/` and
`out*/` trees are still there and can be re-inspected; `cfghash.py`, `cfghash2.py`, `rehash.py`,
`resolved.py` are the probe scripts).

File:line citations use `SPHINX/...` for upstream and repo-relative paths for this crate.

---------------------------------------------------------------------------------------------------

## 0. Top findings (read this first)

1. **Static assets are copied BEFORE any page is written** (`Builder.write` → `copy_assets()`,
   `SPHINX/builders/__init__.py:744-745`; `StandaloneHTMLBuilder.copy_assets`,
   `SPHINX/builders/html/__init__.py:644-648`). This is load-bearing: every `<link>`/`<script>`
   for a local asset carries `?v=<crc32>` of the *already written* `_static` file
   (`_file_checksum`, `SPHINX/builders/html/_assets.py:111-135`). The checksum is
   `format(zlib.crc32(bytes_with_all_\r_removed), '08x')`; empty or missing file ⇒ no `?v=` at all;
   URLs containing `://` ⇒ never a `?v=`.
2. **`.buildinfo` is exactly 4 lines**, config hash = `stable_hash({name: value for every config
   option whose rebuild == 'html'})`, tags hash = `stable_hash(sorted(tags))`. I re-implemented
   `stable_hash` independently and matched Sphinx on the probe projects (algorithm in §13). Gotcha
   **[probed]**: the hash is taken in `init()` *before* `init_css_files`/`init_js_files` mutate the
   `html_css_files`/`html_js_files` attribute dicts with `setdefault('priority', 800)` — hashing the
   post-mutation values gives a different (wrong) hash. The option set is 62 names for a default
   Sphinx 9.1 install and includes non-`html_*` names (`copyright`, `project_copyright`,
   `templates_path`, `template_bridge`, `pygments_style`, `modindex_common_prefix`, `mathjax_*`,
   `qthelp_*`, `htmlhelp_*_suffix`, `singlehtml_sidebars`) — full list in §13.
3. **Templates are rendered by a Jinja2 `SandboxedEnvironment` with autoescape OFF and the
   `jinja2.ext.i18n` extension ON** (`SPHINX/jinja2glue.py:200-213`). minijinja 2.24 (this crate's
   version, `Cargo.lock`) differs in ways that each break byte parity unless handled:
   (a) minijinja turns **HTML autoescape on for `*.html`/`*.xml` names by default**
   (`minijinja-2.24.0/src/defaults.rs:31-45`) — must be switched off;
   (b) minijinja's `escape`/`e` emits `&quot; &#x27; &#x2f;` (`src/utils.rs:334-336`) while
   markupsafe emits `&#34; &#39;` and leaves `/` alone — the filter must be replaced;
   (c) **minijinja has no `{% trans %}` tag** — `layout.html`, `search.html`, `genindex-single.html`
   and `opensearch.xml` use it (9 blocks, §5.3) — templates must be rewritten at vendoring time or
   pre-processed at load time; (d) `striptags` (used on every `<title>`, rellink and prev/next
   title) must be the markupsafe algorithm, which *unescapes entities* (§5.2); (e) Python-style
   `str()` of `True/False/None` vs minijinja `true/false/none`. The crate's current
   `src/template.rs:204-225` filters are wrong on (b) and (d).
4. **Every doc page, `genindex*`, `py-modindex`, `search` and (with `html_use_opensearch`)
   `_static/opensearch.xml` go through one `handle_page`** (`SPHINX/builders/html/__init__.py:1070-1257`);
   the output **never ends with a newline** (Jinja2 strips one trailing `\n` of a template;
   `keep_trailing_newline=False`), same for rendered static templates (`basic.css`,
   `documentation_options.js`, …) **[probed]**.
5. The **resolved doctree differs between the dummy and html builders** in every URI-bearing
   attribute **[probed]**: dummy `refuri="#spam"` vs html `refuri="index.html#spam"`; toctree
   references `refuri=""` vs `refuri="other.html"`. The crate hard-wires the dummy answers today
   (`src/builder.rs:1303-1307` resolver `relative_uri` returns `""`; `src/builder.rs:1256-1260`
   genindex `rel_uri` returns `Some("")`). Wave 5 must thread the html builder's
   `get_relative_uri` into both. (The claim in `tools/gen_env_fixture.py`'s docstring that the dummy
   resolution is "byte-identical to what an HTML build would resolve" is false for `refuri`.)
6. **Default theme is `alabaster`** (`SPHINX/builders/html/__init__.py:1459`); the ROADMAP puts it
   in M3. A wave-5 oracle must set `html_theme = 'basic'` explicitly in every fixture project.
   What alabaster adds is in §16.3.
7. The search stack cannot be fully skipped: `search.html` is written on every html/dirhtml build
   (`self.search = True`), and the theme's static dir always yields `searchtools.js`,
   `language_data.js` (a *rendered* template containing the English stopword list and the minified
   Snowball stemmer), plus `base-stemmer.js`/`english-stemmer.js` copies. Only `searchindex.js`
   needs the M3 indexer. Also: **Sphinx 9.1 English search uses the Snowball `english` stemmer
   (Porter2)**, `SPHINX/search/en.py:11-21`, not the classic Porter the ROADMAP (M3 search bullet)
   still describes.
8. The crate's config defaults disagree with Sphinx's for the HTML family (`src/config.rs:411-441,
   479`: `html_theme` `sphinx_rtd_theme` vs `alabaster`; `html_static_path` `['_static']` vs `[]`;
   `templates_path` `['_templates']` vs `[]`; `html_style` `['sphinx_rtd_theme.css']` vs `None`;
   `html_last_updated_fmt` `Some("%b %d, %Y")` vs `None`; `html_use_opensearch` is a `bool` but is a
   `str` (base URL) in Sphinx). `src/utils.rs:563-581` `relative_uri` is a `pathdiff` approximation,
   not Sphinx's algorithm (§9.1).

---------------------------------------------------------------------------------------------------

## 1. Build flow and which documents get written

### 1.1 `Builder.build` (`SPHINX/builders/__init__.py:388-466`)

1. `read()` (inside `logging.pending_warnings()` unless `-W`/exception-on-warning) → updated docnames.
2. `env.check_dependents` adds dependents; "looking for now-outdated files... none found / N found".
3. If anything was updated: pickle env, `env.check_consistency()` ("checking consistency... done").
   Else, for `method == 'update'` with no docnames: "no targets are out of date.".
4. `docnames &= env.found_docs` (drops removed docs).
5. `parallel_ok` only if `-j > 1`, `allow_parallel` and every extension is `parallel_write_safe`.
6. `self.finish_tasks = SerialTasks()` (finish tasks are always serial in 9.1).
7. `self.write(docnames, updated_docnames, method)`; `self.finish()`; `self.finish_tasks.join()`.

### 1.2 `Builder.write` (`SPHINX/builders/__init__.py:704-748`)

```
emit('write-started', builder)
if build_docnames is None or == ['__all__']: build_docnames = env.found_docs
docnames = set(build_docnames) | set(updated_docnames)   if method == 'update'
         = set(build_docnames)                           otherwise
docnames |= {toc for d in docnames for toc in env.files_to_rebuild.get(d, ()) if toc in env.found_docs}
env.toctree_includes = dict(sorted(env.toctree_includes.items()))
with progress_message('preparing documents'): prepare_writing(docnames)
with progress_message('copying assets', nonl=False): copy_assets()
if docnames: write_documents(docnames)       # sorted(docnames)
```

`_write_serial` (`:764-777`) loops `status_iterator(sorted docnames, 'writing output... ')` calling
`_write_docname` (`:877-890`): `env.get_and_resolve_doctree(docname, builder, tags)` →
`write_doc_serialized(docname, doctree)` → `write_doc(docname, doctree)`. Warnings raised while
writing are buffered by `pending_warnings()` and flushed **after** the last "writing output..."
line.

`_write_parallel` (`:779-818`): the first doc is written serially in the main process; the rest are
chunked (`make_chunks`, `SPHINX/util/parallel.py:156-169`: `chunksize = nargs // nproc`, if
`>= 10` then `int(sqrt(nargs / nproc * 10))`, min 1); for each chunk the **main process** runs
`get_and_resolve_doctree` + `write_doc_serialized` (image selection + search indexing), the worker
runs `write_doc`. Output bytes do not depend on this; warning order can.

### 1.3 Which docs does an incremental html build write? (`get_outdated_docs`, `SPHINX/builders/html/__init__.py:332-404`)

* Load `outdir/.buildinfo`. `ValueError` ⇒ warning `Failed to read build info file: %r`
  (repr of the exception, e.g. `ValueError('failed to read broken build info file (unknown version)')`);
  `OSError` (missing) ⇒ silently ignored (**a missing `.buildinfo` does NOT force a full rebuild**
  despite the file's own header text; only mtimes decide then).
* If loaded and `self.build_info != loaded` ⇒ move `.buildinfo` → `.buildinfo.bak`, dump the new
  one immediately, log `building [html]: build_info mismatch, copying .buildinfo to .buildinfo.bak`
  **[probed]**, and yield **all** `env.found_docs`.
* Else `template_mtime = int(newest_template_mtime() * 10**6)` (µs, over every `*.html` file in
  `templates.pathchain` — the templates_path dirs + theme dirs, walked recursively,
  `SPHINX/jinja2glue.py:221-234`); if newer than `.buildinfo`'s mtime, log
  `building [html]: template %s has been changed since the previous build, all docs will be rebuilt`.
* Yield each found doc that is not in `env.all_docs`, or whose output file
  (`get_output_path(docname)`) mtime (µs, rounded **up**, `SPHINX/util/osutil.py:74-86`) is older
  than `max(source mtime, template_mtime)`; missing output ⇒ mtime 0 ⇒ outdated.

Probed consequences (`incr/`): second run with no change writes **no** documents but still runs
every finish task (genindex, py-modindex, search, static copies, objects.inv, searchindex.js,
.buildinfo are all rewritten); touching `other.rst` rewrites `other` **and** `index` (its toctree
container via `files_to_rebuild`); changing an `html`-rebuild value (e.g. `html_title`) rewrites
every page without re-reading anything (`updating environment: 0 added, 0 changed, 0 removed`).

The crate currently writes *every* found doc every build (`src/builder.rs:1440-1470` doc comment);
that is output-equivalent as long as rendering is deterministic.

### 1.4 `StandaloneHTMLBuilder.finish` (`SPHINX/builders/html/__init__.py:675-683`) — exact task order

```
finish_tasks.add_task(gen_indices)                 # genindex (+split pages), then domain indices
finish_tasks.add_task(gen_pages_from_extensions)   # 'html-collect-pages' event
finish_tasks.add_task(gen_additional_pages)        # html_additional_pages, search, opensearch
finish_tasks.add_task(copy_image_files)            # _images/
finish_tasks.add_task(write_buildinfo)             # .buildinfo
handle_finish():                                   # :1264-1266
    finish_tasks.add_task(dump_search_index)       # searchindex.js
    finish_tasks.add_task(dump_inventory)          # objects.inv
```

### 1.5 Complete stdout of a fresh build **[probed]** (`feat/stdout.txt`, non-tty, `<P>` = probe dir)

```
Running Sphinx v9.1.0
loading translations [en]... done
making output directory... done
building [mo]: targets for 0 po files that are out of date
writing output... 
building [html]: targets for 3 source files that are out of date
updating environment: [new config] 3 added, 0 changed, 0 removed
reading sources... [ 33%] index
reading sources... [ 67%] sub/index
reading sources... [100%] sub/page

looking for now-outdated files... none found
pickling environment... done
checking consistency... done
preparing documents... done
copying assets... 
copying downloadable files... [100%] _static/print.css

copying static files... 
Writing evaluated template result to <P>/feat/out/_static/basic.css
Writing evaluated template result to <P>/feat/out/_static/documentation_options.js
Writing evaluated template result to <P>/feat/out/_static/language_data.js
Writing evaluated template result to <P>/feat/out/_static/tmpl.css
copying static files: done
copying extra files... 
copying extra files: done
copying assets: done
writing output... [ 33%] index
writing output... [ 67%] sub/index
writing output... [100%] sub/page

generating indices... genindex done
writing additional pages... search opensearch done
copying images... [100%] _static/logo.png

dumping search index in English (code: en)... done
dumping object inventory... done
build succeeded.

The HTML pages are in out.
```

(`writing output... ` after `building [mo]` is the empty catalog status_iterator. "Writing
evaluated template result to" is logged per rendered static template with its **absolute**
destination path, `SPHINX/util/fileutil.py:89-95`. `generating indices... ` prints each index name
followed by a space: `genindex py-modindex done`. The dummy builder epilog is
`The dummy builder generates no files.`; html/dirhtml: `The HTML pages are in <relpath(outdir)>.`)

---------------------------------------------------------------------------------------------------

## 2. Builder construction and `init()`

### 2.1 `Builder.__init__` (`SPHINX/builders/__init__.py:108-136`)

Tags added unconditionally: `format`, `name`, `format_<format>`, `builder_<name>`:
html → `{'html', 'format_html', 'builder_html'}`; dirhtml → `{'html', 'dirhtml', 'format_html',
'builder_dirhtml'}`; dummy (format `''`) → `{'', 'dummy', 'format_', 'builder_dummy'}`.
Also `images: dict[str,str] = {}` (src → unique dest name, filled at write time), `imagedir = ''`,
`imgpath = ''`.

### 2.2 `StandaloneHTMLBuilder` class attributes (`SPHINX/builders/html/__init__.py:109-137`)

`name='html'`, `format='html'`, `epilog='The HTML pages are in %(outdir)s.'`,
`default_translator_class=HTML5Translator`, `copysource=True`, `allow_parallel=True`,
`out_suffix='.html'`, `link_suffix='.html'`, `indexer_format=js_index`,
`indexer_dumps_unicode=True`, `html_scaled_image_link=True`,
`supported_image_types=['image/svg+xml','image/png','image/gif','image/jpeg']`,
`supported_remote_images=True`, `supported_data_uri_images=True`,
`searchindex_filename='searchindex.js'`, `add_permalinks=True`,
`allow_sharp_as_current_path=True`, `embedded=False`, `search=True`, `use_index=False`,
`download_support=True`, `imgpath=''`, `domain_indices=[]`.

`__init__` (`:139-160`): `_static_dir = outdir/'_static'`, `_sources_dir = outdir/'_sources'`,
`_downloads_dir = outdir/'_downloads'`, `_images_dir = outdir/'_images'`; `_css_files = []`,
`_js_files = []`; `_settings` = docutils settings for `render_partial` (`doctree.Reader`, `rst.Parser`,
`HTMLWriter`, defaults `{'output_encoding': 'unicode', 'traceback': True}`).

### 2.3 `init()` (`:162-186`) in order

1. `self.build_info = self.create_build_info()` → `BuildInfo(config, tags, frozenset({'html'}))`
   (`:188-189`). **Before** anything mutates config (see §13.3).
2. `imagedir = '_images'`, `secnumbers = {}`, `current_docname = ''`.
3. `init_templates()` (`:224-235`): `HTMLThemeFactory(confdir, app, config, registry)`;
   `theme = factory.create(html_theme)`; `theme_options = html_theme_options`;
   `create_template_bridge()` (`BuiltinTemplateLoader` unless `template_bridge` set);
   `templates.init(self, theme)`.
4. `init_highlighter()` (`:237-258`): style = `pygments_style` if set, else
   `theme.pygments_style_default or 'none'`; `PygmentsBridge('html', style)`. Dark highlighter
   only if the theme defines `pygments_style.dark` (basic/alabaster don't).
5. `init_css_files()` (`:260-280`): reset; `add_css_file('pygments.css', priority=200)`; if dark:
   `pygments_dark.css` (200, `media='(prefers-color-scheme: dark)'`, `id='pygments_dark_css'`);
   each of `_get_style_filenames()` at priority 200 (html_style str → [it]; list → it; None →
   `theme.stylesheets`; basic ⇒ `basic.css`); each `registry.css_files` (extensions, their own
   attrs, default priority 500); each `html_css_files` entry with `attrs.setdefault('priority', 800)`
   (**mutates the config's dicts**).
6. `init_js_files()` (`:289-303`): `documentation_options.js` (200), `doctools.js` (200),
   `sphinx_highlight.js` (200); registry js files; `html_js_files` with `setdefault('priority',
   800)`; finally `translations.js` (priority **500**) iff `_get_translations_js()` finds
   `<locale_dir>/<language>/LC_MESSAGES/sphinx.js`, `SPHINX/locale/<language>/LC_MESSAGES/sphinx.js`
   or `sys.prefix/share/sphinx/locale/<language>/sphinx.js`. **There is no `en` locale**, so English
   builds never get `translations.js`; e.g. `de` does (`SPHINX/locale/de/LC_MESSAGES/sphinx.js`).
7. `out_suffix = html_file_suffix` if not None; `link_suffix = html_link_suffix` if not None else
   `out_suffix`; `use_index = get_builder_config('use_index', 'html')` (looks up
   `<buildername>_use_index` first, then `html_use_index`; `SPHINX/builders/__init__.py:860-874`).

`add_css_file(filename, **kw)` (`:282-287`): if no `://` → `posixpath.join('_static', filename)`;
append `_CascadingStyleSheet(filename, **kw)` unless an equal one exists (equality = filename +
priority + attributes). `add_js_file` (`:305-310`) same, but an empty filename (inline script) is
kept as `''`.

### 2.4 Asset objects (`SPHINX/builders/html/_assets.py`)

* `_CascadingStyleSheet(filename, *, priority=500, rel='stylesheet', type='text/css', **attrs)`
  → `attributes = {'rel': rel, 'type': type} | attrs` (`:20-32`). Immutable, hashable.
* `_JavaScript(filename, *, priority=500, **attrs)` → `attributes = attrs` (may contain `body`).
* `convert_html_css_files` / `convert_html_js_files` (`__init__.py:1295-1326`, `config-inited`
  priority 800) turn string entries into `(name, {})` and `(name, attrs)` tuples, warning
  `invalid css_file: %r, ignored` / `invalid js_file: %r, ignored` otherwise.

### 2.5 Config validation on `config-inited` (`__init__.py:1364-1451`) — warning texts **[probed]**

In this order (connect order, all priority 800):
`html_extra_path entry %r is placed inside outdir` / `html_extra_path entry %r does not exist`,
same two for `html_static_path`, `logo file %r does not exist` (and `html_logo = None`),
`favicon file %r does not exist` (and `html_favicon = None`); `ConfigError` for string values in
`html_sidebars` and for `html4_writer=True`. Probe output (stderr, no location prefix):

```
WARNING: html_extra_path entry 'nope' does not exist
WARNING: html_static_path entry '_static' does not exist
WARNING: logo file 'missing.png' does not exist
WARNING: unsupported theme option 'bogus' given
```

(the last one comes from `Theme.get_options` during `prepare_writing`, `SPHINX/theming.py:131-143`).
Invalid entries are *removed* from `html_extra_path`/`html_static_path` — note that
`html_static_path = ['_static']` without the directory is the sphinx-quickstart default, so this
warning is common in the wild.

---------------------------------------------------------------------------------------------------

## 3. Themes (`SPHINX/theming.py`)

### 3.1 Discovery (`HTMLThemeFactory`, `:152-265`)

* Built-ins: every subdirectory of `SPHINX/themes/` containing `theme.toml` or `theme.conf`
  (`_find_themes`, `:223-249`) — `agogo basic bizstyle classic default epub haiku nature nonav
  pyramid scrolls sphinxdoc traditional`. `.zip` files are themes too.
* Then `html_theme_path` entries (relative to confdir), then `sphinx.html_themes` entry points
  (deferred; loaded with `app.setup_extension`). `alabaster` is registered by the always-loaded
  builtin extension `alabaster` (`SPHINX/application.py:136-141`, `ALABASTER/__init__.py:34-43`
  `app.add_html_theme`).
* Unknown theme ⇒ `ThemeError: no theme named %r found (missing theme.toml?)`.

### 3.2 Loading with ancestors (`_load_theme_with_ancestors`, `:278-315`; `_load_theme`, `:318-342`)

Walk `inherit` up to 10 levels until `inherit == "none"`; `theme.toml` preferred over
`theme.conf`. `Theme.__init__` (`:64-92`) merges configs **from the root ancestor down**
(`reversed(configs.values())`): options dict-merged (child wins), and `stylesheets`,
`sidebar_templates`, `pygments_style_default`, `pygments_style_dark` each taken from the most
derived theme that sets them. `get_theme_dirs()` = `[this theme dir, parent dir, …, basic]`.

* `theme.toml` (`_convert_theme_toml`, `:382-407`): `[theme] inherit` (required, non-empty),
  `stylesheets` (list), `sidebars` (list), `pygments_style = { default = "...", dark = "..." }`
  (a bare string is a `ThemeError` with a hint), `[options]` table.
* `theme.conf` (`_convert_theme_conf`, `:425-451`): `[theme] stylesheet` / `sidebars` are
  comma-separated (stripped), `pygments_style`, `pygments_dark_style`; `[options]` via
  `RawConfigParser.items` (so option values are **strings**; `key =` with nothing ⇒ `''`).

`basic` (`SPHINX/themes/basic/theme.toml`):

```toml
[theme]
inherit = "none"
stylesheets = ["basic.css"]
sidebars = ["localtoc.html", "relations.html", "sourcelink.html", "searchbox.html"]
pygments_style = { default = "none" }

[options]
nosidebar = "false"
sidebarwidth = "230"
body_min_width = "360"
body_max_width = "800"
navigation_with_keys = "False"
enable_search_shortcuts = "True"
globaltoc_collapse = "true"
globaltoc_includehidden = "false"
globaltoc_maxdepth = ""
```

All option values are strings unless the user overrides them via `html_theme_options` (then
whatever Python type they gave). `get_options(overrides)` (`:131-143`) warns
`unsupported theme option %r given` for keys not declared by the theme chain and drops them.

### 3.3 Template loader chain (`BuiltinTemplateLoader.init`, `SPHINX/jinja2glue.py:166-213`)

```
pathchain   = theme.get_theme_dirs()                      # [child, ..., basic]
loaderchain = pathchain + [p.parent for p in pathchain]   # lets "basic/layout.html" resolve
if templates_path: prepend [confdir/tp for tp in templates_path] to both
```

`get_source` (`:238-252`): a name starting with `!` skips the `len(templates_path)` user loaders
(`{% extends "!layout.html" %}`); each `SphinxFileSystemLoader` (`:120-158`) additionally accepts a
legacy `_t` file for a requested `*.jinja` name. Not found ⇒
`TemplateNotFound("'<name>' not found in [<pathchain>]")`. alabaster's `layout.html` starts with
`{%- extends "basic/layout.html" %}` — that only works because of the parent-dir entries.

### 3.4 Pygments style for `pygments.css`

`PygmentsBridge.get_style` (`SPHINX/highlighting.py:121-130`): `''`/`'sphinx'` → `SphinxStyle`,
`'none'` → `NoneStyle`, dotted name → import, else `pygments.styles.get_style_by_name`.
`get_stylesheet()` = `HtmlFormatter(style=...).get_style_defs('.highlight')` (`:232-237`).
The basic theme's `pygments.css` **[probed]** (533 bytes, no trailing newline, crc `8e8a900e`):

```
pre { line-height: 125%; }
td.linenos .normal { color: inherit; background-color: transparent; padding-left: 5px; padding-right: 5px; }
span.linenos { color: inherit; background-color: transparent; padding-left: 5px; padding-right: 5px; }
td.linenos .special { color: #000000; background-color: #ffffc0; padding-left: 5px; padding-right: 5px; }
span.linenos.special { color: #000000; background-color: #ffffc0; padding-left: 5px; padding-right: 5px; }
.highlight .hll { background-color: #ffffcc }
.highlight { background: #ffffff; }
```

Recommendation: vendor pre-generated CSS for every style name reachable without executing
Python (`none`, `sphinx`, the ~49 built-in Pygments styles, `sphinx.pygments_styles.PyramidStyle`,
`alabaster.support.Alabaster`) via a `tools/gen_pygments_css.py`; the CSS is a pure function of
the style (and pygments version, pinned by the oracle environment: 2.21.0).

---------------------------------------------------------------------------------------------------

## 4. basic theme files

Templates (`SPHINX/themes/basic/`): `layout.html` (master), `page.html` (doc pages:
`{%- extends "layout.html" %}` + `{% block body %}\n  {{ body }}\n{% endblock %}`), sidebar
fragments `localtoc.html`, `relations.html`, `sourcelink.html`, `searchbox.html` (defaults) plus
`globaltoc.html`, `searchfield.html`; `genindex.html`, `genindex-single.html`,
`genindex-split.html`, `domainindex.html`, `search.html`, `opensearch.xml`, `defindex.html`
(deprecated), `changes/*` (changes builder only).

Static (`SPHINX/themes/basic/static/`): `basic.css.jinja` (templated: `theme_sidebarwidth|todim`
line 51, `theme_body_min_width|todim` line 214, `theme_body_max_width|todim` line 215),
`documentation_options.js.jinja`, `language_data.js.jinja`, and verbatim `doctools.js` (4332 B,
crc `fd6eb6e6`), `sphinx_highlight.js` (5325 B, crc `6ffebe34`), `searchtools.js` (22464 B),
`file.png`, `minus.png`, `plus.png` — all verbatim copies **[probed, cmp]**. The crate's
`static/doctools.js` and `static/sphinx_highlight.js` differ from upstream and its
`templates/*.html` are unrelated rewrites (not the Sphinx templates) — both must be replaced by
vendored upstream bytes.

`documentation_options.js.jinja` (complete):

```
const DOCUMENTATION_OPTIONS = {
    VERSION: '{{ release|e }}',
    LANGUAGE: '{{ language }}',
    COLLAPSE_INDEX: false,
    BUILDER: '{{ builder }}',
    FILE_SUFFIX: '{{ file_suffix }}',
    LINK_SUFFIX: '{{ link_suffix }}',
    HAS_SOURCE: {{ has_source|lower }},
    SOURCELINK_SUFFIX: '{{ sourcelink_suffix }}',
    NAVIGATION_WITH_KEYS: {{ 'true' if theme_navigation_with_keys|tobool else 'false'}},
    SHOW_SEARCH_SUMMARY: {{ 'true' if show_search_summary else 'false' }},
    ENABLE_SEARCH_SHORTCUTS: {{ 'true' if theme_enable_search_shortcuts|tobool else 'false'}},
};
```

Rendered **[probed]** for the basic probe (326 bytes, ends with `};` — no newline):

```
const DOCUMENTATION_OPTIONS = {
    VERSION: '1.0',
    LANGUAGE: 'en',
    COLLAPSE_INDEX: false,
    BUILDER: 'html',
    FILE_SUFFIX: '.html',
    LINK_SUFFIX: '.html',
    HAS_SOURCE: true,
    SOURCELINK_SUFFIX: '.txt',
    NAVIGATION_WITH_KEYS: false,
    SHOW_SEARCH_SUMMARY: true,
    ENABLE_SEARCH_SHORTCUTS: true,
};
```

Note for dirhtml **[probed]**: `BUILDER: 'dirhtml'` but `FILE_SUFFIX: '.html'` and
`LINK_SUFFIX: '.html'` (the builder's `link_suffix` stays `.html` even though dirhtml URIs never use
it). `HAS_SOURCE` follows `html_copy_source`.

`basic.css` = `basic.css.jinja` with `230px`/`360px`/`800px` substituted and the final newline
dropped **[probed, diff]** (14685 B, crc `29da98fa`). `_todim` (`SPHINX/jinja2glue.py:46-60`):
`None`→`'initial'`, digit-string/int → `'0'` if zero else `'<n>px'`, else unchanged (alabaster's
`body_min_width = inherit` ⇒ `min-width: inherit;`, crc `b08954a9`).

`language_data.js` (13589 B for `en`) = header comment, `const stopwords = new Set(<json.dumps(sorted(stopwords))>);`,
`window.stopwords = stopwords;  // Export to global scope`, then the minified
`search/minified-js/base-stemmer.js` + `\n` + `english-stemmer.js` + `\n` +
`window.Stemmer = EnglishStemmer;` and one final `\n` (`SPHINX/search/__init__.py:552-589`). It is
a pure function of the search language (+ `html_search_scorer` file, + splitter) — vendor the
rendered bytes per language. `base-stemmer.js` / `english-stemmer.js` are verbatim copies of
`SPHINX/search/non-minified-js/*` (`copy_stemmer_js`, `__init__.py:832-849`).

---------------------------------------------------------------------------------------------------

## 5. Template engine semantics (what minijinja must reproduce)

### 5.1 Environment set-up (`SPHINX/jinja2glue.py:166-213`)

* `SandboxedEnvironment(loader=self, extensions=['jinja2.ext.i18n'] if builder._translator is not None)`.
  For every normal build `_translator` is set (English gets a NullTranslations-like object), so
  i18n is on. No `autoescape` ⇒ **off**. `trim_blocks=False`, `lstrip_blocks=False`,
  `keep_trailing_newline=False` (Jinja2 defaults; minijinja's defaults match).
* Filters added: `tobool` (`:33-36`: str → `lower() in {'true','1','yes','on'}`, else `bool()`),
  `toint` (`:39-43`: `int()` or 0), `todim` (above), `slice_index` (`:63-83`, genindex column split
  counting `1 + len(subitems)` per entry). Globals: `debug` (pformat of context), `warning`
  (`:110-117`: logs `in rendering <pagename><file_suffix>: <msg>` on logger `sphinx.themes`,
  returns `''`), `accesskey` (`:86-93`), `idgen` (`:96-107`, object with `.current()` and
  `.next()`; `next` increments then returns). i18n installs `_`, `gettext`, `ngettext`.
* `accesskey(key)` stores emitted keys in `context.vars['_accesskeys']` for the **whole page
  render** (shared across macro calls): the first relbar gets `accesskey="I"`, `"N"`, `"P"`, `"U"`,
  the second relbar gets none **[probed]**. Empty key ⇒ `''`. minijinja: use
  `State::get_or_set_temp_object` (exists in 2.24, `src/vm/state.rs:399`).

### 5.2 Value semantics that differ from minijinja defaults

Checked with real Jinja2 **[probed]**:

| Expression | Jinja2 (Sphinx) | minijinja default |
|---|---|---|
| `{{ True }}` / `{{ None }}` / `{{ (9,1,0,'final',0) }}` / `{{ {'a':1} }}` | `True` / `None` / `(9, 1, 0, 'final', 0)` / `{'a': 1}` (Python `str()`) | `true` / `none` / `[9, 1, 0, "final", 0]`… |
| `{{ "a'b\"c<>&/"\|e }}` | `a&#39;b&#34;c&lt;&gt;&amp;/` | `a&#x27;b&quot;c&lt;&gt;&amp;&#x2f;` |
| `{{ x\|striptags }}` for `'A &amp; B <em>c</em>\n   d &#8212; <!-- x -->e &lt;f&gt;'` | `A & B c d — e <f>` | (no such filter) |
| `{{ True\|lower }}` / `{{ None\|lower }}` | `true` / `none` | `true` / `none` |
| undefined `{{ u }}`, `{{ u\|e }}`, `{{ u\|striptags }}` | `''` | `''` (lenient) |
| undefined `{{ u.attr }}` | `UndefinedError` | error |
| `" &#8212; "\|safe + d` with `d='a&b'` | ` &#8212; a&amp;b` (Markup `+` escapes the plain operand) | concatenation, no escaping |
| template `'x\n'` / `'x\n\n'` | `'x'` / `'x\n'` | same |
| `{% for a, (b, c, _) in l %}` | nested unpack OK | OK (`src/compiler/parser.rs:919-957`) |

`striptags` = `Markup(str(v)).striptags()` (`$SITE/markupsafe/__init__.py:199-228`): (1) repeatedly
cut `<!--`…`-->` (stop if no closing `-->`), (2) repeatedly cut `<`…`>` (stop if no `>`),
(3) `" ".join(value.split())` — Python whitespace split (includes `\xa0`, `\x1c-\x1f`, `\x85`,
U+2000…), (4) `html.unescape` (full HTML5 entity table, numeric refs, legacy semicolon-less
names). Order matters: an `&nbsp;` survives as U+00A0 because unescape runs after the collapse.
`escape` (`$SITE/markupsafe/_native.py:1-8`): `&`→`&amp;`, `>`→`&gt;`, `<`→`&lt;`, `'`→`&#39;`,
`"`→`&#34;`; a value already marked safe is returned unchanged.

**Different escaper in `css_tag`/`js_tag`**: attributes there use Python `html.escape(value,
quote=True)` → `&amp; &lt; &gt; &quot; &#x27;` (§8.4).

Places the basic templates print a raw bool/None: none by default (`has_source|lower` is the only
bool, and `lower` agrees). A Python-`str()` formatter is still needed for robustness against
`html_context`/`html_theme_options` values.

### 5.3 `{% trans %}` — every occurrence in the basic theme

minijinja has no `trans` tag (grep of `minijinja-2.24.0/src/compiler/parser.rs`: none). Jinja2's
i18n semantics (`$SITE/jinja2/ext.py:372-478`): the block body's literal text is the msgid (with
`%` doubled when there are variables); `trimmed` ⇒ `_ws_re = r"\s*\n\s*"` replaced by one space
after `strip()`; `{{ name }}` inside becomes `%(name)s`; output = `gettext(msgid) % vars`. Without
`trimmed` the whitespace is kept verbatim **[probed]** (search page:
`"    Please activate JavaScript to enable the search\n    functionality.\n"` spacing is preserved).

| Template:line | Block |
|---|---|
| `layout.html:115` | `{% trans docstitle=docstitle\|e %}Search within {{ docstitle }}{% endtrans %}` |
| `layout.html:183-185` | `{% trans trimmed copyright_prefix=copyright_prefix, copyright=copyright_line\|e %}\n        &#169; {{ copyright_prefix }} {{ copyright }}.\n      {% endtrans %}` |
| `layout.html:189-191` | same with `copyright=copyright\|e` |
| `layout.html:201` | `{% trans last_updated=last_updated\|e %}Last updated on {{ last_updated }}.{% endtrans %}` |
| `layout.html:204` | `{% trans sphinx_version=sphinx_version\|e %}Created using <a href="https://www.sphinx-doc.org/">Sphinx</a> {{ sphinx_version }}.{% endtrans %}` |
| `search.html:20-21` | `{% trans %}Please activate JavaScript to enable the search\n    functionality.{% endtrans %}` |
| `search.html:28-29` | `{% trans %}Searching for multiple words only shows matches that contain\n    all words.{% endtrans %}` |
| `genindex-single.html:26` | `{% trans key=key %}Index &#x2013; {{ key }}{% endtrans %}` |
| `opensearch.xml:4` | `{% trans docstitle=docstitle\|e %}Search {{ docstitle }}{% endtrans %}` |

`_('…')` msgids used by basic templates: `Navigation`, `About these documents`, `Index`, `Search`,
`Copyright`, `Table of Contents`, `Previous topic`, `previous chapter`, `Next topic`,
`next chapter`, `This Page`, `Show Source`, `Quick search`, `Go`, `search`,
`Index pages by letter`, `Full index on one page`, `can be huge`, plus the builder-side
`General Index`, `index`, `next`, `previous`, `Logo of %s`, `%s %s documentation`, `%b %d, %Y`,
`Python Module Index`, `modules`. For `language = 'en'` all are identity; other languages need
`SPHINX/locale/<lang>/LC_MESSAGES/sphinx.mo` (M7).

Recommended approach: a vendoring script that copies the templates byte-for-byte and rewrites each
`{% trans … %}…{% endtrans %}` into `{{ _trans("<msgid>", name=(expr), …) }}` with the msgid
pre-trimmed exactly as Jinja2 would, where `_trans` = gettext then Python `%`-formatting — keeping
all surrounding bytes identical. The macros around the copyright block rely on the trans output
being inline text (no whitespace control on the trans tags themselves).

### 5.4 Other constructs used by basic (all supported by minijinja 2.24, verify in unit tests)

`{% extends %}` with content before it being **emitted** (genindex-single.html defines its macro
before `{%- extends "layout.html" %}` and its pages start with a `\n` **[probed]**; minijinja
begins the discard-capture only at `LoadBlocks`, `src/vm/mod.rs:738-748`, so it matches);
top-level `{% set title = _('Index') %}` in child templates visible to parent blocks (minijinja
runs the whole child with output discarded — matches); `super()`; macros; `{% include
sidebartemplate %}` (dynamic name); `{% for … if … %}`; `loop.first/last/index`; `is defined`,
`is not none`, `is iterable`, `is string`; `None`/`True` literals (minijinja accepts both cases,
`src/compiler/parser.rs:716-718`); `attr` filter (`css|attr("filename")`); `slice(2)` (same
algorithm as Jinja2's `do_slice`); `~` concatenation.

---------------------------------------------------------------------------------------------------

## 6. `prepare_writing(docnames)` — the global context (`SPHINX/builders/html/__init__.py:427-562`)

Order of side effects:

1. Search indexer (`self.search` is True): `IndexBuilder(env, html_search_language or language,
   html_search_options, html_search_scorer)` + `load_indexer(docnames)` (reads the old
   `searchindex.js`; if unreadable and some docs are not being rebuilt, warns
   `search index couldn't be loaded, but not all documents will be built: the index will be incomplete.`).
2. `self.docsettings = _get_settings(HTMLWriter, defaults=env.settings, read_config_files=True)`;
   `docsettings.compact_lists = bool(html_compact_lists)`. `read_config_files=True` means docutils
   config files (incl. `confdir/docutils.conf`, via Sphinx's `DOCUTILSCONFIG` patch) apply to the
   writer. `env.settings` = `SPHINX/environment/__init__.py:59-75` defaults (`auto_id_prefix='id'`,
   `image_loading='link'`, `embed_stylesheet=False`, `cloak_email_addresses=True`,
   `pep_base_url`, `rfc_base_url`, `doctitle_xform=False`, `sectsubtitle_xform=False`,
   `section_self_link=False`, `halt_level=5`, `file_insertion_enabled=True`,
   `smartquotes_locales=[]`, `input_encoding='utf-8-sig'`) plus per-build `language_code`,
   `smart_quotes`, `trim_footnote_reference_space`.
3. Domain indices (`:447-468`): if `html_domain_indices` is truthy — `True` means all, a
   list/set means only those `'<domain>-<index>'` names — iterate `env.domains.sorted()` (by domain
   name) × `domain.indices`; `content, collapse = index_cls(domain).generate()`; keep only
   non-empty `content`. In a default install only `py-modindex` exists (`localname='Python Module
   Index'`, `shortname='modules'`).
4. `last_updated` = `format_date(html_last_updated_fmt or _('%b %d, %Y'), language=…,
   local_time=not html_last_updated_use_utc)` when the fmt is not `None`
   (`SPHINX/util/i18n.py:263-…`; honours `SOURCE_DATE_EPOCH`, then forces UTC).
5. `logo`/`favicon`: `html_logo or ''`, `html_favicon or ''`; if not a URL, `os.path.basename`.
6. `self.relations = env.collect_relations()` (parent, prev, next per doc;
   `SPHINX/environment/__init__.py:778-795`; crate: `src/env/toctree.rs:714`).
7. `rellinks` (`:494-505`): `[('genindex', _('General Index'), 'I', _('index'))]` if `use_index`,
   then for each kept domain index with a `shortname`:
   `(index_name, index_cls.localname, '', index_cls.shortname)`.
8. Re-add registry css/js (assets registered after `init()`), then back up
   `_orig_css_files = list(dict.fromkeys(_css_files))` (dedupe by hash), same for js;
   `styles = list(_get_style_filenames())`.
9. `globalcontext` — **every key** (values = basic probe, `ctx/ctxdump.txt`):

| Key | Value / source |
|---|---|
| `embedded` | `self.embedded` → `False` |
| `project` | `config.project` → `'Probe'` |
| `release` | `return_codes_re.sub('', config.release)` (strip `[\r\n]+`) → `'1.0'` |
| `version` | `config.version` → `''` (not derived from release) |
| `last_updated` | str or `None` |
| `copyright` | `config.copyright` (str, or tuple — `check_confval_types` turns a list into a tuple, `SPHINX/config.py:816-819`) |
| `master_doc`, `root_doc` | `config.root_doc` → `'index'` |
| `use_opensearch` | `html_use_opensearch` (str, default `''`) |
| `docstitle` | `html_title` (default `_('%s %s documentation') % (project, release)` → `'Probe 1.0 documentation'`; empty release gives a double space: `'Misc  documentation'` **[probed]**) |
| `shorttitle` | `html_short_title` (default = `html_title`) |
| `show_copyright` | `html_show_copyright` → `True` |
| `show_search_summary` | `html_show_search_summary` → `True` |
| `show_sphinx` | `html_show_sphinx` → `True` |
| `has_source` | `html_copy_source` → `True` |
| `show_source` | `html_show_sourcelink` → `True` |
| `sourcelink_suffix` | `html_sourcelink_suffix` → `'.txt'` |
| `file_suffix` | `self.out_suffix` → `'.html'` |
| `link_suffix` | `self.link_suffix` → `'.html'` |
| `script_files` | **the live `self._js_files` list object** |
| `language` | `language.replace('_', '-')` or `None` → `'en'` |
| `css_files` | **the live `self._css_files` list object** |
| `sphinx_version` | `'9.1.0'` |
| `sphinx_version_tuple` | `(9, 1, 0, 'final', 0)` |
| `docutils_version_info` | `(0, 22, 4, 'final', 0)` |
| `styles` | `['basic.css']` |
| `rellinks` | list above |
| `builder` | `'html'` / `'dirhtml'` |
| `parents` | `[]` |
| `logo_url` | basename or URL or `''` |
| `logo_alt` | `_('Logo of %s') % project` |
| `favicon_url` | basename or URL or `''` |
| `html5_doctype` | `True` |
| `theme_<opt>` | every `theme.get_options(html_theme_options)` entry (basic: 9 keys, strings) |
| *(html_context keys)* | `globalcontext |= html_context` **last** — user keys override everything |

10. `ensuredir(outdir/'_sources')` — **always**, because `copysource` is a class attribute; with
    `html_copy_source = False` an empty `_sources/` directory still appears **[probed, misc/]**.

---------------------------------------------------------------------------------------------------

## 7. `get_doc_context(docname, body, metatags)` (`SPHINX/builders/html/__init__.py:564-642`)

```
related = self.relations.get(docname)            # [parent, prev, next]
rellinks = globalcontext['rellinks'][:]
next = {'link': get_relative_uri(docname, related[2]),
        'title': render_partial(env.titles[related[2]])['title']}   → rellinks.append((next, title, 'N', _('next')))
prev = {... related[1] ...}                                          → rellinks.append((prev, title, 'P', _('previous')))
   (KeyError from titles ⇒ that one is None)
parents: walk related[0] upwards collecting {'link', 'title'}; pop() the last (the root doc);
         reverse() → root-most first
title = render_partial(env.longtitles[docname])['title'] if present else ''
source_suffix = str(env.doc2path(docname, False))[len(docname):]       # '.rst'
sourcename = docname + source_suffix (+ html_sourcelink_suffix unless equal to source_suffix)
             if html_copy_source else ''
meta = env.metadata.get(docname)
toc = render_partial(document_toc(env, docname, tags))['fragment']
```

Returned keys (exactly these 12): `parents`, `prev`, `next`, `title`, `meta`, `body`,
`metatags`, `rellinks`, `sourcename`, `toc`, `display_toc` (= `env.toc_num_entries[docname] > 1`),
`page_source_suffix`. `write_doc` then adds `has_maths_elements`.

Examples **[probed]**: `sourcename` = `index.rst.txt`; with `html_sourcelink_suffix = ''` →
`sub/page.rst`; with suffix `.rst` → `index.rst` (no double suffix). `title` is **HTML**
(`'Solo <em>Title</em> with <code class="docutils literal notranslate"><span class="pre">code</span></code>'`,
`'Main &lt;Title&gt; &amp; Co'`); the templates apply `striptags|e` where plain text is needed.
A doc without a title gets `&lt;no title&gt;` (env title collector's `<no title>` text).
`.. title::` directive text feeds `longtitles` (page `<title>`), not `titles` (prev/next).

### 7.1 `render_partial(node)` (`:409-425`)

`None` ⇒ `{'fragment': ''}`. Else a fresh `docutils.utils.new_document('<partial node>', self._settings)`,
append the node, apply transforms: reader `[]`, parser `[Validate(835), SmartQuotes(855)]`
(no-op: `smart_quotes=False` in `_settings`), writer `[Messages(860), FilterMessages(870),
StripClassesAndElements(420), Admonitions(920)]` **[probed]**; walk with the HTML5 translator;
`fragment = ''.join(visitor.fragment)`, `title = ''.join(visitor.title)` (the inner HTML of a
document-level `title` node — the node was appended directly to the document, so docutils treats
it as the document title).

### 7.2 `document_toc` (`SPHINX/environment/adapters/toctree.py:50-67`)

`tocdepth = env.metadata[docname].get('tocdepth', 0)`; `_toctree_copy(env.tocs[docname], 2,
tocdepth, False, tags)` (`:485-560`: shallow-copies `compact_paragraph`/`list_item`/`bullet_list`,
keeps sub-lists while `depth <= maxdepth or maxdepth <= 0`, filters `only` by tags, deep-copies
`reference`/`title`); then every `reference['refuri'] = reference['anchorname'] or '#'`. Rendered
local TOC for the basic index page:

```
<ul>
<li><a class="reference internal" href="#">Welcome to Probe</a><ul>
<li><a class="reference internal" href="#section-a">Section A</a><ul>
<li><a class="reference internal" href="#spam"><code class="docutils literal notranslate"><span class="pre">spam()</span></code></a></li>
</ul>
</li>
<li><a class="reference internal" href="#section-b">Section B</a></li>
</ul>
</li>
</ul>
```

(the string ends with `</ul>\n`).

---------------------------------------------------------------------------------------------------

## 8. `handle_page(pagename, addctx, templatename='page.html', *, outfilename=None, event_arg=None)` (`:1070-1257`)

### 8.1 Context assembly (in this order)

```
ctx = globalcontext.copy()
ctx['pagename'] = ctx['current_page_name'] = pagename
ctx['encoding'] = html_output_encoding                  # 'utf-8'
default_baseuri = get_target_uri(pagename).rsplit('#', 1)[0]
ctx['pageurl'] = posixpath.join(html_baseurl, get_target_uri(pagename)) if html_baseurl else None
ctx['pathto'], ctx['hasdoc'], ctx['toctree']            # closures, below
ctx['sidebars'] = list(_get_sidebars(pagename))
ctx.update(addctx)                                      # doc context / index context
ctx['content_root'] = '../' * default_baseuri.count('/') or './'
ctx['css_tag'], ctx['js_tag']                           # closures, below
self._css_files[:] = self._orig_css_files               # undo previous page's additions
self._js_files[:]  = self._orig_js_files
update_page_context(...)                                # no-op hook
new_template = emit_firstresult('html-page-context', pagename, templatename, ctx, event_arg)
ctx['script_files'] = sorted(script_files, key=priority)   # stable; skipped if not sortable
ctx['css_files']    = sorted(css_files,    key=priority)
output = templates.render(templatename, ctx)
```

`pageurl` examples **[probed]**: `https://example.org/docs/sub/page.html`; dirhtml
`https://example.org/docs/sub/page/`; `posixpath.join` with an empty target yields a trailing `/`.

### 8.2 `pathto(otheruri, resource=False, baseuri=default_baseuri)` (`:1095-1108`)

```
if resource and '://' in otheruri: return otheruri
if not resource: otheruri = get_target_uri(otheruri)
uri = relative_uri(baseuri, otheruri) or '#'
```

(`allow_sharp_as_current_path` is True for html/dirhtml.) Self-link ⇒ `#`: `pathto(root_doc)` on
the root page renders `href="#"` in both builders **[probed]**. Resources are **not** URL-quoted
(`_sources/my doc.rst.txt` keeps its space **[probed]**).

### 8.3 `hasdoc(name)` (`:1112-1117`), `toctree(**kw)` (`:1121`, `:1022-1032`)

`hasdoc`: `name in env.all_docs`, or `name == 'search'` (and `self.search`), or
`name == 'genindex'` and `html_use_index`. So `about`/`copyright` link tags only appear if such
documents exist.

`toctree(**kw)` → `_get_local_toctree(pagename, collapse=True, **kw)`: `includehidden` defaults to
`False`; `maxdepth == ''` is dropped; `global_toctree_for_doc(env, docname, builder, tags,
collapse, **kw)` then `render_partial(...)['fragment']` (`''` if no toctree). **Quirk:** theme
options arrive as strings, so `globaltoc.html`'s `includehidden=theme_globaltoc_includehidden`
passes `'false'`, which is truthy in Python — hidden toctrees are included. Not used by basic's
default sidebars; used by alabaster's `navigation.html`.

### 8.4 `css_tag(css)` / `js_tag(js)` (`:1130-1180`) — exact strings

```
css_tag: attrs = sorted(f'{k}="{html.escape(v, quote=True)}"' for k, v in css.attributes.items() if v is not None)
         uri = pathto(css.filename, resource=True) (+ f'?v={crc}' if non-empty checksum; skipped for epub/htmlhelp)
         → f'<link {" ".join(attrs)} href="{uri}" />'
js_tag:  str (old style) → f'<script src="{pathto(js, resource=True)}"></script>'
         body = attributes.get('body', ''); attrs = sorted(... for k != 'body' and v is not None)
         no filename → f'<script {attrs}>{body}</script>' or f'<script>{body}</script>'   (body NOT escaped)
         filename    → uri (+ ?v=crc unless 'MathJax.js?' in name or builder is epub)
                     → f'<script {attrs} src="{uri}"></script>' or f'<script src="{uri}"></script>'
```

`sorted(attrs)` sorts the formatted `key="value"` strings (so `data-x=` sorts before `data=`).
Probed output (`feat/out/sub/page.html`), showing priority order (100 < 200 < 800, stable):

```
    <link media="print" rel="stylesheet" type="text/css" href="../_static/print.css?v=46ea081f" />
    <link rel="stylesheet" type="text/css" href="../_static/pygments.css?v=8e8a900e" />
    <link rel="stylesheet" type="text/css" href="../_static/basic.css?v=29da98fa" />
    <link rel="stylesheet" type="text/css" href="../_static/custom.css?v=a8e7618a" />
    <link rel="stylesheet" type="text/css" href="https://cdn.example.org/x.css" />
    <script src="../_static/documentation_options.js?v=789eb5fd"></script>
    <script src="../_static/doctools.js?v=fd6eb6e6"></script>
    <script src="../_static/sphinx_highlight.js?v=6ffebe34"></script>
    <script src="../_static/custom.js?v=926908a2"></script>
    <script async="async" defer="defer" src="../_static/defer.js"></script>
    <script>var x = 1 < 2;</script>
```

(`defer.js` is empty ⇒ no `?v=`; `custom.js` contains `\r\n` and its crc is over the `\r`-stripped
bytes.) The layout's `css()` macro emits `css_tag(css)` when `css|attr("filename")` is truthy,
else `<link rel="stylesheet" href="{{ pathto(css, 1)|e }}" type="text/css" />` (string entries
injected via `html_context`).

MathJax (`SPHINX/ext/mathjax.py:81-139`, handler on `html-page-context`): only when
`has_maths_elements` (or `html_assets_policy == 'always'`) it calls `builder.add_js_file(mathjax_path,
defer='defer')` (priority 500) — which mutates the *live* list that `ctx['script_files']` points to,
so the tag appears on that page only **[probed, deep/]**:
`<script defer="defer" src="https://cdn.jsdelivr.net/npm/mathjax@4/tex-mml-chtml.js"></script>`.

Built-in `html-page-context` handlers in a default build: `setup_resource_paths`
(`__init__.py:1329-1347`: `favicon_url`/`logo_url` → `pathto('_static/' + x, resource=True)` when
not URLs), `install_mathjax`, and alabaster's `update_context` (adds `alabaster_version`,
`alabaster_version_info`; rewrites `show_sphinx` if `html_theme_options` has `show_powered_by`) —
alabaster's handler runs even when the theme is basic (it's a builtin extension).

### 8.5 Render, write, copy source (`:1208-1257`)

* Rendering errors: `UnicodeError` ⇒ warning `a Unicode error occurred when rendering the page %s. …`
  and the page is skipped; other exceptions ⇒ `ThemeError('An error happened in rendering the
  page %s.\nReason: %r')` (special message for the removed `style` variable).
* Output path = `outfilename` or `get_output_path(pagename)`; `ensuredir(parent)`;
  `write_text(output, encoding=ctx['encoding'], errors='xmlcharrefreplace')` (non-representable
  characters become `&#NNN;` for non-UTF-8 `html_output_encoding`).
* If `copysource and ctx.get('sourcename')`: `copyfile(env.doc2path(pagename), _sources/<sourcename>, force=True)`
  — raw bytes of the source file (BOM/CRLF preserved) and its mtime (`SPHINX/util/osutil.py:95-144`).

---------------------------------------------------------------------------------------------------

## 9. URIs and output paths

### 9.1 `relative_uri(base, to)` (`SPHINX/util/osutil.py:46-66`) — port verbatim

```python
if to.startswith('/'): return to
b2 = base.partition('#')[0].split('/')
t2 = to.partition('#')[0].split('/')
for x, y in zip(b2[:-1], t2[:-1]):      # common leading dirs, never the last segment
    if x != y: break
    b2.pop(0); t2.pop(0)
if b2 == t2: return ''                                  # same file (anchor dropped!)
if len(b2) == 1 and t2 == ['']: return './'             # 'f/index.html' -> 'f/'
return '../' * (len(b2) - 1) + '/'.join(t2)
```

Note the fragment of `to` is discarded. `Builder.get_relative_uri(from_, to, typ=None)` =
`relative_uri(get_target_uri(from_), get_target_uri(to, typ))` (`SPHINX/builders/__init__.py:189-197`).
The crate's `src/utils.rs:563-581` (`pathdiff`-based, with a `suffix` parameter) is not this
algorithm and must not be used for HTML links.

### 9.2 html (`SPHINX/builders/html/__init__.py:1034-1038, 1067-1068`)

* `get_target_uri(docname, typ=None) = urllib.parse.quote(docname) + link_suffix` —
  `quote` with `safe='/'`: keeps ASCII letters/digits, `_.-~` and `/`, percent-encodes the rest as
  UTF-8 with uppercase hex. **[probed]** `café` → `caf%C3%A9.html`, `my doc` → `my%20doc.html`
  (files on disk: `café.html`, `my doc.html`).
* `get_output_path(page) = outdir / (page + out_suffix)`; `get_outfilename` wraps it.

### 9.3 dirhtml (`SPHINX/builders/dirhtml.py:27-38`)

```python
def get_target_uri(docname, typ=None):
    if docname == 'index': return ''
    if docname.endswith('/index'): return docname[:-5]     # keeps the trailing '/'
    return docname + '/'                                   # NOT url-quoted (unlike html)
def get_output_path(page):
    parts = page.split('/'); if parts[-1] == 'index': parts.pop()
    return outdir.joinpath(*parts, 'index' + out_suffix)
```

Probed dirhtml tree (`feat/outdir`): `index.html`, `sub/index.html`, `sub/page/index.html`,
`genindex/index.html`, `genindex-A/index.html`, …, `genindex-all/index.html`,
`search/index.html`, `_static/opensearch.xml`. `content_root` for `sub/page/index.html` is
`../../`; for the root page `./`. genindex links are relative to `genindex/`:
`<a href="../#index-0">`, `<a href="../sub/page/#index-0">`. OpenSearch quirk (page `opensearch`
has target uri `opensearch/`): `template="https://example.org/docs/../search/?q={searchTerms}"`
and `https://example.org/docs/../_static/fav.ico`.

### 9.4 `write_doc` path fields

`imgpath = relative_uri(get_target_uri(docname), '_images')` (e.g. `sub/page.html` → `../_images`;
dirhtml `sub/page/` → `../../_images`); `dlpath` likewise for `_downloads`; `content_root` as
above.

---------------------------------------------------------------------------------------------------

## 10. Writing one document

### 10.1 `write_doc_serialized(docname, doctree)` (`:667-673`) — always in the main process

1. `imgpath = relative_uri(get_target_uri(docname), '_images')`.
2. `post_process_images(doctree)` (`:961-987` → `Builder.post_process_images`,
   `SPHINX/builders/__init__.py:213-250`): for each `image` node: skip if `'?'` in `candidates`
   (remote/data URI); if no `'*'` pick the first of `supported_image_types` present in candidates,
   else warn `a suitable image for html builder not found: %s (%s)` (sorted mimetypes, original
   uri) / `…: %s` and leave it; set `node['uri'] = candidate`; if `candidate in env.images`:
   `self.images[candidate] = env.images[candidate][1]` (the unique basename). Then, if
   `html_scaled_image_link` and the builder allows it, every image with `scale`/`width`/`height`,
   not already inside a `reference`, and without class `no-scaled-link` is wrapped in
   `reference(internal=True, refuri=posixpath.join(imgpath, images[uri]) or uri)` **[probed]**:
   `<a class="reference internal image-reference" href="../_images/pic.png"><img alt="../_images/pic.png" src="../_images/pic.png" style="width: 10px;" />\n</a>`.
3. `index_page(docname, doctree, longtitle.astext())` — search indexing (M3); docs with metadata
   `no-search`/`nosearch` are fed with an empty title/doc.

`self.images` therefore only contains images of documents written **in this build** — on an
incremental build `_images/` gets only those (older files stay on disk).

### 10.2 `write_doc(docname, doctree)` (`:650-665`)

```
doctree.settings = self.docsettings
self.secnumbers = env.toc_secnumbers.get(docname, {})
self.fignumbers = env.toc_fignumbers.get(docname, {})
self.imgpath = relative_uri(get_target_uri(docname), '_images')
self.dlpath  = relative_uri(get_target_uri(docname), '_downloads')
self.current_docname = docname
visitor = create_translator(doctree, self); doctree.walkabout(visitor)
body = ''.join(visitor.fragment)
clean_meta = ''.join(visitor.meta[2:])
ctx = get_doc_context(docname, body, clean_meta); ctx['has_maths_elements'] = visitor._has_maths_elements
handle_page(docname, ctx, event_arg=doctree)             # template 'page.html'
```

**No docutils writer transforms run on the page doctree** (unlike `render_partial`): the
translator walks the resolved doctree directly.

`visitor.meta` = `[content_type, generator, viewport, <meta directive tags…>]`
(`DOCUTILS/writers/_html_base.py:342,353-354`; `html5_polyglot/__init__.py:129-132`), so
`meta[2:]` = `'<meta name="viewport" content="width=device-width, initial-scale=1" />\n'` plus
one `emptytag` per `.. meta::` field, e.g. **[probed]**
`<meta content="A &lt;desc&gt; &amp; &quot;more&quot;" name="description" />\n` (attributes sorted,
docutils attribute escaping with `&quot;`). The template inserts `metatags` with `{{- metatags }}`
right after the layout's own viewport tag — hence the characteristic double viewport line.

---------------------------------------------------------------------------------------------------

## 11. Finish tasks in detail

### 11.1 `gen_indices` (`:685-692`) → `write_genindex` (`:722-750`) + `write_domain_indices` (`:752-760`)

`genindex = IndexEntries(env).create_index(self)` — URIs are `get_relative_uri('genindex',
docname) + '#' + target_id` (`SPHINX/environment/adapters/indexentries.py:65-73`). Shape
**[probed]**:

```python
genindexentries = [
  ('B', [('built-in function', ([], [('spam()', [('', 'index.html#spam')])], None))]),
  ('E', [('eggs', ([('', 'index.html#index-0')], [], None))]),
  ('M', [('module', ([], [('mymod', [('', 'other.html#module-mymod')])], None)),
         ('mymod',  ([], [('module', [('', 'other.html#module-mymod')])], None))]),
  ('S', [('spam()', ([], [('built-in function', [('', 'index.html#spam')])], None))]),
]
# entry = (name, (links, subitems, category_key)); links = [(main, uri)], main is '' or 'main'
genindexcounts = [sum(1 + len(subitems) for _, (_, subitems, _) in entries) for _k, entries in genindex]  # [2, 1, 4, 2]
```

* not split: `handle_page('genindex', {'genindexentries', 'genindexcounts', 'split_index': False}, 'genindex.html')`
  — written even when there are no entries (only needs `html_use_index`).
* split (`html_split_index=True`): `genindex` with `genindex-split.html`, `genindex-all` with
  `genindex.html` (same context), and one `genindex-<key>` per letter with `genindex-single.html`
  and `{'key', 'entries', 'count', 'genindexentries'}`. Letter pages start with a blank line
  (content before `{% extends %}`) **[probed]**.
* The `sidebarrel` block overrides in the genindex templates are **dead**: `ctx['sidebars']` is
  always a list, so `layout.html` takes the new-style `{% include %}` branch.

Domain indices: `handle_page(index_name, {'indextitle': localname, 'content': content,
'collapse_index': collapse}, 'domainindex.html')`. `content` = `[(letter, [IndexEntry(name,
subtype, docname, anchor, extra, qualifier, descr), …])]`; the template unpacks
`(name, grouptype, page, anchor, extra, qualifier, description)` and links
`pathto(page)|e + '#' + anchor`. Probed py-modindex body (basic probe):

```
   <h1>Python Module Index</h1>

   <div class="modindex-jumpbox">
   <a href="#cap-m"><strong>m</strong></a>
   </div>

   <table class="indextable modindextable">
     <tr class="pcap"><td></td><td>&#160;</td><td></td></tr>
     <tr class="cap" id="cap-m"><td></td><td>
       <strong>m</strong></td><td></td></tr>
     <tr>
       <td></td>
       <td>
       <a href="other.html#module-mymod"><code class="xref">mymod</code></a></td><td>
       <em></em></td></tr>
   </table>
```

and, because `collapse_index` is True, its head gains (after the link tags and the ` ` of
`extrahead`):

```
 

    <script>
      DOCUMENTATION_OPTIONS.COLLAPSE_INDEX = true;
    </script>


  </head><body>
```

### 11.2 `gen_additional_pages` (`:700-720`)

`html_additional_pages` items (`handle_page(pagename, {}, template)`), then `search`
(`search.html`, always for html/dirhtml), then, iff `html_use_opensearch` (the base URL string),
`opensearch` → `opensearch.xml` written to `_static/opensearch.xml`. Probed opensearch.xml (no
trailing newline):

```
<?xml version="1.0" encoding="UTF-8"?>
<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
  <ShortName>Feat &amp; &#34;Q&#34;</ShortName>
  <Description>Search Feat &amp; &#34;Q&#34; 2.0 documentation</Description>
  <InputEncoding>utf-8</InputEncoding>
  <Url type="text/html" method="get"
       template="https://example.org/docs/search.html?q={searchTerms}"/>
  <LongName>Feat &amp; &#34;Q&#34; 2.0 documentation</LongName>
  <Image height="16" width="16" type="image/x-icon">https://example.org/docs/_static/fav.ico</Image>
  
</OpenSearchDescription>
```

Search page specifics **[probed]**: title `Search`; `{%- block scripts %}` adds
`<script src="_static/searchtools.js"></script>` and `<script src="_static/language_data.js"></script>`
(no `?v=`, hard-coded in the template) after a line of 4 spaces; `extrahead` adds
`<script src="searchindex.js" defer="defer"></script>` and `<meta name="robots" content="noindex" />`;
the sidebar wrapper is empty (no local toc, no relations, no source, searchbox suppressed on
`pagename == "search"`); `<link rel="search" title="Search" href="#" />`.

### 11.3 `copy_image_files` (`:762-784`)

If `self.images`: `ensuredir(_images)`; for each `src → dest` (status "copying images... "):
`copyfile(srcdir/src, _images/dest, force=True)`; failure ⇒ warning `cannot copy image file '%s': %s`.
Unique names come from `env.images` = `FilenameUniqDict` (`SPHINX/util/_files.py:24-41`): first
come keeps its basename, later same-basename files get `stem1.ext`, `stem2.ext`, … in
`add_file` order (document read order). The env collector (`SPHINX/environment/collectors/asset.py:48-99`)
fills `node['candidates']` and `env.images`, and warns `image file not readable: %s`. **The crate's
`BuildEnvironment` (`src/env/mod.rs:146-200`) has no `images`/`dlfiles` fields yet** — this is the
"image without candidates" exemption in `tests/env_differential.rs`.

### 11.4 `write_buildinfo` (`:950-954`) — see §13; failure ⇒ `Failed to write build info file: %r`.

### 11.5 `dump_search_index` (`:1272-1288`)

Prune to `env.all_docs`, write `searchindex.js.tmp`, rename to `searchindex.js`. Format
(`SPHINX/search/__init__.py:160-183, 425-462`): `'Search.setIndex(' + json.dumps(data,
separators=(',', ':'), sort_keys=True) + ')'` (ASCII-escaped JSON, no newline) with keys
`alltitles, docnames, envversion, filenames, indexentries, objects, objnames, objtypes, terms,
titles, titleterms`. Probed sample (basic):

```
Search.setIndex({"alltitles":{"Other Page":[[1,null]],"Section A":[[0,"section-a"]],"Section B":[[0,"section-b"]],"Sub":[[1,"sub"]],"Welcome to Probe":[[0,null]]},"docnames":["index","other"],"envversion":{"sphinx":66,"sphinx.domains.c":3,"sphinx.domains.changeset":1,"sphinx.domains.citation":1,"sphinx.domains.cpp":9,"sphinx.domains.index":1,"sphinx.domains.javascript":3,"sphinx.domains.math":2,"sphinx.domains.python":4,"sphinx.domains.rst":2,"sphinx.domains.std":2},"filenames":["index.rst","other.rst"],"indexentries":{"built-in function":[[0,"spam",false]],"eggs":[[0,"index-0",false]],"module":[[1,"module-mymod",false]],"mymod":[[1,"module-mymod",false]],"spam()":[[0,"spam",false]]},"objects":{"":[[1,0,0,"-","mymod"],[0,1,1,"","spam"]]},"objnames":{"0":["py","module","Python module"],"1":["py","function","Python function"]},"objtypes":{"0":"py:module","1":"py:function"},"terms":{"More":1,"Other":0,"Some":1,"doe":0,"emphasi":0,"intro":0,"page":0,"paragraph":0,"refer":1,"spam":[0,1],"sub":0,"text":[0,1],"x":0},"titles":["Welcome to Probe","Other Page"],"titleterms":{"A":0,"Other":1,"b":0,"page":1,"probe":0,"section":0,"sub":1,"welcom":0}})
```

Wave-5 options: (a) emit no `searchindex.js` and list it as a known file-set gap (the search page
then 404s its index); (b) emit the cheap, exact keys (`docnames`, `filenames`, `titles` =
`longtitle.astext()`, `envversion` constant, `alltitles`, `indexentries`, `objects/objnames/
objtypes`) and empty `terms`/`titleterms`, exempting the file's bytes until M3. Everything
else of the search stack (search.html, searchtools.js, language_data.js, stemmer copies) is
template/static work and belongs in wave 5.

### 11.6 `dump_inventory` (`:1268-1270`) → `InventoryFile.dump` (`SPHINX/util/inventory.py:175-207`)

Header `# Sphinx inventory version 2\n# Project: <re.sub(r'\s+',' ',project)>\n# Version: <…version>\n# The remainder of this file is compressed using zlib.\n`,
then zlib level 9 over lines `f'{fullname} {domain}:{type} {prio} {uri} {dispname}\n'` for
`env.domains.sorted()` × `sorted(domain.get_objects())`, with `uri = builder.get_target_uri(docname)`
(+ `#anchor`, anchor shortened to `$` when it ends with fullname) and dispname `-` when equal to
fullname. Always written (also when empty). The crate already has the writer
(`src/inventory.rs:333-…` `InventoryFile::dump(path, project, version, domains, get_target_uri)`)
with no production call site — wave 5 calls it from the finish sequence with the builder's
`get_target_uri` (dirhtml URIs differ).

---------------------------------------------------------------------------------------------------

## 12. `copy_assets` (runs before pages are written; `:644-648`)

### 12.1 `copy_download_files` (`:786-810`)

For each `env.dlfiles` entry (`DownloadFiles`, `SPHINX/util/_files.py:71-83`): destination
`_downloads/<md5(srcdir-relative posix path)>/<basename>`; e.g. `_static/print.css` →
`_downloads/5114ea0b461573f860748f92b4d6d01f/print.css` (= `md5(b'_static/print.css')`)
**[probed]**. Status line `copying downloadable files... [100%] _static/print.css` then an empty
line. Warning `cannot copy downloadable file %r: %s`. The env collector warns
`download file not readable: %s` (`asset.py:151-175`).

### 12.2 `copy_static_files` (`:913-933`) in order

1. `mkdir _static`.
2. `context = globalcontext.copy()` + `indexer.context_for_searchtool()`
   (`search_language_stemming_code`, `search_language_stop_words`, `search_scorer_tool`,
   `search_word_splitter_code`). **No `pathto`/`css_tag`/page keys** in static templates.
3. `create_pygments_style_file()` → `_static/pygments.css` (and `pygments_dark.css`).
4. `copy_translation_js()` → `_static/translations.js` (non-English only).
5. `copy_stemmer_js()` → `_static/base-stemmer.js`, `_static/english-stemmer.js` (for `en`).
6. `copy_theme_static_files(context)`: for each theme dir **from the root ancestor to the child**
   (`reversed(get_theme_dirs())`), `copy_asset(<dir>/static, _static, excluded=DOTFILES,
   context=context, renderer=self.templates, force=True)` — child themes overwrite.
7. `copy_static_dirs()` — extension static dirs (`registry.static_dirs`), `shutil.copytree`.
8. `copy_html_static_files(context)` — each `html_static_path` entry (relative to confdir),
   excluded `Matcher(exclude_patterns + ['**/.*'])`, templated, `force=True` (user files replace
   theme files of the same name).
9. `copy_html_logo()`, `copy_html_favicon()` → `_static/<basename>`.

`copy_asset`/`copy_asset_file` (`SPHINX/util/fileutil.py:24-168`): a file whose lower-cased name
ends with `_t` or `.jinja` is rendered with `renderer.render_string(text, context)` and written to
the name minus the suffix (`tmpl.css_t` → `tmpl.css`; `foo.CSS_T` → `foo.CSS`) with the
`Writing evaluated template result to <abs path>` info line; everything else is `copyfile`d
(byte copy + mtime). Rendering uses the same Jinja2 environment: no autoescape, trailing newline
stripped (`/* {{ project }} */\n` → `/* Feat & "Q" */`, no newline **[probed]**).

### 12.3 `copy_extra_files` (`:935-948`)

Each `html_extra_path` entry copied into `outdir` root with `Matcher(exclude_patterns)` only (dot
files such as `.htaccess` **are** copied), no templating (no context passed).

---------------------------------------------------------------------------------------------------

## 13. `.buildinfo`

### 13.1 Exact bytes (`SPHINX/builders/html/_build_info.py:71-79`)

```
# Sphinx build info version 1
# This file records the configuration used when building these files. When it is not found, a full rebuild will be done.
config: ee8c8458af746a6d4f6c98626c0842b6
tags: 645f666f9bcd5a90fca523b33c5a78b7
```

UTF-8, `\n` line ends, trailing `\n` after the tags line, 231 bytes. `load` (`:25-45`) requires
line 0 == `# Sphinx build info version 1` (after rstrip), `lines[2].startswith('config: ')`,
`lines[3].startswith('tags: ')`; equality = both hashes equal.

### 13.2 `stable_hash` (`SPHINX/util/_serialise.py:14-28`) — independently re-implemented and matched

```python
def md5(s): return hashlib.md5(s.encode()).hexdigest()   # UTF-8
def h(v):
    if isinstance(v, dict):                    # NOTE: 'if' then 'if', not elif → double hash
        items = sorted(h(pair) for pair in v.items())      # pair = (key, value) tuple
        return md5(str(sorted(md5(x) for x in items)))
    if isinstance(v, (list, tuple, set, frozenset)):
        return md5(str(sorted(h(x) for x in v)))           # str(list of hex) = "['ab..', 'cd..']"
    if isinstance(v, (type, types.FunctionType)):
        return md5(f'{v.__module__}.{v.__qualname__}')
    return md5(str(v))                                     # 'True', 'None', '1', raw str
```

A `(key, value)` pair is a 2-tuple, so its hash is `md5(str(sorted([h(key), h(value)])))`. Empty
list/dict ⇒ `md5('[]')`. `str()` of a list of hex strings is Python's repr:
`"['" + "', '".join(hexes) + "']"`. Verified: my reimplementation over the 62 probe values
reproduces `ee8c8458af746a6d4f6c98626c0842b6`; `h(['builder_html','format_html','html'])` =
`645f666f9bcd5a90fca523b33c5a78b7` (html tags hash for every project without `-t`/`tags.add`);
dirhtml tags hash `d77d1c0d9ca2f4c8421862c7c5a0d620` **[probed]**. The config hash is the same for
html and dirhtml (`1b5e8be02273988e884ffc0e8b95687a` for `feat/`).

### 13.3 What is hashed

`{c.name: c.value for c in config.filter(frozenset({'html'}))}` (`_build_info.py:56-58`;
`Config.filter`, `SPHINX/config.py:513-516`) — **every registered option whose `rebuild` is
`'html'`**, with callable defaults evaluated, values as they stand when `init()` starts
(after `config-inited` handlers: `html_css_files`/`html_js_files` are lists of
`(filename, attrs)` tuples, lists may have become tuples/frozensets via `check_confval_types`,
`copyright` has had `%Y` substituted, invalid static/extra paths removed, missing logo/favicon set
to `None`), **but before** `init_css_files`/`init_js_files` add `'priority': 800` to attrs dicts.
**[probed]** (`cfghash2.py`): at `create_build_info` time `html_css_files == [('custom.css', {}),
('print.css', {'media': 'print', 'priority': 100}), ('https://cdn.example.org/x.css', {})]` → hash
`1b5e8be0…` (= the file); hashing after init (`{'priority': 800}` added) gives `bf8b5cf1…` (wrong).

The 62 names for a default Sphinx 9.1 install (probe values in parentheses where not the default
shown) — registered by `sphinx.config` (5), `sphinx.builders.html` (40), `sphinx.ext.mathjax`
(9, always loaded by the html builder's setup), `sphinx.builders.singlehtml` (1),
`sphinxcontrib.htmlhelp` (2), `sphinxcontrib.qthelp` (4), plus `html4_writer`:

```
copyright ('2026, Tester')             project_copyright ('2026, Tester')   # aliases of each other
pygments_style=None  templates_path=[]  template_bridge=None  modindex_common_prefix=[]
html4_writer=False  html_additional_pages={}  html_baseurl=''  html_codeblock_linenos_style='inline'
html_compact_lists=True  html_context={}  html_copy_source=True  html_css_files=[]
html_domain_indices=True  html_extra_path=[]  html_favicon=None  html_file_suffix=None
html_js_files=[]  html_last_updated_fmt=None  html_last_updated_use_utc=False
html_link_suffix=None  html_logo=None  html_output_encoding='utf-8'  html_permalinks=True
html_permalinks_icon='¶'  html_scaled_image_link=True  html_search_language=None
html_search_options={}  html_secnumber_suffix='. '  html_short_title=<html_title>
html_show_copyright=True  html_show_search_summary=True  html_show_sourcelink=True
html_show_sphinx=True  html_sidebars={}  html_sourcelink_suffix='.txt'  html_split_index=False
html_static_path=[]  html_style=None  html_theme='alabaster'  html_theme_options={}
html_theme_path=[]  html_title='<project> <release> documentation'  html_use_index=True
html_use_opensearch=''
htmlhelp_file_suffix=None  htmlhelp_link_suffix=None
mathjax2_config=None (default: mathjax_config)  mathjax3_config=None  mathjax4_config=None
mathjax_config=None  mathjax_config_path=''  mathjax_display=['\\[', '\\]']
mathjax_inline=['\\(', '\\)']  mathjax_options={}
mathjax_path='https://cdn.jsdelivr.net/npm/mathjax@4/tex-mml-chtml.js'
qthelp_basename=make_filename(project) (re.sub(r'[^a-zA-Z0-9_-]', '', project) or 'sphinx')
qthelp_namespace=None  qthelp_theme='nonav'  qthelp_theme_options={}
singlehtml_sidebars={}
```

(`html_math_renderer` is `'env'`, not hashed.) User extensions add theirs (e.g.
`sphinx.ext.todo`: `todo_include_todos`, `todo_link_only`, `todo_emit_warnings`;
`sphinx.ext.graphviz`, `sphinx.ext.imgmath`, `autosummary_filename_map` are `'html'` too). The
crate therefore needs a registry of option name → rebuild class → default (with evaluated
lambdas) for everything that can be set, and must hash the conf.py value *as Python would
`str()` it*. Values that `-D`/`-A` override feed in too (`-A k=v` edits `html_context`).
`SOURCE_DATE_EPOCH` affects `copyright` (`correct_copyright_year`, `SPHINX/config.py:711-739`)
and `last_updated` — an oracle should leave it unset or replicate both.

---------------------------------------------------------------------------------------------------

## 14. Asset checksums (`SPHINX/builders/html/_assets.py:111-135`)

```python
def _file_checksum(outdir, filename):
    if '://' in filename: return ''
    if '?' in filename: raise ThemeError(f'Local asset file paths must not contain query strings: {filename!r}')
    return _file_checksum_inner(outdir.joinpath(filename).resolve())      # @cache'd per process
def _file_checksum_inner(file):
    content = file.read_bytes().translate(None, b'\r')    # FileNotFoundError → ''
    return '' if not content else f'{zlib.crc32(content):08x}'
```

CRC-32 is the standard zlib/IEEE polynomial (crc32fast / flate2's `Crc` compute the same value).
Probed values: `pygments.css` (none style) `8e8a900e`, `basic.css` (basic defaults) `29da98fa`,
`doctools.js` `fd6eb6e6`, `sphinx_highlight.js` `6ffebe34`, `documentation_options.js`
`f2a433a1` (depends on release/language/builder/suffixes — `789eb5fd`, `a417aaa8`, `7f41d439`,
`5929fcd5` in other probes).

---------------------------------------------------------------------------------------------------

## 15. `layout.html` walk-through (whitespace that falls out of the template)

`SPHINX/themes/basic/layout.html` (209 lines). Key facts an implementer must not "fix":

* `<!DOCTYPE html>` then an empty line (the `\n` before the `{# URL root … #}` comment survives),
  then `<html lang="{{ language }}" data-content_root="{{ content_root }}">` (the `lang`
  attribute is omitted when `language is none`; `html_tag` context var replaces the whole tag).
* `render_sidebar = not embedded and not theme_nosidebar|tobool and sidebars != []`.
* `titlesuffix = " &#8212; " + docstitle|e` unless `embedded` or empty `docstitle`.
* Head: `<meta charset="{{ encoding }}" />`, the layout's own viewport meta, `{{- metatags }}`,
  `<title>{{ title|striptags|e }}{{ titlesuffix }}</title>`, css(), (if not embedded) scripts(),
  canonical (`pageurl`), opensearch link, favicon; then `linktags`: `about`, `genindex` (`index`),
  `search`, `copyright`, `next`, `prev` (each only if `hasdoc`/present); then
  `{%- block extrahead %} {% endblock %}` which leaves a single space before `\n  </head>`.
* `</head><body>` on one line (`{%- block body_tag %}` strips).
* relbar macro: rellinks rendered in list order as `<li class="right" …>` (the first gets
  `style="margin-right: 10px"`, no delimiter; others end with ` |`), then
  `nav-item-0` → `pathto(root_doc)` with `shorttitle|e` and ` &#187;`, then one `nav-item-<n>` per
  parent (last parent gets `accesskey="U"`), then `nav-item-this` whose `href="{{ link|e }}"` is
  **always empty** (`link` is never defined in the context) — `<a href="">`, then a trailing space
  from `{%- block relbaritems %} {% endblock %}`.
* After the first relbar: `</div>` + two spaces (from `{%- block sidebar1 %} {# … #} {% endblock %}`).
* Footer: copyright via the macro (list/tuple copyright → one `&#169; Copyright <line>.` per item,
  `<br/>` between, with blank/indented lines **[probed]**), `Last updated on …`, `Created using …`.

### 15.1 Full `index.html`, basic theme, 2-doc project **[probed]** (`basic/out/index.html`, 5.9 KB, no final newline)

Source: `conf.py` = `project='Probe'; copyright='2026, Tester'; author='Tester'; release='1.0';
html_theme='basic'`; `index.rst` has a title, a paragraph, a toctree (`other`), two sections, a
`py:function` and an `index` directive; `other.rst` references `spam` and declares `py:module`.

```html
<!DOCTYPE html>

<html lang="en" data-content_root="./">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" /><meta name="viewport" content="width=device-width, initial-scale=1" />

    <title>Welcome to Probe &#8212; Probe 1.0 documentation</title>
    <link rel="stylesheet" type="text/css" href="_static/pygments.css?v=8e8a900e" />
    <link rel="stylesheet" type="text/css" href="_static/basic.css?v=29da98fa" />
    <script src="_static/documentation_options.js?v=f2a433a1"></script>
    <script src="_static/doctools.js?v=fd6eb6e6"></script>
    <script src="_static/sphinx_highlight.js?v=6ffebe34"></script>
    <link rel="index" title="Index" href="genindex.html" />
    <link rel="search" title="Search" href="search.html" />
    <link rel="next" title="Other Page" href="other.html" /> 
  </head><body>
    <div class="related" role="navigation" aria-label="Related">
      <h3>Navigation</h3>
      <ul>
        <li class="right" style="margin-right: 10px">
          <a href="genindex.html" title="General Index"
             accesskey="I">index</a></li>
        <li class="right" >
          <a href="py-modindex.html" title="Python Module Index"
             >modules</a> |</li>
        <li class="right" >
          <a href="other.html" title="Other Page"
             accesskey="N">next</a> |</li>
        <li class="nav-item nav-item-0"><a href="#">Probe 1.0 documentation</a> &#187;</li>
        <li class="nav-item nav-item-this"><a href="">Welcome to Probe</a></li> 
      </ul>
    </div>  

    <div class="document">
      <div class="documentwrapper">
        <div class="bodywrapper">
          <div class="body" role="main">
            
  <section id="welcome-to-probe">
<h1>Welcome to Probe<a class="headerlink" href="#welcome-to-probe" title="Link to this heading">¶</a></h1>
<p>Intro paragraph with <em>emphasis</em>.</p>
<div class="toctree-wrapper compound">
<ul>
<li class="toctree-l1"><a class="reference internal" href="other.html">Other Page</a><ul>
<li class="toctree-l2"><a class="reference internal" href="other.html#sub">Sub</a></li>
</ul>
</li>
</ul>
</div>
<section id="section-a">
<h2>Section A<a class="headerlink" href="#section-a" title="Link to this heading">¶</a></h2>
<p>Text A.</p>
<dl class="py function">
<dt class="sig sig-object py" id="spam">
<span class="sig-name descname"><span class="pre">spam</span></span><span class="sig-paren">(</span><em class="sig-param"><span class="n"><span class="pre">x</span></span></em><span class="sig-paren">)</span><a class="headerlink" href="#spam" title="Link to this definition">¶</a></dt>
<dd><p>Does spam.</p>
</dd></dl>

</section>
<section id="section-b">
<span id="index-0"></span><h2>Section B<a class="headerlink" href="#section-b" title="Link to this heading">¶</a></h2>
<p>Text B.</p>
</section>
</section>


            <div class="clearer"></div>
          </div>
        </div>
      </div>
      <div class="sphinxsidebar" role="navigation" aria-label="Main">
        <div class="sphinxsidebarwrapper">
  <div>
    <h3><a href="#">Table of Contents</a></h3>
    <ul>
<li><a class="reference internal" href="#">Welcome to Probe</a><ul>
<li><a class="reference internal" href="#section-a">Section A</a><ul>
<li><a class="reference internal" href="#spam"><code class="docutils literal notranslate"><span class="pre">spam()</span></code></a></li>
</ul>
</li>
<li><a class="reference internal" href="#section-b">Section B</a></li>
</ul>
</li>
</ul>

  </div>
  <div>
    <h4>Next topic</h4>
    <p class="topless"><a href="other.html"
                          title="next chapter">Other Page</a></p>
  </div>
  <div role="note" aria-label="source link">
    <h3>This Page</h3>
    <ul class="this-page-menu">
      <li><a href="_sources/index.rst.txt"
            rel="nofollow">Show Source</a></li>
    </ul>
   </div>
<search id="searchbox" style="display: none" role="search">
  <h3 id="searchlabel">Quick search</h3>
    <div class="searchformwrapper">
    <form class="search" action="search.html" method="get">
      <input type="text" name="q" aria-labelledby="searchlabel" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck="false"/>
      <input type="submit" value="Go" />
    </form>
    </div>
</search>
<script>document.getElementById('searchbox').style.display = "block"</script>
        </div>
      </div>
      <div class="clearer"></div>
    </div>
    <div class="related" role="navigation" aria-label="Related">
      <h3>Navigation</h3>
      <ul>
        <li class="right" style="margin-right: 10px">
          <a href="genindex.html" title="General Index"
             >index</a></li>
        <li class="right" >
          <a href="py-modindex.html" title="Python Module Index"
             >modules</a> |</li>
        <li class="right" >
          <a href="other.html" title="Other Page"
             >next</a> |</li>
        <li class="nav-item nav-item-0"><a href="#">Probe 1.0 documentation</a> &#187;</li>
        <li class="nav-item nav-item-this"><a href="">Welcome to Probe</a></li> 
      </ul>
    </div>
    <div class="footer" role="contentinfo">
    &#169; Copyright 2026, Tester.
      Created using <a href="https://www.sphinx-doc.org/">Sphinx</a> 9.1.0.
    </div>
  </body>
</html>
```

Output tree of that build (`find out -type f`, minus `.doctrees/`):

```
.buildinfo  genindex.html  index.html  objects.inv  other.html  py-modindex.html  search.html  searchindex.js
_sources/index.rst.txt  _sources/other.rst.txt
_static/base-stemmer.js  _static/basic.css  _static/doctools.js  _static/documentation_options.js
_static/english-stemmer.js  _static/file.png  _static/language_data.js  _static/minus.png
_static/plus.png  _static/pygments.css  _static/searchtools.js  _static/sphinx_highlight.js
```

(`sphinx-build src out` puts the doctree cache at `out/.doctrees/`; an output-tree oracle must
ignore that directory — the crate keeps its own cache elsewhere.)

### 15.2 Variants observed

* `other.html` (has prev, no next): `<link rel="prev" title="Welcome to Probe" href="index.html" /> `,
  rellink `accesskey="P">previous</a> |`, sidebar `Previous topic` block, and
  `nav-item-0` links to `index.html`.
* Deeper page with a parent (`deep/out/a/b.html`):
  `          <li class="nav-item nav-item-1"><a href="index.html" accesskey="U">A Index</a> &#187;</li>`
  (10-space indent).
* `html_use_index=False`, `html_domain_indices=False`, `html_copy_source=False` (`misc/`): no
  `rel="index"` link, no index/modules rellinks, no "This Page" block, `HAS_SOURCE: false`,
  no `genindex.html`/`py-modindex.html`; `display_toc` false for a single-section page ⇒ no
  "Table of Contents" block.
* `.. meta::` tags are appended after the docutils viewport tag (see §10.2).
* Logo (`feat/`): sidebar begins with
  `            <p class="logo"><a href="../index.html">\n              <img class="logo" src="../_static/logo.png" alt="Logo of Feat &amp; &#34;Q&#34;"/>\n            </a></p>`.
* List copyright + `html_last_updated_fmt='%Y'` footer **[probed]** (`$` = end of line):

```
    <div class="footer" role="contentinfo">$
    $
      &#169; Copyright 2020, A &lt;b&gt;.<br/>$
    $
      &#169; Copyright 2021, B.$
    $
      Last updated on 2026.$
      Created using <a href="https://www.sphinx-doc.org/">Sphinx</a> 9.1.0.$
    </div>$
```

---------------------------------------------------------------------------------------------------

## 16. Other builders and alabaster

### 16.1 DirectoryHTMLBuilder — only `get_target_uri` and `get_output_path` differ (§9.3); name
`dirhtml`, same templates, same finish tasks, same `.buildinfo` categories (different tags hash).

### 16.2 DummyBuilder (`SPHINX/builders/dummy.py:17-36`)

`init` no-op; `get_outdated_docs` = `env.found_docs`; `get_target_uri` = `''`; `write_doc` and
`finish` no-ops. The base `write()` still runs `prepare_writing` (no-op), `copy_assets` (no-op,
but prints `copying assets... ` / `copying assets: done`) and **`get_and_resolve_doctree` for every
doc** ("writing output... [ 50%] index"), so resolution warnings are emitted. No output files
(only the doctree cache); no `.buildinfo` **[probed]**.

### 16.3 What alabaster (the default theme) adds **[probed, alab/]**

* Theme config (`ALABASTER/theme.conf`, INI): `inherit = basic`, `stylesheet = basic.css,
  alabaster.css`, `sidebars = about.html, searchfield.html, navigation.html, relations.html,
  donate.html`, `pygments_style = alabaster.support.Alabaster`, ~110 string options (`page_width
  = 940px`, `sidebar_width = 220px`, `body_min_width = inherit`, `show_relbars = false`, colours…).
* Static: `alabaster.css_t` (102 `{{ }}` substitutions; rendered 10776 B), `custom.css`
  (`/* This file intentionally left blank. */`), `github-banner.svg`. `pygments.css` is the
  Alabaster Pygments style (5263 B, crc `5ecbeea2`); `basic.css` gets `min-width: inherit;`
  (crc `b08954a9`).
* Templates: `layout.html` (`{%- extends "basic/layout.html" %}`): `extrahead` adds
  `<link rel="stylesheet" href="{{ pathto('_static/custom.css', resource=True) }}" type="text/css" />`
  (no checksum), optional touch icon / canonical; **empties `relbar1`/`relbar2`**; optional
  fixed-sidebar content layout; own footer (`&#169;{{ copyright }}.` — note: no "Copyright" word,
  and a list copyright (turned into a tuple by `check_confval_types`) is printed as the raw,
  unescaped Python tuple repr **[probed, alab2/]**: `&#169;('2020, A <b>', '2021, B').`;
  `Powered by <a …>Sphinx 9.1.0</a>\n      &amp; <a
  href="https://alabaster.readthedocs.io">Alabaster 1.0.0</a>`, `Page source` link). Sidebars:
  `about.html` (`<h1 class="logo"><a href="{{ pathto(master_doc) }}">{{ project }}</a></h1>` when no
  `logo` option), `searchfield.html` (basic), `navigation.html` (`<h3>Navigation</h3>` +
  `toctree(includehidden=theme_sidebar_includehidden, collapse=theme_sidebar_collapse)` — global
  toc), `relations.html` ("Related Topics"/"Documentation overview"), `donate.html`.
* `update_context` html-page-context handler (`ALABASTER/__init__.py:17-30`).

Probed alabaster `index.html` head/sidebar/footer excerpts:

```html
    <link rel="stylesheet" type="text/css" href="_static/pygments.css?v=5ecbeea2" />
    <link rel="stylesheet" type="text/css" href="_static/basic.css?v=b08954a9" />
    <link rel="stylesheet" type="text/css" href="_static/alabaster.css?v=27fed22d" />
    …
    <link rel="next" title="Other Page" href="other.html" />
   
  <link rel="stylesheet" href="_static/custom.css" type="text/css" />
  

  
  

  </head><body>
  

    <div class="document">
…
        <div class="sphinxsidebarwrapper">
<h1 class="logo"><a href="#">Probe</a></h1>
…
<script>document.getElementById('searchbox').style.display = "block"</script><h3>Navigation</h3>
<ul>
<li class="toctree-l1"><a class="reference internal" href="other.html">Other Page</a></li>
</ul>

<div class="relations">
<h3>Related Topics</h3>
<ul>
  <li><a href="#">Documentation overview</a><ul>
      <li>Next: <a href="other.html" title="next chapter">Other Page</a></li>
  </ul></li>
</ul>
</div>
…
    <div class="footer">
      &#169;2026, Tester.
      
      |
      Powered by <a href="https://www.sphinx-doc.org/">Sphinx 9.1.0</a>
      &amp; <a href="https://alabaster.readthedocs.io">Alabaster 1.0.0</a>
      
      |
      <a href="_sources/index.rst.txt"
          rel="nofollow">Page source</a>
    </div>
```

Alabaster needs, beyond basic: the global `toctree()` callable (so `global_toctree_for_doc` +
`_resolve_toctree` with `collapse`/`includehidden` string-truthiness), the Alabaster Pygments CSS,
`alabaster.css_t` rendering, and Python tuple `str()` for list copyrights. Everything else is
template work on the same engine.

### 16.4 Other built-in themes

All inherit `basic` (`default` → `classic` → `basic`); each is CSS + at most a `layout.html`
override; pygments defaults: agogo/nature/scrolls `tango`, bizstyle/sphinxdoc `friendly`, classic
`sphinx`, haiku `autumn`, pyramid `sphinx.pygments_styles.PyramidStyle`, epub/nonav `none`,
traditional unset (→ `none`). Cheap to add once basic is exact.

---------------------------------------------------------------------------------------------------

## 17. HTML config values (`SPHINX/builders/html/__init__.py:1454-1530`) — default, rebuild, types

| Name | Default | Rebuild | Types |
|---|---|---|---|
| html_theme | `'alabaster'` | html | str |
| html_theme_path | `[]` | html | list, tuple |
| html_theme_options | `{}` | html | dict |
| html_title | `_('%s %s documentation') % (project, release)` | html | str |
| html_short_title | `html_title` | html | str |
| html_style | `None` | html | list, str, tuple |
| html_logo / html_favicon | `None` | html | str |
| html_css_files / html_js_files | `[]` | html | list, tuple |
| html_static_path / html_extra_path | `[]` | html | list, tuple |
| html_last_updated_fmt | `None` | html | str |
| html_last_updated_use_utc | `False` | html | bool |
| html_sidebars | `{}` | html | dict |
| html_additional_pages | `{}` | html | dict |
| html_domain_indices | `True` | html | frozenset, list, set, tuple (and bool) |
| html_permalinks | `True` | html | bool |
| html_permalinks_icon | `'¶'` | html | str |
| html_use_index | `True` | html | bool |
| html_split_index | `False` | html | bool |
| html_copy_source | `True` | html | bool |
| html_show_sourcelink | `True` | html | bool |
| html_sourcelink_suffix | `'.txt'` | html | str |
| html_use_opensearch | `''` | html | str |
| html_file_suffix / html_link_suffix | `None` | html | str |
| html_show_copyright / html_show_search_summary / html_show_sphinx | `True` | html | bool |
| html_context | `{}` | html | dict |
| html_output_encoding | `'utf-8'` | html | str |
| html_compact_lists | `True` | html | bool |
| html_secnumber_suffix | `'. '` | html | str |
| html_search_language | `None` | html | str |
| html_search_options | `{}` | html | dict |
| html_search_scorer | `''` | `''` | str |
| html_scaled_image_link | `True` | html | bool |
| html_baseurl | `''` | html | str |
| html_codeblock_linenos_style | `'inline'` | html | ENUM('table','inline') |
| html_math_renderer | `None` | env | str, None |
| html4_writer | `False` | html | bool (True ⇒ ConfigError) |

Events registered: `html-collect-pages`, `html-page-context`.

`_get_sidebars(pagename)` (`:1040-1060`): start with `theme.sidebar_templates`; for each
`(pattern, list)` in `html_sidebars` (dict order) that `patmatch`es the page
(`SPHINX/util/matching.py:94-100`, Sphinx's glob → regex, `*` does not cross `/`): if a previous
match exists and this pattern has wildcards — warn `page %s matches two patterns in html_sidebars:
%r and %r` only if the previous one also had wildcards, and keep the previous; otherwise take this
one. `_has_wildcard` = any of `*?[`.

---------------------------------------------------------------------------------------------------

## 18. Crate integration points and gaps (as of `f353db9`)

| Concern | Crate location | State |
|---|---|---|
| Placeholder render | `src/builder.rs:874-880` | `format!("<html><body>{}</body></html>", escaped source)` done in the **read** phase and cached |
| Write phase | `src/builder.rs:1459-1478` (`write_phase`, `write_one`) | writes `Document.html` in parallel; no finish tasks |
| Resolver URIs | `src/builder.rs:1303-1307` | `relative_uri = \|_, _\| String::new()` (dummy semantics) |
| genindex URIs | `src/builder.rs:1255-1262` | `rel_uri = \|_\| Some("")`; runs after the build, not in finish |
| genindex data | `src/env/genindex.rs:355` `create_index(env, rel_uri, messages)` | ready; needs `get_relative_uri('genindex', d)` |
| relations | `src/env/toctree.rs:714` `collect_relations` | ready |
| titles / longtitles / tocs / toc_num_entries | `src/env/mod.rs:169-174` | ready (Node trees for `render_partial`) |
| images / dlfiles | — | **missing** in `BuildEnvironment` (`src/env/mod.rs:146-200`) |
| objects.inv writer | `src/inventory.rs:333-…` `InventoryFile::dump` | ready, no call site |
| HTMLBuilder scaffold | `src/html_builder.rs` (788 lines) | M1 scaffold, never invoked; not Sphinx-shaped |
| Templates | `src/template.rs` (387), `templates/*.html`, `static/*` | wrong escaper/striptags, non-upstream templates and JS |
| `relative_uri` | `src/utils.rs:563-581` | not Sphinx's algorithm |
| `format_date` | `src/utils.rs:532` | exists (check against `SPHINX/util/i18n.py:177-…` mapping for `last_updated`) |
| HTML config | `src/config.rs:57-146, 334, 411-479`; `src/python_config.rs:31-…` | defaults diverge (§0.8); many options absent (`html_split_index`, `html_domain_indices`, `html_baseurl`, `html_sidebars`, `html_file_suffix`, `html_additional_pages`, `html_extra_path`…); `html_css_files`/`html_js_files` are `Vec<String>` (no attrs tuples) |
| Deps | `Cargo.toml:73,76` | `flate2`, `minijinja 2.12 (lock: 2.24.0) features=["loader"]` — no md5/crc crate yet (flate2 exposes `Crc`; md5 needs a crate or a small impl) |

---------------------------------------------------------------------------------------------------

## 19. Recommendations for wave 5

1. **Vendor upstream bytes** with a `tools/` script (like the other generators): the 17 basic
   templates + 9 static files, `language_data.js` rendered for `en`, the two non-minified stemmer
   files, and a table of pygments CSS per style name. Store the SHA of each against Sphinx 9.1.0
   so drift is caught. Rewrite `{% trans %}` mechanically in the script (§5.3) and assert the
   rewritten template renders byte-identically under real Jinja2 for a set of contexts.
2. **Template environment**: minijinja with `set_auto_escape_callback(|_| AutoEscape::None)`,
   `set_keep_trailing_newline(false)`, a Python-`str()` formatter, and filters/globals:
   `e`/`escape` (markupsafe), `striptags` (markupsafe + `html.unescape`), `tobool`, `toint`,
   `todim`, `slice_index`, `_`/`gettext`/`ngettext`/`_trans`, `accesskey` (per-render temp
   state), `idgen`, `warning`, `debug`; a loader implementing the §3.3 chain (`!` prefix, parent
   dirs, `_t` fallback). Per-page closures (`pathto`, `hasdoc`, `toctree`, `css_tag`, `js_tag`)
   go into the context as function values.
3. **Order of operations** exactly as §1.2/§1.4: prepare_writing → copy_assets (downloads,
   static incl. rendered templates, extra) → pages (sorted) → genindex(+split) → py-modindex →
   additional pages/search/opensearch → images → .buildinfo → searchindex.js → objects.inv.
   Checksums must be computed after `copy_assets`.
4. **URIs**: port `relative_uri`, `quote`, the two `get_target_uri`/`get_output_path` pairs, and
   feed the html builder's `get_relative_uri` into the resolver and genindex (replacing the dummy
   closures at `src/builder.rs:1256-1260, 1303-1307`). This will change resolved doctrees relative
   to the current dummy-builder env oracle — plan a separate html-builder resolved-doctree
   snapshot or accept that the env oracle keeps using `dummy`.
5. **.buildinfo**: implement `stable_hash` over a typed Python-value model (`Str`, `Bool`, `None`,
   `Int`, `Float`, `List`, `Tuple`, `Dict`, …) with Python `str()` for leaves; build the 62-name
   `html`-category table with evaluated defaults; hash css/js entries *without* the injected
   `priority`. Differential-test the hash against the oracle's `.buildinfo` for every fixture
   project (it is the cheapest byte-exact check in the whole wave).
6. **Oracle design**: build each fixture with `-b html` (and a subset with `-b dirhtml`) under
   `html_theme = 'basic'`, record the full output tree (path → bytes, or hash + text for HTML),
   excluding `.doctrees/` and — until M3 — `searchindex.js` bytes; normalise the absolute srcdir
   path in stdout lines only. Leave `SOURCE_DATE_EPOCH` unset and avoid
   `html_last_updated_fmt` (or pin it and replicate `format_date`). Keep projects on `language =
   'en'` (translations are M7).
7. Warnings emitted by the write side, in the order they appear in a real build:
   config-inited validations (§2.5), `unsupported theme option`, per-page write warnings (buffered
   until after the last "writing output" line), genindex messages (during finish), image copy
   failures, `.buildinfo` read/write failures.

---------------------------------------------------------------------------------------------------

## 20. Open questions / risks

* minijinja behaviours asserted from source reading (top-level `set` in child templates visible in
  parent blocks, output before `extends`, nested unpacking, `{% include var %}`, `attr` on a map)
  should be pinned by unit tests before relying on them — I did not run cargo (another build was
  in progress).
* `striptags` needs a faithful `html.unescape` (HTML5 entity table incl. legacy no-semicolon
  names and the invalid-codepoint replacement rules); titles normally only contain `&amp; &lt;
  &gt; &quot; &#…;`, but `.. title::` text and user `html_context` values can contain anything.
* `.buildinfo` requires modelling Python `str()` of arbitrary conf.py values (dict reprs,
  float reprs, tuples, functions → `module.qualname`); the crate's conf.py parser only handles a
  literal subset (`src/python_config.rs:8-15`), so hash parity is limited to that subset.
* Sphinx's `_file_checksum_inner` is `@cache`d per process; irrelevant for one-shot builds but a
  future `serve` mode must not copy that staleness.
* `html_static_path` default and quickstart projects: the missing-`_static` warning is emitted by
  Sphinx; the crate's current `['_static']` default would both suppress the warning and change
  the `.buildinfo` hash.
* The ROADMAP's M3 note that English search uses classic Porter is stale for Sphinx 9.1 (Snowball
  English / Porter2 on both Python and JS sides).
