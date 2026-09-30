# M2 wave 5 research notes (2026-09-30)

Probe-verified specifications written before wave 5's design, one per area.
Each note cites crate code as `src/…:line` (at commit `f353db9`) and upstream
code as `SP/…` (Sphinx 9.1.0), `DU/…` (docutils 0.22.4) or Pygments 2.21.0,
all from the pinned environment:

    PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' \
        --with 'docutils==0.22.4' --with 'pygments==2.21.0' python <script>

| Note | Covers |
|---|---|
| `…-pipeline.md` | the crate's build pipeline, write path, warning rendering, HTML config keys |
| `…-doctree.md` | the doctree IR, a census of every node kind, parse-time divergences |
| `…-env.md` | environment data a writer reads, builder/URI spec, toctree resolution, exemption map |
| `…-transforms.md` | every read transform and post-transform Sphinx runs, in order, with gap table T1–T21 |
| `…-reporter-oracle.md` | the docutils reporter channel, and the HTML-oracle design |
| `…-translator-docutils.md` | docutils' HTML5 translator as Sphinx drives it |
| `…-translator-sphinx.md` | Sphinx's HTML5Translator, PygmentsBridge and HtmlFormatter |
| `…-htmlbuilder.md` | StandaloneHTMLBuilder, dirhtml, dummy, templates, finish tasks, `.buildinfo` |
| `…-deadstack.md` | the unwired write-side modules, and minijinja vs Sphinx's Jinja2 templates |

`probes/` holds the probe scripts and source projects the notes quote (build
outputs omitted). Paths inside the notes that point at a `scratchpad/probe-*`
directory refer to these files: `scratchpad/probe-X/…` is `probes/probe-X/…`.
