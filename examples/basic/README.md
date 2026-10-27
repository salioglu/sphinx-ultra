# Basic example

The minimal layout for a sphinx-ultra project:

- `sphinx-ultra.yaml`: the native configuration (every field optional; see
  [docs/configuration.md](../../docs/configuration.md))
- `index.rst`: the root document with a `toctree`
- `validation-examples.rst`: a page exercising directives the built-in
  validators check

## Build

```bash
# From this directory
sphinx-ultra . _build/html -b html          # sphinx-build style
sphinx-ultra build --source . --output _build   # native style
```

`_build/` is git-ignored. There is no live-reload server yet (roadmap M3).
