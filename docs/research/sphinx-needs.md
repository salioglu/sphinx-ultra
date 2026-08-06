# sphinx-needs Feature Inventory for sphinx-ultra Parity

**Verified against:** sphinx-needs docs @ readthedocs "latest" = **v8.3.0** (released 2026-08; 8.2.0 = 2026-07-01, 8.1.x = 2026-05-20, 8.0.0 = 2026-03-19, 7.0.0 = 2026-02-24, 6.x = late 2025). The prompt's "5.x/6.x" assumption is stale — the project is on **8.x**. Parity target should be 8.x semantics **plus** the deprecated-but-still-accepted legacy spellings (7.x-era projects in the wild still use `needs_extra_options`/`needs_extra_links`/`needs_global_options`).

**Version watershed summary (drives compat matrix):**
- **6.0.0**: JSON-schema validation (`needs_schema_definitions`), `network_back`, `schema_violations.json` export; 6.2 switched to `jsonschema-rs` (~3x faster)
- **7.0.0**: `needs_extra_options` → **`needs_fields`**, `needs_extra_links` → **`needs_links`** (unified, schema-carrying config); jinja2 → **minijinja**
- **8.0.0**: **conditional links** `TARGET_ID[filter_expr]`; `links_from_content` parses doctree (not regex)
- **8.1.x**: filter short-circuit optimization + caching
- **8.2.0**: **`needs_variant_data`** (`var.` namespace), **`if` directive**, **`variant` role**; `needs_filter_data` deprecated; open-needs service **removed**
- **8.3.0**: role templates rendered with **Jinja** (old `str.format` bracket syntax deprecated)

Legend: **[MUST]** required for a credible "first-class sphinx-needs" claim · **[SHOULD]** expected by real projects, deferrable briefly · **[NICHE]** rarely used / legacy / tooling-adjacent.

---

## 1. Core need objects

### 1.1 The `need` directive family **[MUST]**
Every type in `needs_types` becomes a directive (`req`, `spec`, `test`, `impl`, … fully user-defined). `needs_types` entries: `directive`, `title`, `prefix`, `color`, `style` (PlantUML node style). Builder: dynamic directive registration from config; type color/style feed needflow/legend rendering.

### 1.2 Need directive options — full list
| Option | Semantics | Priority |
|---|---|---|
| `id` | Must match `needs_id_regex` (default `^[A-Z0-9_]{5,}`); auto-generated from title/content hash when absent, length = `needs_id_length` (default 5, excl. prefix); `needs_id_required` forces explicit IDs; `needs_id_from_title` derives from title | MUST |
| `status` | single string | MUST |
| `tags` | `;`-separated, whitespace-stripped | MUST |
| `links` (+ every extra link type) | `;`-separated need IDs; supports `ID.part` and (8.0+) **conditional links** `ID[filter]`, `ID.part[filter]` | MUST |
| `collapse` | hide meta section behind toggle | MUST |
| `hide` | render nothing, still filterable | MUST |
| `delete` | drop need entirely (lists + needs.json) | SHOULD |
| `layout` / `style` | per-need layout/style override; `style` accepts dynamic functions | MUST |
| `template` / `pre_template` / `post_template` | `.need` Jinja templates from `needs_template_folder` (default `needs_templates/`); template replaces content, pre/post wrap it; need fields are the Jinja context | MUST |
| `jinja_content` | `true` ⇒ need content itself is Jinja-rendered against need data + `needs_render_context` | MUST |
| `title_from_content` | title = first sentence; global `needs_title_from_content`; `needs_max_title_length`; `needs_title_optional` allows titleless needs | MUST |
| `constraints` | names from `needs_constraints` to apply | SHOULD |
| `duration`, `completion` | Gantt fields (renameable via `needs_duration_option` / `needs_completion_option`) | SHOULD |
| `arch` | not a real option — populated by nested `needuml :key:`; dict of PlantUML snippets | SHOULD |
| every field from `needs_fields` / legacy `needs_extra_options` | free-form extra options | MUST |

### 1.3 Fields config — `needs_fields` (7.0+) and legacy **[MUST]**
`needs_fields[name]`: `description`, `schema` (JSON-schema type: string/boolean/integer/number/array + constraints — this is the typed-fields system), `nullable`, `default`, **`predicates`** (conditional defaults — the 7.0 successor of `needs_global_options` predicate form), `parse_variants` (enable `[variant]:value` parsing per field), and dynamic-function parsing toggle (`needs_parse_dynamic_functions` global default). Legacy accepted with deprecation warnings: `needs_extra_options` (list/dict), `needs_global_options` (old and 5.1 restructured "predicate" format). Builder: implement the new model, translate legacy spellings onto it.

### 1.4 Parts & nesting **[MUST]**
- **Need parts**: `:need_part:` / `:np:` role inside need content → sub-IDs (`ID.part_id`), linkable, appear in filters as pseudo-needs with `id_parent`, `id_complete`, `is_part`, inherited fields (`id`, `title`, `links_back` distinct); `needs_part_prefix` controls rendered prefix; dashed rendering in needflow; `show_parts` in needtable.
- **Nested needs**: needs inside needs via indentation; parent/child recorded (`parent_need`, `parent_needs`(link)); clusters in needflow.
- **`list2need` directive**: bullet-list → needs; `:types:` per indent level, `:delimiter:` (default `.`) title/content split, `:presentation:` nested|standalone, `:links-down:` (n−1 link types), `:tags:`; inline `(ID)` prefix and `((key="v", …))` metadata. **[SHOULD]**

### 1.5 Misc core config
`needs_include_needs` (master off-switch) [SHOULD]; `needs_from_toml` + `needs_from_toml_table` (config via TOML `[needs]`/`[tool.needs]`) [SHOULD — ubproject.toml ecosystem]; `needs_statuses`/`needs_tags` allow-lists: **gone from 8.x docs** — superseded by field `schema.enum`; accept-and-translate [NICHE]; `needs_hide_options` removed (layout-era) [NICHE].

**Builder must provide:** directive synthesis from config, hash-based ID generation identical to sphinx-needs (projects diff needs.json!), Jinja engine for templates/`jinja_content` (note upstream moved to **minijinja** semantics in 7.0), part/nested bookkeeping.

---

## 2. Filtering **[MUST — the load-bearing subsystem]**

- **Filter string** = Python expression `eval()`-ed per need/part. Namespace: all core fields (`id`, `title`, `type`, `status`, `tags`, `content`, `links`, all link types + `_back` backlink lists, `docname`, `lineno`, `is_need`, `is_part`, `is_external`, `is_modified`, `modifications`, `constraints_passed`, `parent_need`, `sections`/`section_name`, `signature`, `variant`…), all extra fields, part specials (`id_parent`, `id_complete`).
- Helpers: `search(pattern, field)` (re.search), `needs` (a `NeedsAndPartsListView` with `filter_types/filter_statuses/filter_ids/filter_has_tag/filter_is_external`), `c.this_doc()` (current-document scope), **`var.*`** namespace (8.2, from `needs_variant_data`/`needs_variant_data_file`; missing key ⇒ AttributeError), `current_need` (in dynamic-function/copy filters), `filter_warning` option text when zero matches.
- Common directive filter options: `:status:`, `:tags:`, `:types:` (semicolon lists, ANDed groups, OR within), `:filter:`, `:sort_by:`, `:filter_warning:`.
- **Filter code** (directive content as Python populating `results`) and **`:filter-func:`** `module.func(args)` from external file — both gated by `needs_allow_unsafe_filters`.
- Performance contract: 8.1 short-circuits recognized patterns (`id ==`, `id in [...]`, `type ==/in`, `status ==/in`, `'x' in tags`, `var.k ==`) without eval; `needs_filter_max_time` warning; `needs_debug_filters` → `debug_filters.jsonl`; deprecated `:export_id:` removed.
- Legacy `needs_filter_data` (flat extra names in filter namespace) — deprecated 8.2, still accepted. 

**Builder must provide:** a Python-expression-compatible evaluator (subset interpreter or embedded Python) — semantics must match CPython truthiness/comparisons; the fast-path optimizer; per-filter caching.

---

## 3. Presentation directives

| Directive | Key options (beyond common filters) | Backend needed | Priority |
|---|---|---|---|
| `needtable` | `columns` (incl. `field as "Title"`), `colwidths`, `style` table\|datatables (`needs_table_style`, `needs_table_columns`, `needs_table_classes`), `show_filters`, `show_parts`, `sort`, `class` | HTML table + **DataTables JS assets** shipped by builder | MUST |
| `needlist` | `show_status`, `show_tags`, `show_filters` | plain list | MUST |
| `needflow` | `engine` plantuml\|graphviz (`needs_flow_engine`), `link_types`, `config` (`needs_flow_configs`), `graphviz_style` (`needs_graphviz_styles`), `root_id`/`root_direction` both\|incoming\|outgoing/`root_depth`, `highlight` (filter⇒red border), `border_color` (variant-syntax hex), `show_filters`, `show_legend`, `show_link_names` (`needs_flow_show_links`), `scale` 1–300, `align`, `alt`, `debug`; nested needs ⇒ clusters/subgraphs, parts ⇒ dashed | **PlantUML AND Graphviz** invocation | MUST |
| `needpie` | argument=title, content = per-slice filter strings or literals, `labels`, `legend`, `explode`, `shadow`, `colors`, `text_color`, `style` (matplotlib style names), `filter-func`; auto label-overlap fallback legend | chart renderer (upstream: **matplotlib**) | MUST |
| `needbar` | content = 2-D grid of filters/literals, `legend`, `colors`, `text_color`, `style`, `x_axis_title`, `y_axis_title`, `xlabels`/`ylabels` (+`FROM_DATA`), `xlabels_rotation`/`ylabels_rotation`/`sum_rotation`, `separator`, `stacked`, `show_sum`, `show_top_sum`, `transpose`, `horizontal` | chart renderer | SHOULD |
| `needgantt` | `milestone_filter`, `starts_with_links`/`starts_after_links` (default `links`)/`ends_with_links`, `start_date` YYYY-MM-DD, `timeline` daily\|weekly\|monthly, `no_color`, `duration_option`, `completion_option` | PlantUML gantt | SHOULD |
| `needsequence` | `start` (IDs), `link_types`, `filter`; alternating participant/message walk over links | PlantUML sequence | NICHE |
| `needuml` | content = **Jinja-templated PlantUML**; options `key` (store into `arch`, ≠"diagram"), `save` (path for needumls export), `scale`, `align`, `config`, `debug`, `extra` (k:v pairs); Jinja context: `needs` dict, `need()`, `flow(id)`, `filter(str)`, `ref(id, option=/text=)`, `uml(id, key=, **kwargs)` with recursive import (allowmixing) | PlantUML + Jinja | MUST |
| `needarch` | needuml restricted to inside a need; adds `import(*link_types)` to pull `arch` from linked needs, `need()` = enclosing need | same | SHOULD |
| `needextract` | argument=IDs or `:filter:` (mutually exclusive), `layout`, `style`; renders copies (known upstream limitation: transform-heavy content may not survive) | doctree copying | SHOULD |
| `needreport` | flags `types`, `links`, `options`/fields, `usage`; `needs_report_template` (Jinja; default dropdown-based) | Jinja | NICHE |
| `if` (8.2) | argument = Python expr over `var.*` only; parse-time pruning (content never parsed); nestable; no elif/else | variant evaluator | SHOULD |

**Builder must provide:** PlantUML pipeline (sphinxcontrib-plantuml-equivalent: jar/server invocation, SVG/PNG), Graphviz `dot` invocation, a matplotlib-equivalent chart renderer (or embed a plotting lib) for pie/bar, DataTables asset bundling, `scale/align/debug` handling on generated images.

---

## 4. Data flow (import/export/modify)

- **`needextend`** **[MUST]**: argument = `<ID>` | `"filter"` | bare ID | multiword filter (that priority); `:option: v` set, `:+option: v` append, `:-option:` delete; works on string/list fields, tags, links (backlinks auto-recalculated); `:strict:` + `needs_needextend_strict`; sets `is_modified`, `modifications` count; applied post-collection, before dynamic-function resolution of dependents.
- **`needimport`** **[MUST]**: arg = path (doc-relative, `/`=srcdir, `//`=absolute, `c:/`=win) or **http(s) URL** or key from `needs_import_keys`; options `version` (default file's `current_version`), `ids`, `filter`, `id_prefix` (rewrites link references too), `tags`, `hide`, `collapse`, `layout`, `style`, `template`, `pre_template`, `post_template`, `allow_type_coercion` (default true — re-parse dynamic funcs/types); imported needs are first-class.
- **`needs_external_needs`** **[MUST]**: list of `{base_url, target_url (Jinja over need), json_url|json_path, version, id_prefix, css_class (default external_link), allow_type_coercion}`; needs exist only as link targets (`is_external`), no local rendering; excluded from needs builder by default via `needs_builder_filter` default `is_external==False`.
- **needs.json export** **[MUST — fidelity is the ecosystem contract]**: `needs` builder (`-b needs`) → `needs.json` with `current_version`, versioned `versions.{v}.needs` keyed by ID, `filters`, and `needs_schema` (draft-07 schema per-field with `field_type`: core|links|backlinks|extra|global, `needs_defaults_removed` flag). Config: `needs_build_json` (emit during html), `needs_file` (input for version history merging), `needs_builder_filter`, `needs_reproducible_json` (strip timestamps), `needs_json_exclude_fields`, `needs_json_remove_defaults`, `needs_json_include_link_conditions` (8.0), `needs_build_json_per_id` + `needs_build_json_per_id_path` (`needs_id` builder, one file per need).
- **`needumls` builder** + `needs_build_needumls`: export `:save:`-marked PlantUML sources. **[NICHE]**
- **Permalinks** **[SHOULD]**: `needs_permalink_file` (default `permalink.html` — self-contained JS page resolving `?id=` against `needs_permalink_data`, default `needs.json`); `permalink()` layout function.
- **`schema_violations.json`** export (see §8). **[SHOULD]**

---

## 5. Links

- **`needs_links`** (7.0; legacy `needs_extra_links` list still parsed) **[MUST]**: per link type — `option` name, `incoming`/`outgoing` display names (optional since 6.1), `copy` (copy into common `links`), `allow_dead_links` (dead targets tolerated, flagged `has_dead_links`/`has_forbidden_dead_links`), `style`/`style_part`/`style_start`/`style_end` (PlantUML edge styling), `schema` (minItems/maxItems/etc — §8). Every type auto-creates `<name>` and `<name>_back` fields.
- **Backlink computation** for all types **[MUST]**.
- **Conditional links** (8.0) `ID[expr]`, evaluated against target at build; failures warn; serialized in needs.json (`needs_json_include_link_conditions`). **[SHOULD]**
- **Link chains in filters**: `'ID' in links_back`, traversal helpers in filter code. **[MUST]**
- **Roles**: `:need:` (rendered via `needs_role_need_template` — **Jinja since 8.3**, `str.format` `[[field]]` deprecated; `needs_role_need_max_title_length` default 30), `:need_outgoing:`, `:need_incoming:` (ID lists per link type), `:need_count:` (filter; `filter1 ? filter2` ratio syntax; counts parts too), `:need_part:`/`:np:`, `:ndf:` (§6), `:variant:` (8.2 — resolves `var.` dotted path at parse time, scalars+arrays). **[MUST, `variant` SHOULD]**
- Role display config: `needs_show_link_type`, `needs_show_link_title`, `needs_show_link_id`. **[SHOULD]**
- `needs_string_links` (regex → hyperlink rendering of field values: `regex`, `link_url`, `link_name`, `options`). **[SHOULD]**
- Dead-link reporting: modern route = `suppress_warnings`-style categories + `allow_dead_links`; old `needs_report_dead_links` removed. **[SHOULD]**
- `needs_default_link_type`: not in 8.x docs (removed) — accept/ignore. **[NICHE]**

---

## 6. Dynamic functions **[MUST]**

- Syntax: `[[func(args)]]` **in options** (status, tags, style, layout, constraints, extra fields, link fields); `:ndf:\`func(...)\`` role **in content** (old `[[...]]`-in-content deprecated 3.1, removed later). `needs_parse_dynamic_functions` global toggle; per-field opt-out in `needs_fields`.
- Execution: after all needs collected (full `needs` view available); incoming links NOT yet final during execution; results cached; assigning functions in conf.py kills incremental builds (upstream caveat).
- Built-ins: `test(...)`, `echo(str)`, `copy(option, need_id=None, lower=False, upper=False, filter=None)` (supports `current_need[...]` in filter), `check_linked_values(result, search_option, search_value, filter_string=None, one_hit=False)`, `calc_sum(option, filter=None, links_only=False)`, `links_from_content(need_id=None, filter=None)` (8.0: doctree-based `:need:` role harvesting).
- Registration: `needs_functions = [func,...]` in conf.py or `add_dynamic_function(app, func, name=None)`; signature `f(app, need, needs, *args, **kwargs)`.

**Builder must provide:** a function VM/registry; for conf.py-registered *Python* functions in a Rust builder you need a Python-embedding story or a documented Rust/WASM plugin equivalent — this is the hardest compat point, flag it.

---

## 7. Constraints (legacy engine — still fully supported in 8.3) **[SHOULD]**

- `needs_constraints = {name: {check_0..check_N: filter_expr, severity: CRITICAL|HIGH|MEDIUM|LOW, error_message: jinja_str}}`; applied via need `:constraints:` option; all checks must pass.
- Results on need: `constraints_passed` (bool), `constraints_results` (per-check dict), `constraints_error`.
- `needs_constraint_failed_options = {SEVERITY: {on_fail: [warn|break], style: [...], force_style: bool}}` — `warn` ⇒ Sphinx warning, `break` ⇒ abort build, style injection onto failing needs; `needs_constraints_failed_color` (older, removed from 8.x docs — accept/ignore).

---

## 8. Schema validation (6.0+, the strategic system) **[MUST for 8.x parity]**

- `needs_schema_definitions` (inline) / `needs_schema_definitions_from_json` (recommended file); `needs_schema_validation_enabled` (default true).
- Structure: `{"$defs": {reusable fragments, $ref, no recursion}, "schemas": [{id, severity: info|warning|violation (default violation), message, select (JSON-schema predicate; omitted ⇒ all needs), validate: {local, network, network_back}}]}`.
- Typed fields: `needs_fields[*].schema` / `needs_links[*].schema` (links always `array[string]`, `minItems`/`maxItems` on raw ID lists); types auto-injected into schema defs; string formats (date, email, uri, uuid), enum/const/pattern (safe-regex subset: no lookarounds/backrefs), numeric bounds, array constraints.
- **local**: per-need properties, `required`, `unevaluatedProperties: false` (allOf-aware evaluation rules).
- **network**: outgoing per-link-type `{contains: {local…, network… (nested ≤4 hops)}, minContains, maxContains}`; **network_back** (6.0/8.2): same over incoming links, select always targets the validated need.
- Reporting: warning categories `sn_schema_violation|sn_schema_warning|sn_schema_info` with subtypes `field_fail`, `link_fail`, `local_fail`, `network_missing_target`, `network_contains_too_few`, `network_contains_too_many`, `network_items_fail`; suppressible via `suppress_warnings`; rich context (need path, schema path, messages).
- **`schema_violations.json`** export (summary, needs/sec, per-need violation records with children chains).
- Debug: `needs_schema_debug_active`, `needs_schema_debug_path` (default `schema_debug`), `needs_schema_debug_ignore`.
- Perf bar: upstream uses `jsonschema-rs` — a Rust builder should beat 1.3k needs/sec validation easily.

---

## 9. Services **[SHOULD overall]**

- `needservice` directive (any service; plus any `needs_fields`/`needs_links` values as options; content appended to generated needs); `needs_services` config (`class`, `class_init`, per-service options); `needs_service_all_data` (dump unknown returned fields into content).
- **GitHub service** (`github-issues`, `github-prs`, `github-commits`): `query`/`specific`, `max_amount`, `max_content_lines`, `id_prefix`, `url` (GH Enterprise), auth `username`/`token`, avatar download, rate-limit handling. **[SHOULD]**
- **Open-Needs service: REMOVED in 8.2.0** — do not implement; note for migration. **[skip]**
- Custom services: `BaseService` subclass with `request(options)` (list of need dicts) / newer `request_from_directive`, optional `debug(options)` (Sphinx-Data-Viewer output); multiple instances of one class. Same Python-plugin caveat as §6. **[NICHE]**

---

## 10. Layouts & styles **[MUST]**

- Built-in layouts: `clean` (default, `needs_default_layout`), `clean_l/r/lp/rp`, `complete`, `focus`, `focus_f/l/r`, `debug`.
- Grids: `simple`, `simple_footer`, `simple_side_left/right`, `simple_side_left_partial/right_partial`, `complex` (9 areas), `content`, `content_footer`, `content_side_left/right`, `content_footer_side_left/right`. Areas: head/meta/side/footer(+left/right variants)/content (fixed).
- `needs_layouts = {name: {grid, layout: {area: [lines with <<func()>>]}}}`; line joining, multiple lines per area.
- Layout functions (exact arg lists): `meta(name, prefix, show_empty)`, `meta_id()`, `meta_all(prefix, postfix, exclude, no_links, defaults, show_empty)`, `meta_links(name, incoming)`, `meta_links_all(prefix, postfix, exclude)`, `image(url, height, width, align, no_link, prefix, is_external, img_class)` with `icon:` (Feather icons — builder must bundle) and `field:` prefixes, `link(url, text, image_url, image_height, image_width, prefix, is_dynamic)`, `permalink(image_url, image_height, image_width, text, prefix)`, `collapse_button(target, collapsed, visible, initial)` (no-op in LaTeX).
- Styles: `green/red/yellow/blue/discreet` (+bg aliases implemented/open/in_progress), `*_border`, `red/orange/yellow/green/blue_bar`, `clean`, comma-combinable; `needs_default_style`; dynamic-function-computable.
- CSS: `needs_css` = blank|modern|dark|path; generated classes `need`, `needs_grid_<grid>`, `needs_layout_<layout>`, `needs_style_<style>`, `needs_type_<type>` plus status/tag classes on need tables; `needs_table_classes`.
- **Builder must provide:** the whole grid/area HTML (and LaTeX) rendering engine, the `<<…>>` mini-language parser, Feather icon assets, three CSS themes.

---

## 11. Warnings **[SHOULD]**

- `needs_warnings = {id: filter_str | callable(need, log)}` — match ⇒ warning `needs_warning.<id>` (fails `-W` builds); `needs_warnings_always_warn` (log-to-file mode with `-w`); `add_warning()` API. Build warnings integrate with `suppress_warnings` categories (`needs.*`, `sn_schema*`).
- Dead-link reporting via link config (§5); legacy `needs_report_dead_links` removed.

---

## 12. Remaining config (catch-all)

**[MUST]** `needs_id_regex`, `needs_id_length`, `needs_id_required`, `needs_id_from_title`, `needs_title_optional`, `needs_title_from_content`, `needs_max_title_length`, `needs_role_need_template`, `needs_role_need_max_title_length`, `needs_table_*`, `needs_flow_*`, `needs_graphviz_styles`, `needs_diagram_template` (needflow node content template), `needs_template_folder`, `needs_render_context` (extra Jinja vars), `needs_parse_dynamic_functions`.

**[SHOULD]** `needs_variant_data` / `needs_variant_data_file` (8.2 structured variant store; powers `var.` filters, `if`, `variant` role, per-field `parse_variants` `[variant]:value` / legacy `needs_variants` + variant option syntax `va == v1:value; other`), `needs_from_toml(_table)`, `needs_import_keys`, `needs_needextend_strict`, `needs_filter_max_time`, `needs_uml_process_max_time`, `needs_permalink_file/data`, `needs_service_all_data`, `needs_duration_option`, `needs_completion_option`, all §4 JSON knobs, `needs_include_needs`.

**[NICHE/legacy accept-and-warn]** `needs_extra_options`, `needs_extra_links`, `needs_global_options`, `needs_filter_data`, `needs_statuses`, `needs_tags`, `needs_hide_options`, `needs_default_link_type`, `needs_report_dead_links`, `needs_constraints_failed_color`.

Also: incremental-build correctness (env pickling of needs data, `-E` semantics for remote needimport) — **[MUST]** for a builder claiming Sphinx parity.

---

## 13. IDE / tooling contracts **[SHOULD]**

- **needs.json is the API**: ubCode (useblocks' VS Code extension/LSP) and the older needs-vscode consume `needs.json` (+ per-ID files) for completion, go-to-def, live validation; **byte-level schema fidelity** (field_type annotations, versions structure, `needs_schema`) is the compatibility bar. `ubproject.toml` (`needs_from_toml`) is how ubCode shares config with the build.
- `schema_violations.json` consumed by CI dashboards.
- `needs_debug_measurement` — runtime/timing report (`debug_measurement.json` + HTML); `needs_debug_filters` → `debug_filters.jsonl`. Reimplement as equivalent profiling output. **[NICHE]**

---

## 14. Extension API **[SHOULD; MUST if you want the ecosystem]**

Public surface other extensions call (e.g. Score/PM tooling, test-report importers):
- Config-time: `add_need_type(app, directive, title, prefix, color, style)`, `add_field(...)` (7.0; deprecated `add_extra_option`), `add_dynamic_function(app, func, name)`, `add_warning(app, name, filter|func)`.
- Runtime: `add_need(app, docname, lineno, need_type, title, id=…, content, status, tags, links…, constraints, layout, template, …) → nodes` (raises `InvalidNeedException`), `add_external_need(...)` (returns `[]`), `del_need(app, id)`, `generate_need(...)` (validate without insert), `get_needs_view(app)` (post-write read-only `NeedsView` with fast filter methods; `NeedsAndPartsListView`; `NeedItem`/`NeedPartItem` with `get/get_links/get_backlinks/get_extra`; `NeedLink` parsing `ID`, `ID.part`, `ID[cond]`, `ID.part[cond]`).
- Data in env: needs store accessible to other extensions/events; exceptions `NeedsApiConfigException`, `FunctionParsingException`, `VariantParsingException`.
- For sphinx-ultra: expose an equivalent Rust API + (if Python interop exists) a shim mirroring `sphinx_needs.api`; otherwise document divergence — third-party Sphinx extensions calling this API are the main incompatibility risk, alongside Python callables in conf.py (dynamic functions, warnings, services, filter-func).

---

## Cross-cutting builder obligations (summary)

1. **Python-expression filter evaluator** with CPython semantics + 8.1 fast paths — MUST.
2. **Jinja (minijinja-compatible) templating** for content templates, jinja_content, needuml, role templates (8.3), target_url, error messages — MUST.
3. **PlantUML + Graphviz invocation** (flow/gantt/sequence/uml/arch; config presets, scale/align/debug) — MUST.
4. **Chart rendering** (pie/bar; matplotlib-style options incl. named styles/colors) — MUST/SHOULD.
5. **needs.json read+write at full fidelity** (versions, needs_schema, field_type, conditional links, reproducible mode, per-id) — MUST.
6. **JSON-schema validation engine** with select/local/network/network_back, severities, violations export — MUST.
7. **Layout grid renderer + `<<fn>>` parser + CSS themes + DataTables + Feather icons** — MUST.
8. **Build orchestration**: post-collection pipeline order = collect → needextend → dynamic functions → backlinks → constraints/schema/warnings → render; incremental caching of all of it — MUST.
9. **Python interop or declared divergence** for conf.py callables (dynamic functions, warnings funcs, services, filter_func, API) — decide explicitly; this is the only area where "NO feature excluded" is technically contingent.