# Configuration

sphinx-ultra reads a Sphinx `conf.py` (a declarative subset), or its own
YAML/JSON file, and accepts `-D` overrides on the command line. This page
lists what is parsed, what the build actually consumes today, and the YAML
schema. Status per key is taken from the audited
[IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md).

## Discovery order

1. `conf.py` in the source directory (or the directory given with `-c`)
2. `sphinx-ultra.yaml`
3. `sphinx-ultra.yml`
4. `sphinx-ultra.json`
5. Built-in defaults

In sphinx-build mode one of the first four must exist (exit 2 otherwise). In
native mode `--config FILE` selects a file explicitly: `.py` goes through the
`conf.py` parser, anything else through serde. Every YAML/JSON field is
optional; a partial file loads over the defaults.

## `conf.py`: parsed, not executed

No Python interpreter runs. A logical-statement scanner and a Python literal
parser read assignments of:

- strings, including raw and prefixed strings, triple-quoted strings and
  adjacent-string concatenation;
- integers, floats, booleans, `None`;
- lists, tuples and dicts, nested and multi-line, with comments.

Every construct the parser cannot evaluate (`import`, function calls,
`os.environ`, f-strings, `sys.path` manipulation, arithmetic) is dropped with
a warning that names the line:

```
conf.py:4: unsupported value for 'html_theme' dropped (unsupported expression): os.environ.get(...)
```

These parser warnings are logged but **not** counted toward `-W` and not
written to the `-w` file yet. Executing a dynamic `conf.py` inside the
project's virtualenv is the M5 Python sidecar.

Every assignment that parses is kept as its literal in `conf_values`, which is
what the `.buildinfo` config hash is computed from (so the hash matches
`sphinx-build`'s even for keys the build does not otherwise use).

## Keys the build consumes

| Key | Effect | Since |
|---|---|---|
| `project`, `version`, `release`, `copyright`, `author`, `language` | Metadata; `project`/`version` go into `objects.inv`'s header. `version` and `release` default to empty, as in Sphinx | 0.1 / 0.6 |
| `root_doc` / `master_doc` | Root of the toctree graph | 0.5 |
| `include_patterns`, `exclude_patterns` | Source discovery, with Sphinx 9.1's exact pattern semantics ([SPHINX_PATTERNS_COMPATIBILITY.md](SPHINX_PATTERNS_COMPATIBILITY.md)) | 0.4 |
| `source_encoding` | Encoding of files inserted by `include`/`literalinclude` (default `utf-8-sig`); a non-UTF-8 value prints Sphinx's deprecation warning | 0.5 |
| `numfig`, `numfig_format`, `numfig_secnum_depth` | Figure/table/code-block numbering and `:numref:` rendering | 0.5 |
| `nitpicky`, `nitpick_ignore`, `nitpick_ignore_regex` | Reference-resolution strictness | 0.4 / 0.5 |
| `intersphinx_mapping`, `intersphinx_disabled_reftypes`, `intersphinx_resolve_self`, `intersphinx_cache_limit`, `intersphinx_timeout` | Intersphinx, validated with Sphinx's own `ConfigError` texts | 0.5 |
| `tls_verify`, `tls_cacerts`, `user_agent` | The HTTP settings used when fetching inventories | 0.5 |
| `maximum_signature_line_length`, `python_maximum_signature_line_length`, `python_trailing_comma_in_multi_line_signatures`, `python_display_short_literal_types`, `python_use_unqualified_type_names`, `toc_object_entries`, `toc_object_entries_show_parents`, `add_function_parentheses`, `add_module_names`, `strip_signature_backslash` | Object-signature rendering and toc entries for `py:*` objects | 0.5 |
| `modindex_common_prefix` | `py-modindex` grouping | 0.5 |
| `validate_directives` | Run the built-in directive/role validators (default on; `-D validate_directives=0` disables) | 0.4 |
| `fail_on_warning` | Same as `-W` | 0.1 |
| `parallel_jobs` | Read-phase parallelism (`-j`) | 0.1 |
| `doctree_dir` | Cache directory (`-d`) | 0.4 |
| `max_cache_size_mb`, `cache_expiration_hours` | Cache retention | 0.4 |
| `html_context`, `tags` | Only hashed into `.buildinfo` (no templates, no `only::` evaluation yet) | 0.5 / 0.6 |

Keys that are **parsed and hashed into `.buildinfo` but otherwise inert**
until the HTML writer and theme engine land: `html_theme`,
`html_theme_options`, `html_static_path`, `html_css_files`, `html_js_files`,
`html_logo`, `html_favicon`, `html_title`, `html_short_title`,
`html_show_copyright`, `html_show_sphinx`, `html_copy_source`,
`html_show_sourcelink`, `html_sourcelink_suffix`, `html_use_index`,
`html_use_opensearch`, `html_last_updated_fmt`, `templates_path`,
`html_style`, `extensions`.

Two config-inited checks from Sphinx run after loading: an `html_logo` or
`html_favicon` that is neither a file under the configuration directory nor a
URL is dropped with Sphinx's warning, and the two `*_signature_line_length`
keys are type-checked the way `check_confval_types` checks them (a `-D`
string value warns and leaves the key unset, as it does under Sphinx).

## Overrides on the command line

```bash
sphinx-ultra docs _build -D project="My Docs" -D numfig=1 -D exclude_patterns=_build,drafts \
                          -D html_context.version=2.0 -A theme_color=blue -t draft -n
```

- `-D key=value` is coerced against the field's existing type: booleans
  accept `0/1/true/false`, lists are comma-split, dotted paths reach nested
  sections (`output.html_theme=furo`) and map members
  (`html_context.name=value`). Keys that Sphinx keeps duplicated
  (`html_theme`/`theme.name`, `templates_path`/`template_dirs`,
  `html_static_path`/`static_dirs`) are synced. An unknown key warns with
  sphinx-build's text and counts toward `-W`.
- `-A name=value` sets an `html_context` entry.
- `-t TAG` defines a tag. Tags reach the `.buildinfo` hash; the `only::`
  directive stores its expression but does not evaluate it yet.
- `-n` sets `nitpicky`.

## YAML / JSON schema

The native schema is this crate's `BuildConfig`, not the `conf.py` key set:
the `output`, `theme` and `optimization` sections are nested. Everything is
optional.

```yaml
# Project metadata
project: "My Documentation"
version: "1.0"          # default: "" (Sphinx's default)
release: "1.0.0"
copyright: "2026, My Company"
author: "Me"
language: "en"
root_doc: "index"

# Discovery
include_patterns: ["**"]
exclude_patterns: ["_build/**", "drafts/**"]
source_encoding: "utf-8-sig"

# Build behaviour
parallel_jobs: 8              # default: number of cores
fail_on_warning: false
validate_directives: true
nitpicky: false
nitpick_ignore: []            # [["py:class", "object"]]
nitpick_ignore_regex: []
doctree_dir: null             # default: OUTPUT/.sphinx-ultra-cache
max_cache_size_mb: 500
cache_expiration_hours: 24

# Numbering
numfig: false
numfig_format: {}             # {figure: "Fig. %s", table: "Table %s", code-block: "Listing %s"}
numfig_secnum_depth: 1

# Intersphinx
intersphinx_mapping: {}       # {python: ["https://docs.python.org/3", [null]]}
intersphinx_disabled_reftypes: ["std:doc"]
intersphinx_resolve_self: ""
intersphinx_cache_limit: 5    # days; negative = never refresh
intersphinx_timeout: null
tls_verify: true
tls_cacerts: null             # a path, or {host: path}
user_agent: null

# Object signatures (py domain)
maximum_signature_line_length: null
python_maximum_signature_line_length: null
python_trailing_comma_in_multi_line_signatures: true
python_display_short_literal_types: false
python_use_unqualified_type_names: false
toc_object_entries: true
toc_object_entries_show_parents: "domain"
add_function_parentheses: true
add_module_names: true
strip_signature_backslash: false
modindex_common_prefix: []

# HTML keys: parsed and hashed into .buildinfo, inert until the HTML writer
html_static_path: ["_static"]
templates_path: ["_templates"]
html_css_files: []
html_js_files: []
html_logo: null
html_favicon: null
html_title: null
html_context: {}
extensions: []                # listing an extension has no effect yet

# Nested sections (inert until the HTML writer / theme engine)
output:
  html_theme: "sphinx_rtd_theme"
  syntax_highlighting: true
  highlight_theme: "github"
  search_index: true
  minify_html: false
  compress_output: false
theme:
  name: "sphinx_rtd_theme"
  options: {}
  custom_css: []
  custom_js: []
optimization:
  parallel_processing: true
  incremental_builds: true
  document_caching: true
  image_optimization: false
  asset_bundling: false
```

The repository's own [`sphinx-ultra.yaml`](../sphinx-ultra.yaml) and
[`examples/basic/sphinx-ultra.yaml`](../examples/basic/sphinx-ultra.yaml) are
loaded by a unit test, so they always parse against the current schema.

## Cache invalidation

A blake3 fingerprint of the loaded configuration is stored in the cache
directory. Any change to a content-bearing key wipes the cache and triggers a
full read. `-W`/`fail_on_warning`, `-n`/`nitpicky` and the order of
`html_context` entries are excluded from the fingerprint, because they change
diagnostics, not output.
