# Examples

## `basic/`: a minimal project

A two-page project with a `sphinx-ultra.yaml` configuration. It shows the
layout sphinx-ultra expects and is loaded by a unit test, so its
configuration always matches the current schema.

```bash
# From the repository root, sphinx-build style
sphinx-ultra examples/basic examples/basic/_build/html -b html

# Or with the native interface
sphinx-ultra build --source examples/basic --output examples/basic/_build
```

The output tree contains `searchindex.js`, `objects.inv` and `.buildinfo` in
Sphinx 9.1's formats and one `.html` file per page. Note that the page body
is still a placeholder (the escaped source), see the project
[README](../README.md#what-works-today).

## Library examples (`cargo run --example`)

Two Rust examples exercise library APIs that the binary does not use yet:

| Example | What it shows |
|---|---|
| `cargo run --example directive_validation` | The directive and role validators that run in every build, called directly on RST text |
| `cargo run --example constraint_validation` | The sphinx-needs-style constraint engine (a library module; nothing produces items for it in the build until sphinx-needs support lands, roadmap M6) |

## Fixture projects

The projects under [`tests/fixtures/`](../tests/fixtures/) (`basic`,
`basic_missing_ref`, `deps_image`, `intersphinx`, `literalinclude`,
`toctree_forms`, `toctree_glob`) are small Sphinx projects used by the
end-to-end suite and by the HTML oracle; each has a `conf.py` and can be built
the same way.
