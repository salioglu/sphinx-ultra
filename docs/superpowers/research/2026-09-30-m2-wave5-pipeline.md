# M2 wave 5 research note — build pipeline & write path (key: `pipeline`)

Scope: how `SphinxBuilder::build` is wired today, where doctrees live, how the
placeholder write path and the incremental cache interact, how warnings flow,
what the output tree looks like, which tests pin the placeholder, and which
HTML-relevant config keys exist. Everything below was read from the tree at
`2ed9eaf` (v0.5.0) plus the pinned upstream sources:

- `SPHINX` = `/root/.cache/uv/archive-v0/b4dBDAdEzskuqge1iT52j/lib/python3.12/site-packages/sphinx`
- `DOCUTILS` = same dir `/docutils`

Every "Sphinx does X" claim that is not a source citation was **probed** with
real Sphinx 9.1.0 / docutils 0.22.4 (`PYTHONNOUSERSITE=1 uv run --python 3.12
--with sphinx==9.1.0 --with docutils==0.22.4 python -m sphinx ...`); probe
projects live under `scratchpad/probe-pipeline/p1..p5` (scripts `probe1.sh`,
`probe2.sh`, `probe3.py`). Transcripts are quoted in §3.5, §4.6 and §7.3.

---

## 0. TL;DR for implementers (the load-bearing findings)

1. **Resolved doctrees are thrown away.** `xref_phase` (`src/builder.rs:1294-1359`)
   clones each doctree, resolves it, stores only `doctree.root.pformat()` in
   `self.resolved: Mutex<BTreeMap<String, String>>` (`:138`, `:1344-1347`) and
   drops the tree. A writer has no resolved doctree to consume today.
2. **The resolver is wired with dummy-builder URIs.** `relative_uri = |_, _| String::new()`
   (`:1309`) and genindex `rel_uri = |_| Some(String::new())` (`:1260`) —
   deliberately Sphinx's `DummyBuilder.get_target_uri == ''`
   (`SPHINX/builders/dummy.py:29-30`) because the env oracle is a dummy build.
   HTML needs `get_relative_uri(from,to) = relative_uri(quote(from)+'.html',
   quote(to)+'.html')` (`SPHINX/builders/__init__.py:189-197`,
   `SPHINX/util/osutil.py:46-66`, `SPHINX/builders/html/__init__.py:1067-1068`).
   The `snapshot_env()` contract (`resolved_pformat`, `genindex`) that
   `tests/env_differential.rs` diffs is *dummy-URI* output, so the builder needs
   a builder-kind switch (`html` vs `dummy`) rather than a URI swap.
3. **Rendered HTML is produced in the READ phase and cached.** `read_one_file`
   renders the placeholder into `Document.html` (`src/builder.rs:874-881`), the
   cache persists it (`src/cache.rs:172-202`), and a cache hit is only accepted
   when `!cached.html.is_empty()` (`src/builder.rs:844`). **If the new writer
   stops filling `Document.html`, every cache lookup is rejected and the
   incremental cache never hits again** — every `Cache hits: N` assertion in
   the e2e suite and the builder unit tests would fail. Remove that clause (and
   stop caching rendered output: a page depends on *other* documents).
4. **Sphinx's HTML write set is NOT "every document".** Write set =
   `get_outdated_docs()` (target html older than source/newest template, or
   missing, or doc not in `all_docs`; everything on `.buildinfo` mismatch) ∪
   docs read this build ∪ docs whose numbering changed ∪ direct toctree
   containers (`files_to_rebuild`) of those. **Neighbours whose prev/next or
   whose linked titles changed are NOT rewritten** (probed: `b.html` keeps no
   `rel="next"` after `c` is appended; keeps prev title "A" after `a` is
   retitled). The crate today writes every found doc (= `DummyBuilder`
   semantics, `SPHINX/builders/dummy.py:26-27`). Pick deliberately (§3.6).
5. **Warm-build warnings follow the write set.** Sphinx html on a no-change
   rebuild emits *no* resolution warnings (nothing is written); Sphinx dummy
   re-emits them (probed). The crate re-emits them every build, and
   `tests/env_differential.rs:2506-2610`
   (`a_warm_rebuild_reports_the_same_std_domain_warnings`) pins that — i.e. it
   pins dummy semantics.
6. **`BuildWarning` cannot express docutils reporter output.** It has no level
   (always renders `WARNING:`, `src/error.rs:134-151`), and the reporter
   channel needs `ERROR:` (level 3) and `CRITICAL:` (level 4 = SEVERE),
   multi-line bodies, a `[docutils]` category, and **emission-order
   interleaving** with the toctree/log records (Sphinx prints docutils messages
   while parsing; probe §4.6). The crate replays toctree warnings, then log
   warnings, per document (`src/builder.rs:1125-1172`) — no shared sequence.
7. **The M1 directive validator will double-report** once the reporter channel
   prints docutils messages: `.. note::` with no body gives both the crate's
   `Note directive requires content` and docutils' `ERROR: Content block
   expected for the "note" directive; none found. [docutils]` (probed).
   `tests/e2e_cli.rs:1302-1334` pins the M1 text.
8. **Config defaults are not Sphinx's** and will leak straight into HTML:
   `project="Sphinx Ultra Project"`, `version=release=Some("1.0.0")`,
   `copyright=Some("2024, Sphinx Ultra")`, `html_theme="sphinx_rtd_theme"`,
   `html_style=["sphinx_rtd_theme.css"]`, `html_last_updated_fmt=Some("%b %d, %Y")`
   (`src/config.rs:387-474`, `:476-487`). Sphinx: `'Project name not set'`,
   `''`, `''`, `''`, `'alabaster'`, `None`, `None` (probe §7.3). With a conf.py
   that sets only `project='P'` Sphinx renders `<title>Index &#8212; P  documentation</title>`
   (two spaces), footer `&#169;.`, `VERSION: ''`, `# Version: ` in objects.inv.
   Also `html_last_updated_fmt` from conf.py is **never read** — the mapper
   reads `html_context["last_updated"]` instead (`src/python_config.rs:1243-1247`).
   ~20 HTML keys are missing from `BuildConfig` entirely (§7.2), so `-D
   html_permalinks=0` currently prints `unknown config value ... ignoring` and
   counts toward `-W`.
9. **Discovery does not exclude `**/_sources`, `templates_path`,
   `html_static_path`, `html_extra_path`** (Sphinx does:
   `SPHINX/environment/__init__.py:489-495`, `SPHINX/project.py:20`). Once the
   writer copies `_sources/<doc>.rst.txt`, an output dir inside the source dir
   makes the crate (which discovers `.txt`) read its own `_sources` copies as
   documents.
10. **Static assets today are M1 leftovers**: five non-Sphinx files
    (`static/*.{css,js}`) plus a verbatim copy of `<srcdir>/_static` **and
    `<srcdir>/_templates`** into the output (`src/builder.rs:1551-1636`).

---

## 1. Phase order in `SphinxBuilder::build`

### 1.1 Before `build()` — `main.rs::run_build` (`src/main.rs:271-347`)

| # | Step | Where | Notes |
|---|---|---|---|
| a | Load config: `--config FILE` → `BuildConfig::from_file`; `-c DIR` → `DIR/conf.py` (bail `config directory doesn't contain a conf.py file` if absent); else `BuildConfig::auto_detect(source)` (conf.py → sphinx-ultra.yaml → .yml → .json → `Default`) | `main.rs:272-290`, `config.rs:513-586` | conf.py dropped-construct warnings are `log::warn!` at parse time (`config.rs:545-552`) and are NOT counted/written to `-w`. |
| b | `-D key=value` → `config.apply_override` (ignored key ⇒ warning string pushed to `config_warnings`, logged with `warn!`) | `main.rs:295-301`, `config.rs:692-810` | |
| c | `-A name=value` → `html_context[name] = Value::String(value)` | `main.rs:302-306` | Sphinx int-coerces `-A` values (`SPHINX/cmd/build.py:379-387`: `with suppress(ValueError): val = int(val)`). Divergence: crate always stores strings. |
| d | `-t` tags appended; `-n` → `nitpicky`; `-d` → `doctree_dir`; `-W` → `fail_on_warning` | `main.rs:307-316` | |
| e | `config.validate()` (config-inited checks) → more `config_warnings` | `main.rs:321-324`, `config.rs:612-668` | No HTML validators exist (Sphinx has 6, §7.4). |
| f | `SphinxBuilder::new(config, source, output)` | `main.rs:329`, `builder.rs:230-303` | Cache dir = `config.doctree_dir` or `output/.sphinx-ultra-cache`; `BuildCache::new` wipes it on fingerprint mismatch (`cache.rs:48-58`); env loaded from `env.bin`; source dir canonicalized; `Parser::new(&config).with_srcdir(..)`; extension stubs loaded. |
| g | `-j` → `set_parallel_jobs`; `--clean` → `builder.clean()` (removes whole output dir + clears cache + fresh env, `builder.rs:409-419`); `-E` → `fresh_env()` (`builder.rs:385-389`); incremental → `enable_incremental()` | `main.rs:331-345` | compat mode: `incremental = !sb.write_all` (`main.rs:567`), i.e. `-a` switches the *read* cache off (documented divergence `main.rs:554-566`). Native mode: incremental only with `--incremental`. |

### 1.2 `SphinxBuilder::build` (`src/builder.rs:439-511`)

| # | Phase | Code | Consumes | Produces | Warnings/errors emitted (in push order) |
|---|---|---|---|---|---|
| 1 | create output dir | `:444` | `output_dir` | dir | — |
| 2 | **discover** | `discover_source_files` `:513-555` → `matching::get_matching_files(src, include_patterns, exclude_patterns + built-ins)` then `is_source_file` (suffix ∈ `rst`,`md`,`txt`, `:187`) then `dedup_by_docname` `:591-649` | config patterns | `Vec<PathBuf>` (absolute, canonical; discovery order) | `multiple files found for the document ...` (bare, no location) only for ≥2 `.rst` colliding — unreachable today. Built-in excludes: `_build/**`, `__pycache__/**`, `.git/**`, `.svn/**`, `.hg/**`, `.*/**`, `Thumbs.db`, `.DS_Store` (`:520-529`). |
| 3 | **intersphinx** | `load_intersphinx_inventories` `:317-367` | `intersphinx_mapping` etc. | `self.intersphinx` | bare `WARNING: ...` per failed inventory (`:345-355`); `ConfigError` aborts (`?`). |
| 4 | take env | `:452` `std::mem::take(&mut self.env)` | | `env` local | |
| 5 | **plan_read** | `:690-759` | env, files | `BTreeSet<docname>` to read | `info!("updating environment: {} added, {} changed, {} removed")` (incremental only). Non-incremental ⇒ all found docs. Adds toctree containers of removed docs (deliberate divergence `:738-756`). |
| 6 | **read_phase** (parallel, rayon pool of `parallel_jobs`) | `:767-818` → `read_one_file` `:829-899` | files, `to_read`, `found_docs` | `Vec<ReadResult>` in discovery order (`ReadResult{docname, document, doctree, read_time_us}` `:81-94`) | per-file failures → `BuildErrorReport(ErrorType::ParseError)` (`:807-812`). |
| 7 | **merge_phase** (sequential, docname order, only `read_time_us.is_some()`) | `:917-1105` | results (mut) | env mutated; doctrees persisted | per doc: `report_parse_warnings` (toctree warnings then log warnings, `:1125-1172`) → `env_genindex::process_doc` warnings (`:1053-1063`) → `env_std::process_doc` warnings (`:1065-1079`); doctree store failure → error (`:1095-1103`). |
| 8 | **resolve_phase** | `:1193-1246` | env, results | numbering, relations, resolved pformats, genindex, modindex; `env.bin` saved | `number_phase` warnings (`:1375-1402`, `report_numbering_warning` `:1409-1440`) → `check_consistency` warnings (orphans; multi-parent is `info!`) `:1217-1233` → `xref_phase` warnings per doc in docname order `:1294-1359` (+ one `info!` cross-domain count) → `genindex_phase` messages `:1255-1272`; `py_modindex_phase` `:1281-1284` (none). `env.save` failure is `log::warn!` only (not counted) `:1239-1245`. |
| 9 | put env back | `:458` | | `self.env` | |
| 10 | unzip results | `:470-473` | read_results | `processed_docs: Vec<Document>`, `doctrees: Vec<Doctree>` (discovery order) | |
| 11 | **write_phase** (parallel, **global** rayon pool — does not honour `-j`) | `:1459-1470` → `write_one` `:1472-1479` | `processed_docs` | `<output>/<relpath>.html` = `document.html` | write failure → `BuildErrorReport(ErrorType::Other)`. |
| 12 | validation (if `validate_directives`, default true) | `:478-480` → `:1654-1765` | docs + doctrees (for source table) | — | M1 validator warnings, discovery order (crate-only pass; Sphinx has none). |
| 13 | `generate_indices` | `:1545-1549` | — | nothing (TODO) | — |
| 14 | `copy_static_assets` | `:1551-1613` | exe-relative `static/`, `<src>/_static`, `<src>/_templates` | `<output>/_static/*`, `<output>/_static/**` (source), `<output>/_templates/**` | — |
| 15 | `generate_search_index` | `:1767-1771` | — | nothing (TODO) | — |
| 16 | stats | `:491-507` | | `BuildStats{files_processed = all docs, files_skipped = cache-hit docs, cache_hits = cache.hit_count(), warnings, errors, warning_details, error_details, output_size_mb (walks the output dir INCLUDING the cache dir)}` | |

### 1.3 The same pipeline in Sphinx (for ordering comparisons)

`Builder.build` (`SPHINX/builders/__init__.py:389-466`):

1. `read()` inside `logging.pending_warnings()` (`:402-408`) — buffered, flushed
   in order when the read ends. `read()` (`:469-577`): `find_files`,
   `get_outdated_files`, `env-get-outdated` event, glob-toctree re-read
   (`:487-489`), `updating environment: [reason] N added, N changed, N removed`,
   `clear_doc` removed docs, `_read_serial(sorted(added|changed))` (`:579-590`)
   — each `read_doc` parses, stores `all_docs[docname]`, `write_doctree` pickles
   (`:632-703`). docutils reporter messages and directive `logger.warning` calls
   are emitted **during** each doc's parse (interleaved, see §4.6).
2. `env.check_dependents(app, updated)` (`:411`; `environment/__init__.py:556-562`)
   = `env-get-updated` → `TocTreeCollector.get_updated_docs` (the numbering
   passes) → docs whose section/figure numbers changed are **added to the write
   set**. Numbering warnings are emitted here.
3. If anything was updated: pickle env, `check_consistency()` (orphan warnings).
4. `write(docnames, updated_docnames, method)` (`:705-748`):
   - `build_docnames` = `get_outdated_docs()` result (update mode) or all found
     docs (`-a`/`build_all`, or `['__all__']`);
   - `docnames = build_docnames ∪ updated_docnames` (update mode);
   - `docnames |= {c for d in docnames for c in env.files_to_rebuild.get(d, ()) if c in found_docs}` — **one level**, not transitive (`:725-730`);
   - `env.toctree_includes = dict(sorted(...))`;
   - `prepare_writing(docnames)`; `copy_assets()`; `write_documents(docnames)` →
     `_write_serial(sorted(docnames))` inside `pending_warnings()`
     (`:764-777`) → `_write_docname` (`:877-890`) = `get_and_resolve_doctree`
     (post-transforms incl. `ReferencesResolver`, then `_resolve_toctree` for
     every toctree node, `environment/__init__.py:668-712`) →
     `write_doc_serialized` (images, search index) → `write_doc`.
5. `finish()` (html: `gen_indices` → `gen_pages_from_extensions` →
   `gen_additional_pages` → `copy_image_files` → `write_buildinfo`; then
   `handle_finish` = `dump_search_index`, `dump_inventory`,
   `SPHINX/builders/html/__init__.py:675-684`, `1264-1267`).

Consequences for ordering parity:
- crate xref warnings come out in resolve_phase over **all** docs in docname
  order; Sphinx html emits them per **written** doc, sorted.
- crate genindex messages come after xref warnings (resolve_phase); Sphinx's
  come in `finish()` (after all doc writes) — same relative order.
- crate validation warnings (step 12) have no Sphinx counterpart.

---

## 2. Doctrees: production, persistence, resolution

### 2.1 Shapes

- `Doctree { root: Node, sources: Vec<String> }` (`src/doctree/mod.rs`); `Node
  { kind: &'static str, span: Span{source:u16,line:u32,start:u32,end:u32}, text:
  Option<String>, attrs: Attrs{ids,names,dupnames,classes,backrefs,extra:
  Vec<(&'static str, AttrValue)>}, children }`, `AttrValue = Int|Str|List`.
  `sources[0]` is the document path as a string (`file_path.display()`,
  absolute/canonical in builds); includes append entries.
- `ReadResult { docname, document: Document, doctree: Doctree, read_time_us:
  Option<u64> }` (`src/builder.rs:81-94`). `read_time_us == None` ⇔ cache hit.
- `Document` (`src/document.rs:22-83`): `source_path`, `output_path` (= source
  path with `.html`, **inside the source dir**, set by `Parser::get_output_path`
  `src/parser.rs:224-228`, unused for writing), `title` (first section title
  `astext()` or `"Untitled"`), `content: DocumentContent` (raw source text in
  `RstContent.raw`), `metadata`, **`html: String`**, `source_mtime`,
  `build_time`, `cross_refs`, `toc: Vec<TocEntry>` (flat, docutils ids),
  `toctrees: Vec<ToctreeRecord>`, `directive_records`, `role_records`,
  `labels`, `registry: RegistryExport` (nameids, program_options,
  std_objects, py_objects, py_modules, `log_warnings`, dependencies, included,
  index_serial; `src/rst/mod.rs:253-320`).

### 2.2 Lifecycle of the per-document doctree

1. Parsed in `read_one_file` (`:867-871`, `Parser::parse_full` →
   `rst::parse_rst_full`, `src/parser.rs:82-180`). `.md`/`.txt` get an empty
   `document` doctree (`src/parser.rs:236-241`).
2. **Not persisted at read time** (`:883-886`): the merge hooks mutate it
   (`env_genindex::process_doc` removes an invalid `index` node).
3. `merge_phase` persists it **after** the domain hooks (`:1095-1103`) via
   `store_doctree` (`:1494-1506`):
   `<cache_dir>/doctrees/<blake3(docname) hex>.doctree` (`doctree_path`
   `:1482-1488`) = `b"SUDT"` + `DOCTREE_FORMAT_VERSION: u32` LE (currently **2**,
   `:64`) + `crate::doctree::to_bincode(doctree)`. On failure the file is
   deleted and a `BuildErrorReport` recorded.
4. `load_doctree` (`:1513-1533`) returns `None` for missing file, wrong
   magic/version (`current_format_doctree` `:1813-1820`), or a bincode error ⇒
   caller re-reads. Callers: the cache-hit acceptance closure (`:853`) and the
   fallback loaders in `number_phase` (`:1380-1385`) and `xref_phase`
   (`:1299-1304`) for docnames not in memory.
5. **All** doctrees (read or cache-hit) stay in memory in `read_results`
   through the whole build; `build()` unzips them into `doctrees` for the
   validation pass (`:470-473`).
6. What is persisted is Sphinx's pickled doctree equivalent: post-read,
   pre-resolution. system_message nodes stay in (Sphinx's `FilterSystemMessages`,
   `SPHINX/transforms/__init__.py:337-347`, removes all of them at read time
   unless `keep_warnings`; the crate never strips — see §4.5).

Bump rules (`:52-60`): bump `DOCTREE_FORMAT_VERSION` if the serialized shape
or the *meaning* of stored attributes changes (e.g. wave 5 adding `image
[candidates]`, stripping system messages at read time, applying
`PropagateTargets` in-tree). Bump `ENV_VERSION` (`src/env/mod.rs:73`, now 3)
for any new `BuildEnvironment` field (wave 5 will need `images`, `dlfiles`,
possibly `toc_num_entries` already exists). Cache JSON `Document` is **not
versioned**: serde ignores unknown fields, so *removing* `html` still decodes
old entries; *adding* a non-`#[serde(default)]` field makes old entries miss.

### 2.3 Resolution today (`resolve_phase`, `:1193-1246`)

- `number_phase` (`:1375-1402`): `env_numbers::assign_section_numbers(env,
  &load_doctree) -> SectionNumbering{changed: Vec<String>, warnings}` and
  `assign_figure_numbers(env, numfig, numfig_secnum_depth, &load_doctree) ->
  Vec<String>` (`src/env/numbers.rs:66-87`, `:305-310`). The two `changed`
  lists are Sphinx's `rewrite_needed` — **only logged** (`:1397-1401`), because
  the crate writes everything. A Sphinx-semantics write set must union them in.
- `check_consistency` (`src/env/toctree.rs:857-931`).
- `xref_phase` (`:1294-1359`):
  ```rust
  let relative_uri = |_from: &str, _to: &str| String::new();          // :1309
  let resolver = env_resolve::Resolver { env, numfig, numfig_format,
      doctree: &load_doctree, relative_uri: &relative_uri, intersphinx };
  ...
  for result in ordered /* docname order */ {
      let mut doctree = result.doctree.clone();
      let resolution = env_resolve::resolve_document(&resolver, &nitpick,
          &result.docname, &mut doctree, &result.document.source_path);
      ... push warnings ...
      self.resolved.lock().unwrap()
          .insert(result.docname.clone(), doctree.root.pformat());  // tree dropped
  }
  ```
  `resolve_document` (`src/env/resolve.rs:777-801`) replaces `pending_xref`s
  (`ReferencesResolver`) and applies `PropagateDescDomain`. It does **not** do
  toctree resolution (`_resolve_toctree`), `HighlightLanguageTransform`,
  image post-processing, `FilterSystemMessages`, or any other post-transform.
  `Resolver.relative_uri` is consumed at `src/env/resolve.rs:439`, `:442`,
  `:591` (doc/ref/numref/any refuris).
- `genindex_phase` (`:1255-1272`): `create_index(env, &|_| Some(String::new()),
  &mut messages)` — dummy URIs (`#target`).
- `py_modindex_phase` (`:1281-1284`): data only.
- `snapshot_env()` (`:1783-1807`) = `env.snapshot()` (includes `relations`
  computed by `toctree::collect_relations`, `src/env/mod.rs:473-484`) plus
  `resolved_pformat`, `genindex`, `py_modindex`. This is what
  `tests/env_differential.rs` compares (13 `SphinxBuilder::new` call sites
  there, 10 `snapshot_env()` calls).

### 2.4 Where the writer gets a resolved doctree (options)

The oracle (`tools/gen_env_fixture.py:1642-1745`) captures
`resolved_pformat` by overriding `DummyBuilder.write_doc`: i.e. the output of
`get_and_resolve_doctree` (post-transforms **and** toctree resolution) under
dummy URIs, for **every** found doc (dummy's write set).

Option A — keep resolution in the resolve phase, keep the trees:
change `resolved` to `Mutex<BTreeMap<String, Doctree>>` (pformat lazily in
`snapshot_env`), add toctree resolution there, and parametrize the URI
closures by builder kind. Memory: two full copies of every doctree.

Option B (Sphinx-shaped, recommended) — move per-document resolution into the
write phase exactly like `_write_docname`: for `docname in sorted(write_set)`:
clone the in-memory doctree → post-transforms (xref resolve with the
builder's `get_relative_uri`, highlight stamp, `FilterSystemMessages`, ...) →
`_resolve_toctree` → translate → template → write. Run the per-doc work in
parallel but collect `(docname, warnings, output)` and push warnings in sorted
docname order after the join (Sphinx's `_write_parallel` resolves in the main
process in sorted order, `SPHINX/builders/__init__.py:779-818`, so its warning
order equals the serial one). The `dummy` builder kind does the same loop with
`write_set = found_docs` (`DummyBuilder.get_outdated_docs`) and a no-op
translator, and records `pformat` for `snapshot_env` — which keeps
`env_differential` byte-identical. genindex: compute once in `finish()` with
the builder's `get_relative_uri('genindex', docname)` (html: from
`genindex.html` at the root, e.g. `sub/a.html`) — computing it twice would
double-emit `create_index` messages.

Either way `env.save` stays where it is (resolution is read-only over the
env: `Resolver { env: &BuildEnvironment, .. }`).

Things the writer needs from `self.env` (all present after resolve_phase):
`titles`, `longtitles`, `tocs`, `toc_num_entries`, `toc_secnumbers`,
`toc_fignumbers`, `metadata`, `toctree_includes`, `root_doc`, relations via
`toctree::collect_relations(&env)` (`src/env/toctree.rs:714-725`; compute
once per build like Sphinx's `prepare_writing`, `SPHINX/builders/html/__init__.py:492`),
`py` (objects.inv, modindex), `std`, `index_entries`. Missing: `images`
(FilenameUniqDict), `dlfiles`, `found_docs` (the crate derives it from
`read_results`).

---

## 3. Write path today, the incremental cache, and the write set

### 3.1 `write_phase` / `write_one` / `get_output_path` (`src/builder.rs:1459-1543`)

```rust
fn write_phase(&self, documents: &[Document]) {
    documents.par_iter().for_each(|document| {
        if let Err(e) = self.write_one(document) { /* BuildErrorReport(Other) */ }
    });
}
fn write_one(&self, document: &Document) -> Result<()> {
    let output_path = self.get_output_path(&document.source_path)?;
    create_dir_all(parent); std::fs::write(&output_path, &document.html)?; Ok(())
}
fn get_output_path(&self, source_path: &Path) -> Result<PathBuf> {
    let relative_path = source_path.strip_prefix(&self.source_dir)?;
    let mut output_path = self.output_dir.join(relative_path);
    output_path.set_extension("html"); Ok(output_path)
}
```
- Output path is **source-path based** (`guide/x.rst` → `guide/x.html`,
  `README.md` → `README.html`). Sphinx: `outdir / (docname + out_suffix)`
  (`SPHINX/builders/html/__init__.py:1034-1035`), `out_suffix` =
  `html_file_suffix` or `.html` (`:176-178`); links use `quote(docname) +
  link_suffix` (`:1067-1068`; `link_suffix` = `html_link_suffix` or
  `out_suffix`, `:180-184`). Same result for plain names, differs once
  suffixes are configurable or docnames need quoting.
- Every document in `read_results` is written (read or cache hit); docs that
  failed to read are not; stale pages of removed docs are never deleted
  (Sphinx doesn't delete either).

### 3.2 The placeholder renderer lives in the READ phase

`read_one_file` (`:829-899`):
```rust
// cache path, only if !outdated && self.incremental (:841-865)
let hit = self.cache.get_document_with(file_path, |cached| {
    if cached.source_mtime < file_mtime || cached.html.is_empty() { return None; }  // :844
    self.load_doctree(&docname)                                                     // :853
});
...
// miss path
let rendered_html = format!("<html><body>{}</body></html>",
    html_escape::encode_text(&document.content.to_string()));                      // :877-880
document.html = rendered_html;
if self.incremental { self.cache.store_document(file_path, &document)?; }           // :889-891
```

### 3.3 `BuildCache` (`src/cache.rs`)

- `BuildCache { cache_dir, config_fingerprint, config_changed, documents:
  DashMap<PathBuf, CachedDocument>, file_hashes, hit_count, miss_count,
  max_size_mb, expiration_duration }` (`:16-26`);
  `CachedDocument { document: Document, hash: String, cached_at, access_count,
  size_bytes }` (`:28-35`).
- Key = absolute source path. `hash` = blake3(file bytes + mtime seconds LE)
  (`:280-295`).
- `new` (`:38-76`): `.config-fingerprint` mismatch ⇒ `remove_dir_all(cache_dir)`
  (documents, `doctrees/`, `env.bin`, `__intersphinx_cache__`) and rewrite the
  fingerprint; then `load_from_disk` reads every `*.json` in the cache dir
  root (`:361-401`), keeping entries whose hash matches and are not expired.
- `get_document_with(path, accept)` (`:125-170`): recompute hash; entry valid
  if same hash and not expired; then the caller's `accept` must return `Some`
  (else counted as a miss, entry kept). Hit ⇒ `access_count += 1`,
  `hit_count += 1`.
- `store_document` (`:172-202`): insert + evict (LFU by access_count,
  `:325-359`, size from `estimate_document_size` = `html.len()` + paths + 1024,
  `:303-310`) + `persist_to_disk` pretty JSON at `<cache_dir>/<blake3(path)>.json`
  (`:403-421`).
- The fingerprint is blake3 of `serde_json` of the whole `BuildConfig` minus
  `fail_on_warning`, `nitpicky` (`src/builder.rs:177`, `:217-227`). **Every
  HTML-only knob (html_title, html_theme, html_context, `-A`) is in it**, so
  changing one wipes all caches and forces a cold re-read — Sphinx only
  rewrites (`.buildinfo` mismatch). And `config_changed()` feeds
  `get_outdated_files` so the whole project is "added".

### 3.4 What a cache hit writes today

Hit ⇒ `ReadResult{document: cached Document (with placeholder html), doctree:
persisted doctree, read_time_us: None}` ⇒ merge skips it ⇒ resolve uses its
doctree ⇒ `write_one` writes the **cached `html`**. So the cached HTML is
the page. With a real writer this is wrong: a page depends on other docs
(prev/next, parents, toctree entries' titles, xref targets and their titles,
numbering, sidebars' global toctree) — see §3.5.

### 3.5 Which docs Sphinx writes on an incremental HTML build (source + probe)

`StandaloneHTMLBuilder.get_outdated_docs` (`SPHINX/builders/html/__init__.py:332-404`):
1. Load `outdir/.buildinfo` (`BuildInfo.load`, `SPHINX/builders/html/_build_info.py`);
   `ValueError` ⇒ warning `Failed to read build info file: %r`; if it differs
   from the current `BuildInfo(config, tags, {'html'})` (md5 `stable_hash` of
   every `rebuild == 'html'` config value, and of `sorted(tags)`) ⇒ move it to
   `.buildinfo.bak`, dump the new one, log
   `building [html]: build_info mismatch, copying .buildinfo to .buildinfo.bak`,
   and **yield every found doc**.
2. `template_mtime` = newest template mtime (µs) if templates exist; if newer
   than `.buildinfo`'s mtime, log `template %s has been changed since the
   previous build, all docs will be rebuilt` (the comparison below then does it).
3. For each found doc: yield if not in `env.all_docs`; else yield if
   `max(source mtime, template_mtime) > target html mtime` (missing target ⇒
   mtime 0 ⇒ yielded).

`.buildinfo` content (`_build_info.py` `dump`):
```
# Sphinx build info version 1
# This file records the configuration used when building these files. When it is not found, a full rebuild will be done.
config: <md5 hex of stable_hash({name: value for html-category config})>
tags: <md5 hex of stable_hash(sorted(tags))>
```
Probe (`project = 'P'` only): `config: 903c1988e93fbbb40659dd2c9bf2daf9`,
`tags: 645f666f9bcd5a90fca523b33c5a78b7` with tags
`['builder_html', 'format_html', 'html']` (probe3.py; the full list of the 60
html-category values and their defaults is in §7.3).

Probe transcript (`probe1.sh`, `index` toctree `a b`, then append `c`, then
retitle `a`):
```
BUILD 1 (cold): building [html]: targets for 3 source files that are out of date
  updating environment: [new config] 3 added, 0 changed, 0 removed
  writing output... a, b, index; generating indices... genindex; writing additional pages... search;
  dumping search index ...; dumping object inventory...
BUILD 2 (no change): targets for 0 source files ...; 0 added, 0 changed, 0 removed;
  "no targets are out of date."; NO doc written; still: copying static files, genindex,
  search, search index, inventory
BUILD 3 (add c.rst + 'c' in index toctree): 1 added, 1 changed; reads c, index;
  writes c, index ONLY
  -> b.html has NO <link rel="next"> (stale; c's page has rel="prev" title="B")
BUILD 4 (retitle a -> "A renamed"): 0 added, 1 changed; reads a; writes a, index
  (index via files_to_rebuild[a] = {index})
  -> b.html still has <link rel="prev" title="A" href="a.html" /> (stale)
```
Always regenerated per build (even "no targets are out of date"): static
files (`copy_assets` in `write()` prepare), `genindex.html`, domain indices,
`search.html`, `searchindex.js` (indexer loads the old index and prunes, only
written docs are re-fed), `objects.inv`, `.buildinfo`. `_sources/<doc>` is
copied **only for written docs** (`handle_page`, `:1252-1257`). Images are
copied for written docs' images (`write_doc_serialized` →
`post_process_images` fills `self.images`, `copy_image_files` in finish).

Warnings follow the write set (probe p4, `a.rst` has `:doc:\`nope\``, `b.rst`
duplicates label `dup`):
```
html  build 1: b.rst:7 duplicate label dup ... ; a.rst:9 unknown document: 'nope' [ref.doc]
html  build 2 (no change): (nothing)
dummy build 1: same two warnings
dummy build 2 (no change): building [dummy]: targets for 3 source files that are out of date
                           a.rst:9: WARNING: unknown document: 'nope' [ref.doc]   <- re-emitted
```
(Also: re-reading `b` does **not** re-warn the duplicate label, because the
later registration overwrote `labels['dup']` to `b` and `clear_doc(b)` removed
it — `SPHINX/domains/std/__init__.py:959-966`; the crate does the same insert,
`src/env/std_domain.rs:255-268`.)

`sphinx-build -a` = `build_all` → write set = all found docs, **read still
incremental** (`SPHINX/builders/__init__.py:321-325`). `-E` = fresh env ⇒ all
docs read ⇒ all written.

### 3.6 What must change for a real writer (checklist)

1. Stop rendering in `read_one_file`; delete `document.html = rendered_html`
   (`:874-881`) and **delete `|| cached.html.is_empty()`** from the hit
   acceptance (`:844`). Keep `Document.html` (or remove the field; old cache
   JSON still decodes because serde ignores unknown fields). Update
   `estimate_document_size` (`src/cache.rs:303-310`) if eviction should stay
   meaningful. cache.rs tests `roundtrip_preserves_rendered_html` (`:436-450`)
   and `caller_rejected_entry_counts_as_a_miss` (`:470-493`) use `html` as
   payload; they still pass if the field stays.
2. Write from `(docname, resolved doctree, env)`, never from cached output.
3. Output path from docname + `out_suffix`; relative links from the builder's
   `get_relative_uri` (port `sphinx.util.osutil.relative_uri` exactly — it
   strips `#fragment`s from both args, drops common *directory* segments,
   returns `''` for identical, `'./'` for `f/index.html`→`f/`, prefixes
   `'../' * (len(base_segments)-1)`; the existing `utils::relative_uri`
   (`src/utils.rs:563-581`, pathdiff-based, dead code) is **not** equivalent).
4. Builder kind: `html` (write set per §3.5) and `dummy` (write set = all found
   docs, `get_target_uri == ''`). env_differential + builder unit tests
   should build `dummy` to keep `resolved_pformat`/`genindex` and the warm
   warning semantics they pin; the CLI builds `html`. `-b dummy` is a
   ROADMAP M2 deliverable anyway ("Builders: `html`, `dirhtml`, `dummy`",
   `ROADMAP.md:285`). Today `-b` accepts only `html` (`src/main.rs:514-520`,
   `-M` only `html`/`clean` `:489-512`).
5. Choose the html write set:
   - **(A) write every found doc every build** (today's behaviour): always
     fresh pages (strictly better than Sphinx), but warm-build output bytes
     and warm warnings differ from `sphinx-build` (Sphinx emits no resolution
     warnings on a no-op rebuild), and every build pays full resolve+render.
   - **(B) Sphinx's set** (`get_outdated_docs` ∪ read ∪ numbering-changed ∪
     `files_to_rebuild` containers): byte-parity with Sphinx's incremental
     output **including its stale pages**; requires `.buildinfo`, template
     mtimes, the numbering `changed` lists from `number_phase`, output mtime
     checks, and resolving only the write set. `-a` then maps to "write all,
     read incremental" and `sphinx_build_incremental_by_default_and_fresh_env`
     (`tests/e2e_cli.rs:1148-1178`, asserts `-a` ⇒ `Cache hits: 0`) changes —
     exactly the revisit `src/main.rs:554-566` anticipates.
   Under both, `incremental_cache_hit_still_writes_output`
   (`tests/e2e_cli.rs:341-363`) keeps passing (a missing target is outdated).
6. Resolution warnings: with (B), only written docs warn; with a `dummy` kind
   for env_differential, `a_warm_rebuild_reports_the_same_std_domain_warnings`
   (`tests/env_differential.rs:2506-2610`) keeps its expectation.
7. Split the cache fingerprint: html-category keys (and `-A`) should not wipe
   the read cache/doctrees/env (they only change pages). Sphinx's split is
   the `rebuild` class (`'env'` vs `'html'`, list in §7.3).
8. `write_phase` uses the global rayon pool; read uses a `parallel_jobs`
   pool (`:784-800`). Use the same pool for writing if `-j` should bound it.
9. Discovery excludes (§5.4).

---

## 4. Warnings pipeline

### 4.1 Types (`src/error.rs`)

```rust
pub struct BuildWarning {            // :50-66
    pub file: PathBuf,               // empty => bare "WARNING: ..."
    pub line: Option<usize>,
    pub message: String,
    pub warning_type: WarningType,   // #[allow(dead_code)]: never read
    pub category: Option<String>,    // " [type.subtype]" suffix (show_warning_types)
}
pub enum WarningType { MissingToctreeRef, OrphanedDocument, BrokenCrossReference,
    MissingFile, UnusedLabel, DuplicateLabel, EmptyToctree, Other }   // :77-88
pub struct BuildErrorReport { file, line: Option<usize>, message, error_type: ErrorType }  // :68-75
pub enum ErrorType { ParseError, FileNotFound, TemplateError, SyntaxError, Other }
```
`BuildWarning::render()` (`:134-151`): `"{file}[:{line}]: WARNING: {message}[ [{category}]]"`,
or `"WARNING: {message}[ [{category}]]"` when `file` is empty. **Always
"WARNING"** — no ERROR/CRITICAL; `WarningType` has no consumer (only
constructors, e.g. `src/env/resolve.rs:1079`), so a `level` field is free to add.

### 4.2 Emission order inside `build()` (push order into `self.warnings`)

1. `dedup_by_docname` (discovery) — `:626-636`
2. intersphinx load warnings — `:345-355`
3. merge (docname order, read docs only): per doc → toctree record warnings
   (`ToctreeWarningKind` → `MissingToctreeRef`/`EmptyToctree`/`Other`, with
   category) → `registry.log_warnings` (no category; `rendered_path` doubles
   `.rst` for `doc2path_location`) → index `process_doc` warnings → std
   `process_doc` warnings (which replay the py registrations too)
4. numbering warnings (`report_numbering_warning`, at the toctree directive's
   source/line via the doc's Nth `ToctreeRecord`)
5. consistency (orphans, `location=docname`: path, no line, `[toc.not_included]`)
6. xref resolution per doc (docname order, ALL docs)
7. genindex `create_index` messages (`message.into_warning(&source)`)
8. (write errors → `self.errors`)
9. M1 validation warnings (discovery order)

Errors: read failures (`ParseError`), doctree store failures, write failures.

### 4.3 Printing (`src/main.rs:349-459`)

- Warnings are printed **after** the build, all at once, in push order:
  `warn!("{}", warning.render())` → env_logger default format, i.e. stderr
  lines look like `[<RFC3339 timestamp> WARN  sphinx_ultra] /abs/index.rst:4:
  WARNING: ... [toc.not_readable]` — not byte-identical to Sphinx's bare line.
  Because they go through `log`, **`RUST_LOG=error` silences warnings**
  (pinned by `preset_rust_log_is_respected`, `tests/e2e_cli.rs:1257-1276`);
  Sphinx always prints them.
- `-w FILE` (`:350-408`): parent dirs created, truncated; content = config
  warnings as `WARNING: {msg}` (`:368-372`), then each `render()` line, then
  each error as `{file}[:{line}]: ERROR: {message}`. Sphinx's `-w` receives the
  same lines as stderr (probe §4.6), including multi-line bodies.
- Errors: `eprintln!("{file}[:{line}]: ERROR: {message}")` (`:388-403`).
- Totals: `total_warnings = stats.warnings + config_warnings.len()` (`:373`).
  conf.py parse-drop warnings (`config.rs:545-552`) are not counted.
- `-W` (`:414-421`): if `total_warnings > 0` → `eprintln!("build finished with
  problems, N warning[s] (with warnings treated as errors).")`, exit 1. Sphinx
  prints that line on **stdout** (probed) and omits the "HTML pages are in"
  epilog.
- errors (`:427-439`): exit 1 with `build finished with problems, N error[s][,
  M warning[s]].` — deliberately stricter than Sphinx (Sphinx exits 0 on
  logged ERRORs without `-W`; probe: 6 warnings incl. ERROR+CRITICAL, exit 0).
- success with warnings: `warn!("build succeeded, N warning[s].")` (stderr).
  Sphinx: `build succeeded, 6 warnings.` on stdout (and `build succeeded.`
  with none). `intersphinx_resolves_a_cross_project_ref_and_reports_a_missing_external`
  (`tests/e2e_cli.rs:1582-1611`) pins `build succeeded, 1 warning.` on stderr.
- `info!` stats lines: `Build completed successfully!`, `Files processed:`,
  `Files skipped:`, **`Cache hits: N`** (asserted by ~20 e2e checks), `Build
  time`, `Output size`.
- `print_final_location` (compat mode, not `-q`): `println!("\nThe HTML pages
  are in {output}.")`.
- `-q` (`:466-475`): log filter `warn` (info off, warnings on);
  `-v`/`-vv` → debug/trace. Sphinx `-Q` (suppress warnings) is not accepted.

### 4.4 What doctree-held diagnostics are printed today

| Record | Where it lives | Printed? |
|---|---|---|
| `ToctreeRecord.warnings` (missing/excluded doc, empty glob, pattern error, duplicate entry) | `Document.toctrees[*].warnings` (parse records, survive cache) | yes, merge phase, only for docs read this build (`:1127-1151`) |
| `RegistryExport.log_warnings` (`ParseLogWarning{source,message,line,doc2path_location}`, `src/rst/mod.rs:327-343`): literalinclude logger warnings (`block.rs:4383`), malformed option description (`:4985`), py type-param list warnings (`:5285`), duplicate py parameter names (`:5321`) | `Document.registry` | yes, merge phase, after the toctree warnings (`:1152-1169`) |
| index/std/py domain registration diagnostics | computed in merge from doctree + registry | yes |
| numbering / consistency / xref / genindex | computed in resolve | yes |
| **docutils `system_message` nodes** (level 1-4; built by `messages::system_message` `src/doctree/messages.rs:23-33` and `Parser::msg`/`msg_sm` `src/rst/block.rs:712-724`, 43 `msg(` call sites in block.rs + 3 in inline.rs): title/underline, indentation, unknown directive/role, directive option/argument errors, include/literalinclude SEVEREs, circular inclusion chains, glossary misformat ×3, duplicate targets, inline markup errors, `Invalid caption` funnel, ... | in the doctree only (persisted, cache-hit safe) | **never** (`docs/IMPLEMENTATION_STATUS.md:491-501`; `KNOWN_WARNING_GAPS["inc_basic"]` `tests/env_differential.rs:563-577`) |

A docutils `system_message.astext()` port already exists:
`system_message_astext` (`src/rst/block.rs:11418-11440`) → `'{source}:{line}:
({type}/{level}) ' + children joined by '\n\n'`.

### 4.5 Sphinx's reporter channel (what printing must reproduce)

- `WarningStream.write` (`SPHINX/util/docutils.py:385-393`) parses docutils'
  `source:line: (TYPE/level) text` with `report_re` (`:31`) and calls
  `logger.log(TYPE, message, location='source:line', type='docutils')` — so
  the category suffix is `[docutils]`, level names map `WARNING`→`WARNING`,
  `ERROR`→`ERROR`, `SEVERE`→`CRITICAL`; INFO (1) is below the default
  `report_level` 2 and is not streamed.
- Messages are streamed **when the reporter creates them** (docutils
  `Reporter.system_message`), not when attached to the tree — a message built
  in a discarded nested parse still prints. Recording at creation time (at
  `messages::system_message`/`msg()`) is therefore closer than walking the
  finished tree; audit speculative constructions.
- `FilterSystemMessages` (priority 999, read phase;
  `SPHINX/transforms/__init__.py:337-347`): removes every system_message with
  `level < (2 if keep_warnings else 5)` — i.e. by default **all** of them —
  before the doctree is pickled. So Sphinx HTML never contains
  `system-message` asides unless `keep_warnings = True` (probe: `grep -c
  system-message` = 0). The crate keeps them; the writer (or a read-time
  transform + `DOCTREE_FORMAT_VERSION` bump) must strip them. The env oracle
  only uses `keep_warnings=True` projects (`KNOWN_INERT_CONF`,
  `tests/env_differential.rs:883-935`), so stripping must be conditional on a
  real `keep_warnings` key (missing from `BuildConfig`, §7.2).
- Resolution-time reporter: `get_doctree` installs a `LoggingReporter`
  (`SPHINX/environment/__init__.py:650-662`) so post-transform messages print
  the same way.

### 4.6 Probe: format, ordering, counting (`probe2.sh`, p2)

Source `index.rst`: toctree with `a`, `missing`; `:ref:\`nolabel\``;
`.. include:: nofile.rst`; `.. bogusdirective:: x`; two `.. _dup:` targets.
`a.rst`: `Body a *unterminated.`. `sphinx-build -b html -q -w warn.txt src out`:
```
<P>/src/a.rst:4: WARNING: Inline emphasis start-string without end-string. [docutils]
<P>/src/index.rst:4: WARNING: toctree contains reference to nonexisting document 'missing' [toc.not_readable]
<P>/src/index.rst:14: CRITICAL: Problems with "include" directive path:
InputError: [Errno 2] No such file or directory: 'src/nofile.rst'. [docutils]
<P>/src/index.rst:16: ERROR: Unknown directive type "bogusdirective".

.. bogusdirective:: x [docutils]
<P>/src/index.rst:22: WARNING: Duplicate explicit target name: "dup". [docutils]
<P>/src/index.rst:12: WARNING: undefined label: 'nolabel' [ref.ref]
exit=0
```
- `warn.txt` is byte-identical to the stderr lines.
- Multi-line: the literal_block child is appended after a blank line, and the
  ` [docutils]` suffix lands at the end of the *last* line.
- Order: per document in read order (`a` before `index`), and within
  `index` the Sphinx-logger toctree warning (line 4) is interleaved *before*
  the reporter messages at lines 14/16/22 — emission order during the parse —
  then the write-phase `undefined label` comes after all read-phase output.
- Counting: `build succeeded, 6 warnings.` — ERROR and CRITICAL count as
  warnings; `-W` ⇒ `build finished with problems, 6 warnings (with warnings
  treated as errors).` on stdout, exit 1.
- The crate's per-doc replay is "all toctree warnings, then all log warnings"
  (`:1113-1120` documents this simplification). Interleaving reporter output
  correctly needs one sequence across toctree records, log records and
  reporter records (e.g. a shared counter stamped at creation, or one
  unified `Vec<ParseDiagnostic>` in `RegistryExport`, which also makes it
  cache-hit safe the way `log_warnings` is).

### 4.7 Collisions with M1 validation once reporter output is printed

Probe p5 (`.. note::` with no body; toctree `:bogus:` option):
```
<P>/src/index.rst:4: ERROR: Content block expected for the "note" directive; none found. [docutils]
<P>/src/index.rst:6: ERROR: Error in "toctree" directive:
unknown option: "bogus".

.. toctree::
   :bogus:

   self [docutils]
```
The crate today prints `index.rst:4: WARNING: Note directive requires
content` and `Unknown option 'bogus' for toctree directive` from
`validate_directives_and_roles`; both would be printed *in addition* to the
reporter lines. `directive_validation_reports_real_problems`
(`tests/e2e_cli.rs:1302-1334`) pins the M1 texts; `directive_validation_off_switch`
(`:1363-1380`) pins `-D validate_directives=0`. Decide: turn the validator
off by default (or drop overlapping checks) when the reporter channel lands.

---

## 5. Output directory layout and static assets

### 5.1 Crate today

| Invocation | Pages | Cache ("doctrees") dir |
|---|---|---|
| native `build --source S --output O` | `O/<relpath>.html` | `O/.sphinx-ultra-cache` |
| compat `sphinx-ultra S O` (`-b html`) | `O/` | `O/.sphinx-ultra-cache` (or `-d DIR`) |
| compat `-M html S B` | `B/html/` (`main.rs:491-494`) | `B/.sphinx-ultra-cache` (`make_mode_cache_dir`, `main.rs:492`, `:574`) unless `-d` |
| compat `-M clean S B` | removes contents of `B`, prints `Removing everything under 'B'...` (`main.rs:495-505`) | (inside B) |

Cache dir contents: `.config-fingerprint`, `<blake3(abs source path)>.json`
per cached Document (incremental only), `doctrees/<blake3(docname)>.doctree`,
`env.bin`, `__intersphinx_cache__/` (`src/intersphinx/mod.rs:46`).

Output tree for fixture `basic` today: `index.html`, `installation.html`,
`_static/{doctools.js,jquery.js,pygments.css,sphinx_highlight.js,theme.css}`,
`.sphinx-ultra-cache/...` — plus `_static/**` / `_templates/**` if the source
has them.

Refusals: output == source, or output an ancestor of source ⇒ exit 1
(`output_overlaps_source`, `main.rs:603-619`). Output *inside* source is
allowed (see §5.4 hazard).

### 5.2 Sphinx

- `-b html S O`: pages in `O/`, doctrees in `O/.doctrees/`
  (`SPHINX/cmd/build.py:313-316`).
- `-M html S B`: `B/html/` + `B/doctrees/` (`SPHINX/cmd/make_mode.py:188-196`).
- Probe tree (p1, `project='P'`, alabaster default):
```
.buildinfo
.doctrees/{a,b,index}.doctree  .doctrees/environment.pickle
_sources/{a,b,index}.rst.txt
_static/{alabaster.css,base-stemmer.js,basic.css,custom.css,doctools.js,
         documentation_options.js,english-stemmer.js,file.png,github-banner.svg,
         language_data.js,minus.png,plus.png,pygments.css,searchtools.js,
         sphinx_highlight.js}
a.html b.html index.html genindex.html search.html searchindex.js objects.inv
```
(`py-modindex.html` appears when py modules exist; `_images/`, `_downloads/`
when used.) `basic.css`, `documentation_options.js`, `language_data.js`,
`alabaster.css` are `_t`-templated ("Writing evaluated template result to
..."). Asset URLs carry `?v=<crc32 hex>` checksums, e.g.
`_static/pygments.css?v=5ecbeea2`, `basic.css?v=b08954a9`,
`alabaster.css?v=27fed22d`, `documentation_options.js?v=5929fcd5`.

### 5.3 Static copying — crate today vs Sphinx

Crate `copy_static_assets` (`src/builder.rs:1551-1613`), runs **after** the
write phase:
1. `create_dir_all(out/_static)`.
2. Probe `current_exe()/../static`, `/../../static` (hits the repo `static/`
   when running `target/debug/sphinx-ultra` — i.e. in every test),
   `/../../../static`, and CWD-relative `rust-builder/static`; copy its files
   (flat, files only). Else `create_default_static_assets` (`:1615-1636`)
   writes the same five files from `include_str!("../static/...")`.
3. Copy `<srcdir>/_static` → `out/_static` and **`<srcdir>/_templates` →
   `out/_templates`** with `utils::copy_dir_recursive` (`src/utils.rs:476-497`,
   copies dotfiles too). `html_static_path`, confdir (`-c`), `exclude_patterns`
   are all ignored.

Sphinx `copy_assets` runs in `write()` **before** documents are written
(`SPHINX/builders/html/__init__.py:644-648`): `copy_download_files`,
`copy_static_files` (`:913-933`: mkdir `_static`; `create_pygments_style_file`
(+ `pygments_dark.css` if the theme has a dark style); `copy_translation_js`;
`copy_stemmer_js`; theme `static/` dirs in inheritance order with `_t`
rendering (`copy_theme_static_files`); extension static dirs;
`html_static_path` entries relative to **confdir**, excluded
`[*exclude_patterns, '**/.*']`, templated, **overriding** theme files;
`html_logo`, `html_favicon` copied to `_static/<basename>`), then
`copy_extra_files` (`html_extra_path` → outdir root, `exclude_patterns`).

### 5.4 Discovery hazard once `_sources` exist

Sphinx `find_files` excludes `exclude_patterns + templates_path +
builder.get_asset_paths()` (= `html_extra_path + html_static_path`,
`SPHINX/builders/html/__init__.py:406-407`) plus `EXCLUDE_PATHS =
['**/_sources', '.#*', '**/.#*', '*.lproj/**']`
(`SPHINX/environment/__init__.py:489-495`, `SPHINX/project.py:20`, `:62`).
The crate adds none of these (`src/builder.rs:519-529`). With `sphinx-ultra
docs docs/out`, the writer's `docs/out/_sources/index.rst.txt` would be
discovered next build as a `.txt` document. Add the Sphinx list (and the
config-derived paths) to the built-in excludes.

---

## 6. Tests that pin the placeholder / output shape

### 6.1 `tests/e2e_cli.rs` — output-content or output-tree assertions

| Test (lines) | Asserts | Impact of the real writer |
|---|---|---|
| `build_succeeds_and_writes_html_tree` (48-63) | `index.html`, `installation.html` exist; `index.html` contains `"Welcome"` | passes (title/h1) |
| `build_with_relative_source_path_works` (65-80) | `out/index.html` exists | passes |
| `per_file_error_reports_and_exits_one` (145-181) | exit 1, `bad.rst: ERROR:`; `index.html` & `good.html` exist | passes if the writer tolerates a doc missing from results |
| `clean_removes_output_dir` (243-258) | output dir exists, `clean` removes it | passes |
| `config_flag_accepts_conf_py` (260-281) | `index.html` exists | passes |
| `config_flag_accepts_partial_yaml` (318-339) | `index.html` exists (YAML ⇒ `BuildConfig::default()` html keys: `html_theme = sphinx_rtd_theme`, `html_static_path = ["_static"]`) | **risk**: unknown theme / "html_static_path entry '_static' does not exist" if ported validators run |
| `incremental_cache_hit_still_writes_output` (341-363) | delete both pages; warm `--incremental` build ⇒ `Cache hits: 2` and pages recreated | **breaks if `html.is_empty()` stays in hit acceptance**; passes under write-set (A) or (B) |
| `touching_an_embedded_image_re_reads_only_the_page_that_embeds_it` (369-410) | `Cache hits: 2/1/2`; `page.html`, `index.html` exist after run3 | cache-hit counts at risk (same as above) |
| `touching_an_included_file_re_reads_the_documents_that_pull_it_in` (421-493) | `Cache hits: 4/2/2/4`, no orphan warning | cache hits |
| `clean_incremental_build_produces_full_output` (495-507) | both pages exist after `--clean --incremental` | passes |
| `config_change_invalidates_cache` (509-539) | `Cache hits: 2` then `0` after YAML `project` change | passes (project stays `'env'`-class) |
| `two_files_for_one_docname_resolve_silently_and_deterministically` (589-631) | `-W` passes; `page.html` contains `"rst body"`; **page bytes identical across 3 builds** | **risk**: any per-build nondeterminism (e.g. `html_last_updated_fmt` default `Some("%b %d, %Y")` ⇒ date in the page, flaky at midnight; timestamps/hash orders); `-W` must stay clean for the `.md`/`.txt` siblings |
| `operational_flags_do_not_invalidate_the_cache` (705-778) | `Cache hits: 3`; warm `-W` passes twice | cache hits; warm warnings must stay empty (only read-phase dup label here) |
| `sphinx_build_mode_positional` (812-826) | both pages exist; stdout `The HTML pages are in` | passes |
| `sphinx_build_mode_default_builder_is_html` (828-836) | `index.html` exists | passes |
| `sphinx_build_make_mode_html_and_clean` (857-904) | `out/html/index.html`; `-M clean` removes `out/html`, keeps `out`; stdout `Removing everything under`; `-M latexpdf` exit 2 | passes (if cache moves to `B/doctrees`, clean still removes it) |
| `sphinx_build_d_override_excludes_file` (906-923) | `index.html` yes, `installation.html` no | passes (Sphinx also warns `toctree contains reference to excluded document`; test checks exit 0 only) |
| `sphinx_build_confdir_flag` (1098-1122) | `installation.html` absent | passes |
| `sphinx_build_doctreedir_flag` (1124-1146) | `trees/.config-fingerprint` exists; `out/.sphinx-ultra-cache` absent | passes |
| `sphinx_build_incremental_by_default_and_fresh_env` (1148-1178) | `Cache hits: 2` warm; `-E` ⇒ 0; **`-a` ⇒ `Cache hits: 0`** | must change if `-a` becomes "write all, read incremental" |
| `sphinx_build_j_auto_and_a_flag_accepted` (1198-1216) | `index.html` exists (`-A release_banner=1`, `-t mytag`) | passes |
| `source_dir_named_build_works_via_dot_slash` (1237-1255) | `out/index.html` exists | passes |
| `stats_prints_source_file_count` (780-792) | `Source files: 2` | unaffected |

Tests asserting **absence/exact count of warnings or `-W` success** (new writer
warnings, reporter output or HTML config validators would break them):
`toctree_forms_build_without_false_positives` (103-116, no `WARNING` at all),
`toctree_glob_matches_and_warns_on_dead_pattern` (118-143),
`multiple_toctree_parents_do_not_fail_under_fail_on_warning` (198-223),
`a_non_rst_file_is_not_reported_as_an_orphan` (546-577),
`two_files_for_one_docname...` (589-631, `-W`),
`a_numbered_toctree_with_a_depth_builds_clean` (638-662, `-W`),
`a_label_above_a_figure_numbers_it_and_numref_resolves` (670-697, `-W`; its
`pic.png` is the single byte `x` — fine unless the writer reads image sizes),
`operational_flags_do_not_invalidate_the_cache` (705-778),
`directive_validation_silent_on_valid_sphinx` (1336-1361),
`nitpicky_resolves_real_refs` (1550-1574),
`intersphinx_resolves_a_cross_project_ref...` (1582-1611, exact `build succeeded, 1 warning.`),
`sphinx_build_d_source_encoding_warns...` (1706-1751),
`directive_validation_reports_real_problems` (1302-1334, pins M1 texts, §4.7).

### 6.2 Other tests touching the write path

- `src/builder.rs` unit tests: `a_build_whose_environment_cannot_be_saved_still_writes_its_output`
  (2199-2223: `index.html`, `a.html` exist), `a_non_incremental_build_reads_every_document`
  (2268-2293: `cache_hits == 0`, pages exist), `cache_hit_still_writes_output_and_fills_the_environment`
  (2295-2317: delete pages; warm ⇒ `cache_hits == 2`, pages recreated),
  `cache_hit_whose_doctree_is_missing_is_treated_as_a_miss` (2065-2097:
  `cache_hits` 0/2/1), `a_doctree_written_in_the_unversioned_format...`
  (2127-2158), `corrupt_doctree_file_is_treated_as_a_miss` (2178-2192),
  `fresh_env_discards...` (2227-2264). All depend on the `html` hit-acceptance
  pitfall (§0.3).
- `src/cache.rs` tests (436-582) use `Document.html` as the payload.
- `tests/env_differential.rs`: every corpus build (`build_project` 367-408)
  and the warm/cold tests (1903-2040, 2506-2610, 2828-2882) read
  `snapshot_env()`/warnings — they need dummy URIs and dummy write-set
  semantics (§2.4, §3.6.4).
- `benches/builder_benchmark.rs` (2 `SphinxBuilder::new` calls) — already
  broken (`docs/IMPLEMENTATION_STATUS.md:147`), measures the placeholder.

---

## 7. HTML-relevant configuration

### 7.1 Plumbing recap

- **YAML/JSON**: `serde` straight into `BuildConfig` (`#[serde(default)]`,
  unknown keys **silently ignored** — no `deny_unknown_fields`). Field names are
  the Rust names; the theme lives at `output.html_theme` *and* `theme.name`
  (a top-level YAML `html_theme:` is silently dropped).
- **conf.py**: `PythonConfigParser` parses literals into `conf_namespace`
  (`src/python_config.rs:220-255`), `extract_configuration` projects known keys
  into `ConfPyConfig` (`:258-483`; HTML block `:338-370`), keys not in
  `is_standard_config_key` (`:486-571`) go to `custom_configs` (**never read by
  `to_build_config`**), then `to_build_config` (`:1170-1349`) starts from
  `BuildConfig::default()` and copies selected fields. `extract_string` returns
  `None` for non-strings (a tuple/list `copyright` is silently dropped);
  `extract_string_list` returns `[]` when absent and drops non-string items
  (so `html_css_files = [('x.css', {'rel': ...})]` silently loses the tuple).
- **`-D key=value`**: `apply_override` (`src/config.rs:692-810`) round-trips
  through `serde_json` of the whole struct: bool slots take `1/0/true/True/false/False`,
  numbers parse, arrays split on `,`, `Null` (unset `Option`) tries number
  then retries string, dotted keys reach nested structs/maps
  (`html_context.x`); unknown ⇒ `unknown config value 'k' in override,
  ignoring` (counted toward `-W`). Aliases: `html_theme` → `output.html_theme`
  + `theme.name`; `templates_path` → also `template_dirs`; `html_static_path` →
  also `static_dirs` (`:695-709`). Any new `BuildConfig` field becomes
  `-D`-overridable automatically; any new field also enters the cache
  fingerprint (one-time cold build).
- **`-A name=value`**: `html_context[name] = String` (`main.rs:302-306`).

### 7.2 Field-by-field table

"Sphinx" = default / rebuild class / valid types from probe3.py (§7.3).
"conf.py" = what `to_build_config` does.

| Key | In `BuildConfig`? (type, default) | conf.py mapping | Sphinx default, class | Notes |
|---|---|---|---|---|
| `project` | `String`, `"Sphinx Ultra Project"` (`config.rs:39`, `:405`) | copied if set (`python_config.rs:1174-1176`) | `'Project name not set'`, env | default diverges |
| `version` | `Option<String>`, `Some("1.0.0")` (`:42`, `:406`) | copied if set | `''`, env | diverges → `VERSION`, objects.inv `# Version:`, `html_title` |
| `release` | `Option<String>`, `Some("1.0.0")` (`:45`, `:407`) | copied if set | `''`, env | `html_title` default is `'%s %s documentation' % (project, release)` ⇒ `'P  documentation'` |
| `copyright` | `Option<String>`, `Some("2024, Sphinx Ultra")` (`:48`, `:408`) | copied if a **string** | `''`, html; types str/list/tuple; alias `project_copyright`; `%Y` placeholder evaluated, `SOURCE_DATE_EPOCH` correction (`SPHINX/config.py:695-760`) | list form dropped silently |
| `language` | `Option<String>`, `Some("en")` | copied if set | `'en'`, env (`None` → `'en'`, `config.py:573-581`) | ok |
| `root_doc` | `Option<String>`, `Some("index")` | `root_doc` or `master_doc` | `'index'`, env | ok |
| `html_theme` | `output.html_theme` + `theme.name`, both `"sphinx_rtd_theme"` (`:479`, `:492`) | copied to both if set; else stays `sphinx_rtd_theme` | `'alabaster'`, html | **default diverges; YAML has two copies** |
| `html_theme_options` | `theme.options: Value` `{}` | parsed into `ConfPyConfig.html_theme_options` but **not copied** | `{}`, html | dropped |
| `html_static_path` | `Vec<PathBuf>`, `["_static"]` (`:414`) + mirror `static_dirs` | copied (both); **`[]` when absent** (`extract_string_list`) | `[]`, html; entries relative to confdir; validators warn `html_static_path entry %r does not exist` / `... is placed inside outdir` | default differs between YAML and conf.py builds |
| `html_extra_path` | **missing** | parsed, not copied | `[]`, html | |
| `templates_path` | `Vec<PathBuf>`, `["_templates"]` (`:427`) + mirror `template_dirs` | copied (both); `[]` when absent | `[]`, html; relative to confdir; excluded from discovery | `TemplateEngine` resolves them relative to CWD (`src/template.rs:24-26`) |
| `html_title` | `Option<String>`, `None` | copied if set | computed `'{project} {release} documentation'`, html | writer must compute default |
| `html_short_title` | `Option<String>`, `None` | copied if set | `= html_title`, html | |
| `html_baseurl` | **missing** | parsed (`:367`), not copied | `''`, html | canonical `<link>` |
| `html_copy_source` | `Option<bool>`, `Some(true)` | copied | `True`, html | |
| `html_show_sourcelink` | `Option<bool>`, `Some(true)` | copied | `True`, html | |
| `html_sourcelink_suffix` | `Option<String>`, `Some(".txt")` | copied | `'.txt'`, html | `_sources/<docname><source_suffix><suffix>` unless equal (`SPHINX/builders/html/__init__.py:612-619`) |
| `html_permalinks` | **missing** | → `custom_configs` (dropped) | `True`, html | `-D` ⇒ unknown-key warning |
| `html_permalinks_icon` | **missing** | → `custom_configs` | `'¶'`, html | |
| `html_last_updated_fmt` | `Option<String>`, **`Some("%b %d, %Y")`** (`:426`) | **bug**: reads `html_context["last_updated"]` (`python_config.rs:1243-1247`); the real key goes to `custom_configs` | `None` (no "last updated"), html; `''` ⇒ `'%b %d, %Y'`; `html_last_updated_use_utc` (False) picks local vs UTC (`SPHINX/builders/html/__init__.py:472-480`) | default + mapping both wrong |
| `html_last_updated_use_utc` | **missing** | custom_configs | `False`, html | |
| `html_context` | `BTreeMap<String, Value>` `{}` | copied | `{}`, html | `-A` should int-coerce |
| `html_sidebars` | **missing** | custom_configs | `{}`, html; string values raise ConfigError (`:1426-1441`) | |
| `html_style` | `Vec<String>`, `["sphinx_rtd_theme.css"]` (`:411`) | **not mapped** (custom_configs) | `None`, html; str/list/tuple | default diverges |
| `html_css_files` / `html_js_files` | `Vec<String>` `[]` | copied, tuples dropped | `[]`, html; str or `(name, attrs)`; `invalid css_file: %r, ignored` | |
| `html_logo` / `html_favicon` | `Option<String>` `None` | copied | `None`, html; validators `logo file %r does not exist` / `favicon file %r does not exist` | |
| `html_file_suffix` | **missing** | parsed, not copied | `None` (⇒ `.html`), html | |
| `html_link_suffix` | **missing** | parsed, not copied | `None` (⇒ out_suffix), html | |
| `html_domain_indices` | **missing** | custom_configs | `True`, html; bool or list of index names | |
| `html_use_index` | `Option<bool>`, `Some(true)` | copied | `True`, html | |
| `html_split_index` | **missing** | parsed, not copied | `False`, html | |
| `html_secnumber_suffix` | **missing** | parsed, not copied | `'. '`, html | |
| `html_compact_lists` | **missing** | parsed, not copied | `True`, html | |
| `html_codeblock_linenos_style` | **missing** | parsed (ConfPy default `"table"` — wrong), not copied | `'inline'`, html, `ENUM('table','inline')` | |
| `html_math_renderer` | **missing** | parsed (ConfPy default `"mathjax"` — wrong), not copied | `None`, **env**; `sphinx.ext.mathjax` loaded by default | |
| `html_scaled_image_link` | **missing** | parsed, not copied | `True`, html | |
| `html_show_copyright` | `Option<bool>`, `Some(true)` | copied | `True`, html | |
| `html_show_sphinx` | `Option<bool>`, `Some(true)` | copied | `True`, html | |
| `html_show_search_summary` | **missing** | custom_configs | `True`, html | |
| `html_output_encoding` | **missing** | parsed, not copied | `'utf-8'`, html | written with `errors='xmlcharrefreplace'` |
| `html_use_opensearch` | `Option<bool>`, `Some(false)` | `Some(!url.is_empty())` — URL lost | `''` (str), html | `-D html_use_opensearch=https://..` ⇒ "invalid boolean" **hard error, exit 2** |
| `html_additional_pages` | **missing** | custom_configs | `{}`, html | |
| `html_search_language/options/scorer` | **missing** | parsed, not copied | `None`/`{}`/`''` | |
| `today` | **missing** | custom_configs | `''`, env | `|today|`; DefaultSubstitutions not ported in the parser either |
| `today_fmt` | **missing** | custom_configs | `None`, env | |
| `pygments_style` | **missing** (`output.highlight_theme = "github"` is unrelated) | custom_configs | `None`, html. `init_highlighter` (`SPHINX/builders/html/__init__.py:237-258`): config value if set, else the theme's `pygments_style` default or `'none'` (basic `theme.toml:12` = `"none"`; alabaster `theme.conf:5` = `alabaster.support.Alabaster`), `'sphinx'` only without a theme; plus `pygments_dark_style` ⇒ `pygments_dark.css` | drives `_static/pygments.css` |
| `highlight_language` | **missing** | custom_configs | `'default'`, **env** | `HighlightLanguageTransform` (`KNOWN_HIGHLIGHT_STAMP_GAPS`) |
| `highlight_options` | **missing** | custom_configs | `{}`, env | |
| `keep_warnings` | **missing** (env oracle skips it via `KNOWN_INERT_CONF`) | custom_configs | `False`, env | gates `FilterSystemMessages` |
| `suppress_warnings` | **missing** | custom_configs | `[]`, env | users will need it once reporter output prints |
| `show_warning_types` | **missing** (always on) | custom_configs | `True`, env | |
| `author` | **missing** (`ConfPyConfig.author` parsed, not copied) | | `'Author name not set'`, env | |
| `modindex_common_prefix` | `Vec<String>` `[]` | copied | `[]`, html | already consumed by modindex data |
| `extensions` | `Vec<String>`, `[autodoc, viewcode, intersphinx]` | copied | `[]` | YAML/no-config builds claim viewcode etc. |

Unused/non-Sphinx knobs that may confuse: `output.{syntax_highlighting,
highlight_theme, search_index, minify_html, compress_output}`,
`theme.{custom_css, custom_js}`, `template_dirs`, `static_dirs`,
`optimization.*` (`src/config.rs:330-385`).

### 7.3 Probe: Sphinx 9.1.0 html-category config (probe3.py, `project='P'`)

Every `rebuild == 'html'` value (these define the `.buildinfo` config hash;
note mathjax/qthelp/htmlhelp/singlehtml keys are included because their
extensions are loaded by default):
```
copyright = ''                         html_search_language = None
html4_writer = False                   html_search_options = {}
html_additional_pages = {}             html_secnumber_suffix = '. '
html_baseurl = ''                      html_short_title = 'P  documentation'
html_codeblock_linenos_style = 'inline' html_show_copyright = True
html_compact_lists = True              html_show_search_summary = True
html_context = {}                      html_show_sourcelink = True
html_copy_source = True                html_show_sphinx = True
html_css_files = []                    html_sidebars = {}
html_domain_indices = True             html_sourcelink_suffix = '.txt'
html_extra_path = []                   html_split_index = False
html_favicon = None                    html_static_path = []
html_file_suffix = None                html_style = None
html_js_files = []                     html_theme = 'alabaster'
html_last_updated_fmt = None           html_theme_options = {}
html_last_updated_use_utc = False      html_theme_path = []
html_link_suffix = None                html_title = 'P  documentation'
html_logo = None                       html_use_index = True
html_output_encoding = 'utf-8'         html_use_opensearch = ''
html_permalinks = True                 htmlhelp_file_suffix = None
html_permalinks_icon = '¶'             htmlhelp_link_suffix = None
html_scaled_image_link = True          mathjax2_config / mathjax3_config / mathjax4_config / mathjax_config = None
mathjax_config_path = ''               mathjax_display = ['\\[', '\\]']
mathjax_inline = ['\\(', '\\)']        mathjax_options = {}
mathjax_path = 'https://cdn.jsdelivr.net/npm/mathjax@4/tex-mml-chtml.js'
modindex_common_prefix = []            project_copyright = ''
pygments_style = None                  qthelp_basename = 'P'
qthelp_namespace = None                qthelp_theme = 'nonav'
qthelp_theme_options = {}              singlehtml_sidebars = {}
template_bridge = None                 templates_path = []
CONFIG_HASH 903c1988e93fbbb40659dd2c9bf2daf9
TAGS ['builder_html', 'format_html', 'html'] 645f666f9bcd5a90fca523b33c5a78b7
```
`env`-category keys (a change re-reads everything in Sphinx): `add_function_parentheses,
add_module_names, author, c_*, cpp_*, default_role, epub_*, exclude_patterns,
figure_language_filename, gettext_additional_targets, gettext_auto_build,
highlight_language, highlight_options, html_math_renderer, include_patterns,
javascript_*, keep_warnings, language, locale_dirs, manpages_url, master_doc,
math_eqref_format, math_number_all, math_numfig, math_numsep,
maximum_signature_line_length, numfig, numfig_format, numfig_secnum_depth,
option_emphasise_placeholders, primary_domain, project, python_*, release,
root_doc, rst_epilog, rst_prolog, show_authors, show_warning_types,
smartquotes, smartquotes_action, smartquotes_excludes, source_encoding,
source_suffix, strip_signature_backslash, suppress_warnings, text_*, tls_cacerts,
tls_verify, toc_object_entries, toc_object_entries_show_parents, today,
today_fmt, translation_progress_classes, trim_doctest_flags,
trim_footnote_reference_space, user_agent, version, xml_pretty`.

Rendered consequences with those defaults (p1 `index.html`):
`<title>Index &#8212; P  documentation</title>`; footer
`&#169;.` / `Powered by <a href="https://www.sphinx-doc.org/">Sphinx 9.1.0</a>
&amp; <a href="https://alabaster.readthedocs.io">Alabaster 1.0.0</a>` / `<a
href="_sources/index.rst.txt" rel="nofollow">Page source</a>`;
`documentation_options.js`: `VERSION: '',`; objects.inv header `# Project:
P` / `# Version: ` (trailing space before newline). Head rellinks:
`<link rel="index" title="Index" href="genindex.html" />`, `<link
rel="search" title="Search" href="search.html" />`, `<link rel="next"
title="B" href="b.html" />`, `<link rel="prev" title="Index"
href="index.html" />`.

### 7.4 Sphinx config-inited HTML validators (not ported; all count as warnings)

`SPHINX/builders/html/__init__.py:1295-1451`, connected at priority 800
(`:1529-1536`), i.e. alongside the `check_confval_types` pass the crate
already models in `BuildConfig::validate`:
- `convert_html_css_files` / `convert_html_js_files`: `invalid css_file: %r, ignored` / `invalid js_file: %r, ignored`
- `validate_html_extra_path`: `html_extra_path entry %r is placed inside outdir` / `html_extra_path entry %r does not exist`
- `validate_html_static_path`: `html_static_path entry %r is placed inside outdir` / `html_static_path entry %r does not exist`
- `validate_html_logo`: `logo file %r does not exist` (then `html_logo = None`)
- `validate_html_favicon`: `favicon file %r does not exist` (then `html_favicon = None`)
- `error_on_html_sidebars_string_values`: ConfigError
- `error_on_html_4`: ConfigError when `html4_writer`
- `validate_math_renderer` (builder-inited): ConfigError `Unknown math_renderer %r is given.`

Porting `validate_html_static_path` changes the no-config/YAML path (default
`["_static"]`) into a warning on every build of a tree without `_static`;
fix the default to `[]` first.

---

## 8. Recommended pipeline shape (for the design task)

```
build():
  discover (+ Sphinx EXCLUDE_PATHS, templates_path, static/extra paths)
  intersphinx
  plan_read / read / merge (persist doctrees)          [unchanged]
  numbering → rewrite_needed (keep the `changed` lists)
  consistency; env.save
  prepare_writing: relations = collect_relations(env); globalcontext; write_set
      html  : get_outdated_docs ∪ read ∪ rewrite_needed ∪ files_to_rebuild[·]
      dummy : found_docs
  copy_assets (downloads, static incl. pygments.css + theme `_t`, extra)   ← before docs
  write (sorted write_set; parallel map, sequential warning merge):
      doctree = in-memory clone
      post-transforms: xref resolve (builder URIs), highlight stamp,
                       FilterSystemMessages (keep_warnings), images …
      _resolve_toctree for every toctree node
      dummy: record pformat for snapshot_env
      html : write_doc_serialized (images, search feed) → translator → context → template
             → outdir/<docname><out_suffix>; _sources/<sourcename> (copy bytes of the source file)
  finish: genindex (builder URIs; create_index messages once), domain indices,
          search.html, copy images, .buildinfo, searchindex.js, objects.inv
  (crate-only) directive validation — reconsider default once reporter output prints
```
Reporter channel: record docutils messages at creation with a per-document
sequence shared with toctree/log records (store them in `RegistryExport` so
cache hits replay nothing — read-phase output is only printed for docs read
this build, exactly like Sphinx), render with a level
(`WARNING`/`ERROR`/`CRITICAL`), category `docutils`, `\n\n`-joined literal
bodies; count them as warnings.

---

## Appendix A — upstream citations used

- `SPHINX/builders/__init__.py`: `get_target_uri` 181-187, `get_relative_uri` 189-197, `build_all` 321-325, `build_update` 372-386, `build` 389-466, `read` 469-577, `_read_serial` 579-590, `read_doc` 632-671, `write_doctree` 674-703, `write` 705-748, `write_documents` 750-762, `_write_serial` 764-777, `_write_parallel` 779-818, `_write_docname` 877-890.
- `SPHINX/environment/__init__.py`: `find_files` 485-495, `check_dependents` 556-562, `get_doctree` 650-662, `get_and_resolve_doctree` 668-712, `apply_post_transforms` 759-776, `collect_relations` 778-795.
- `SPHINX/builders/html/__init__.py`: class attrs 109-137, `init` 162-186, `get_outdated_docs` 332-404, `get_asset_paths` 406-407, `prepare_writing` last_updated/relations 466-492, `get_doc_context` 564-642, `copy_assets` 644-648, `write_doc` 650-665, `write_doc_serialized` 667-673, `finish` 675-684, `gen_indices` 686-692, `gen_additional_pages` 701-720, `write_genindex` 722-750, `copy_image_files` 762-785, `create_pygments_style_file` 812-821, `copy_*` 823-948, `write_buildinfo` 950-954, `get_output_path` 1034-1035, `_get_sidebars` 1040-1060, `get_target_uri` 1067-1068, `handle_page` tail 1225-1257, `handle_finish`/`dump_inventory`/`dump_search_index` 1264-1289, validators 1295-1451, config registrations 1454-1537.
- `SPHINX/builders/html/_build_info.py` (`BuildInfo.load/__init__/dump`), `SPHINX/util/_serialise.py` (`stable_hash`).
- `SPHINX/builders/dummy.py` 26-30. `SPHINX/util/osutil.py` `relative_uri` 46-66.
- `SPHINX/transforms/__init__.py` `FilterSystemMessages` 337-347. `SPHINX/util/docutils.py` `report_re` 31, `WarningStream` 385-393, `LoggingReporter` 396+.
- `SPHINX/config.py` core options 217-263, copyright placeholders 695-760. `SPHINX/project.py` `EXCLUDE_PATHS` 20. `SPHINX/cmd/build.py` `-A` 379-387, doctreedir 313-316. `SPHINX/cmd/make_mode.py` 188-196. `SPHINX/domains/std/__init__.py` duplicate label 959-966.

## Appendix B — probe commands

```
PY="env PYTHONNOUSERSITE=1 uv run --python 3.12 --with sphinx==9.1.0 --with docutils==0.22.4 python"
$PY -m sphinx -b html src out            # p1 incremental write set (probe1.sh)
$PY -m sphinx -b html -q -w warn.txt src out   # p2 reporter format (probe2.sh)
$PY -m sphinx -b html -E -W src out3     # p2 -W count / stdout placement
$PY probe3.py                            # html-category config + defaults
$PY -m sphinx -b html|dummy -q src out   # p4 warm-build warnings html vs dummy
$PY -m sphinx -b html -q src out         # p5 validator-overlap messages
```
All under `/tmp/claude-0/-home-user-sphinx-ultra/46bf5e6b-694f-5b8e-ba0d-36f1851a8974/scratchpad/probe-pipeline/`.
