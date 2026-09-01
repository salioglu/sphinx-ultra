# Sphinx Ultra Rust Builder

[![Crates.io](https://img.shields.io/crates/v/sphinx-ultra.svg)](https://crates.io/crates/sphinx-ultra)
[![CI](https://github.com/salioglu/sphinx-ultra/actions/workflows/ci.yml/badge.svg)](https://github.com/salioglu/sphinx-ultra/actions/workflows/ci.yml)
[![Documentation](https://github.com/salioglu/sphinx-ultra/actions/workflows/docs.yml/badge.svg)](https://salioglu.github.io/sphinx-ultra)
[![Release](https://github.com/salioglu/sphinx-ultra/actions/workflows/release.yml/badge.svg)](https://github.com/salioglu/sphinx-ultra/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![Sponsor](https://img.shields.io/badge/sponsor-GitHub-pink.svg)](https://github.com/sponsors/salioglu)

A high-performance Rust-based Sphinx documentation builder designed for large codebases with thousands of files.

## ⚠️ Development Status

**🚧 Pre-1.0: not yet recommended for production documentation workflows.**

**Mission (see [ROADMAP.md](ROADMAP.md)):** sphinx-ultra 1.0 will be a
production-grade, drop-in replacement for `sphinx-build -b html` — full Sphinx
feature parity (target: Sphinx 9.1.x), **sphinx-needs built in as a first-class
feature** (target: 8.3.x), support for the 5 most popular themes and 15+ most
popular extensions — at 10–100× the speed. **No Sphinx or sphinx-needs feature is
excluded from scope**; features are phased, never excluded. The earlier
validation-only scoping is retired.

**Honest current state (verified by code audit, 2026-08-31):** everything up to the
page render is real — a docutils-fidelity RST parser, a `BuildEnvironment` with the
toctree graph, numbering, the std domain, index data and intersphinx, and Sphinx's
own warnings coming out of all of it, each pinned against a real `sphinx-build`
9.1.0 by committed differential fixtures. **The HTML is still a placeholder**: the
build writes the source text, escaped, with no rendering, themes, search index,
`genindex.html` or `objects.inv`. That is the M2 wave-5 HTML writer. The Markdown
parser is still a prototype (`.md` titles and TOCs come out empty), and `conf.py`
support is a declarative subset — dynamic values need the M5 Python sidecar. The
full, file-and-line-level status audit lives in
[docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md).

## ✨ Features

### ✅ Working today

- **🚀 Parallel build pipeline**: Rayon-based, scales across cores (`-j`)
- **📂 Pattern-based file discovery**: Sphinx-style `include_patterns` /
  `exclude_patterns` engine, verified against `sphinx.util.matching` 9.1.0 by a
  committed 881-case differential suite (zero divergence)
- **🧰 sphinx-build compatible CLI**: `sphinx-ultra SOURCEDIR OUTPUTDIR` with
  `-b html`, `-M html`/`-M clean` make-mode, `-D`/`-A` overrides, `-d`, `-n`,
  `-q`, `-E`, `-a`, `-T`, `-t`, `-c`, `-j auto`, `-W`/`--keep-going`/`-w` —
  quickstart Makefiles work unchanged
- **🔄 Incremental cache**: cache hits write their output, `--clean
  --incremental` is safe, config changes invalidate automatically (blake3
  fingerprint), and a document is re-read when a file it depends on changes;
  sphinx-build mode is incremental by default
- **🌐 Build environment & cross-references**: a serialized `BuildEnvironment`
  with the global toctree graph and relations, `numfig` section/figure
  numbering, the std domain (labels, glossary terms, `option`/`envvar`/
  `confval`), general-index data, an `objects.inv` reader/writer, and
  **intersphinx** resolution incl. the `:external:` roles — verified against a
  real `sphinx-build` 9.1.0 across a 15-project environment oracle
- **⚠️ Build validation**: toctree consistency (nonexisting/excluded entries,
  self-reference, circular toctrees, orphans, "isn't included in any
  toctree"); directive/role validation on every build; cross-reference
  resolution with Sphinx's own texts and categories — a broken reference of
  any of Sphinx's seven `warn_dangling` std reftypes (`:ref:`, `:numref:`,
  `:doc:`, `:term:`, `:keyword:`, `:option:`, `:confval:`) warns in a
  default build (`unknown document:`, `undefined label:`,
  `term not in glossary:`, …), and `-n`/nitpicky widens that to the
  remaining reference types — all through Sphinx-style warnings, `-W`, and
  `-w warnfile`
- **🔧 Config auto-detection**: conf.py (simple assignments only, for now) →
  sphinx-ultra.yaml → .yml → .json → defaults
- **📊 Statistics**: `stats` command with project analysis
- **🏗️ CLI**: `build`, `clean`, `stats`

### 🧩 Built but not yet wired into `build` (activation is roadmap M2–M4)

- **🔍 Constraint engine** inspired by sphinx-needs (library + examples; wiring
  waits for sphinx-needs item extraction in M4)
- **🖥️ Sphinx-mirroring HTML builder, minijinja template engine, search index**
  (library code, currently bypassed by the build path — the M2 wave-5 HTML
  writer revives it)
- **📇 The `objects.inv` writer**: real and byte-verified against inventories a
  real `sphinx-build` produced, but nothing writes one into your output tree
  until the HTML writer lands. (The *reader* is live — intersphinx uses it.)
  The same applies to the general index: the data is computed, the
  `genindex.html` page is not yet rendered.

### 📋 Roadmap

The canonical, milestone-by-milestone plan — real docutils-fidelity parsing, theme
engine (alabaster, sphinx-rtd-theme, furo, pydata-sphinx-theme, sphinx-book-theme),
byte-compatible search and objects.inv, 16-extension support matrix (autodoc via a
Python sidecar, myst-parser, sphinx-design, copybutton, mermaid, …), first-class
sphinx-needs, i18n, LaTeX/EPUB/man builders, live-reload dev server, and the
production-readiness workstream — is in **[ROADMAP.md](ROADMAP.md)**.

## 🚀 Quick Start

### Prerequisites

- Rust 1.85 or newer (declared MSRV, verified in CI)
- Cargo

### Installation

```bash
# Clone and build from source
git clone https://github.com/salioglu/sphinx-ultra.git
cd sphinx-ultra
cargo build --release

# The binary will be available at target/release/sphinx-ultra
```

### Basic Usage

```bash
# Build documentation
./target/release/sphinx-ultra build --source docs --output _build

# Clean build artifacts
./target/release/sphinx-ultra clean --output _build

# Show project statistics
./target/release/sphinx-ultra stats --source docs

# Get help
./target/release/sphinx-ultra --help
```

### Available Commands

- `build`: Build documentation from source files
- `clean`: Remove build artifacts and output files  
- `stats`: Display project statistics and analysis

### sphinx-build Compatible Mode

Invocations that don't start with a subcommand are parsed exactly like
`sphinx-build`, so existing Makefiles and CI scripts work by swapping the
binary name:

```bash
# Classic sphinx-build style
sphinx-ultra docs _build/html -b html

# Make-mode (what sphinx-quickstart Makefiles invoke): output goes to _build/html
sphinx-ultra -M html docs _build

# Overrides, nitpicky mode, fresh environment, quiet
sphinx-ultra docs _build -D project=MyDocs -n -E -q -j auto
```

Supported: positional `SOURCEDIR OUTPUTDIR`, `-b html` (other builders exit 2
until their milestones land), `-M html`/`-M clean`, `-D key=value`,
`-A name=value`, `-d doctreedir`, `-n`, `-q`, `-E`, `-a`, `-T`, `-t tag`,
`-c confdir`, `-j N|auto`, `-W`, `--keep-going`, `-w file`, repeatable `-v`.
sphinx-build mode is incremental by default (`-E` discards the saved
environment, `-a` rewrites everything), and a pre-set `RUST_LOG` always wins
over the verbosity flags. One caveat: a source directory literally named
`build`, `clean`, or `stats` must be written with a path prefix
(`sphinx-ultra ./build _build`).

### Build Options

```bash
# Parallel processing
sphinx-ultra build --jobs 8 --source docs --output _build

# Incremental builds
sphinx-ultra build --incremental --source docs --output _build

# Clean before build
sphinx-ultra build --clean --source docs --output _build

# Save warnings to file
sphinx-ultra build --warning-file warnings.log --source docs --output _build

# Fail on warnings (useful for CI)
sphinx-ultra build --fail-on-warning --source docs --output _build
```

## 🔧 Configuration

Sphinx Ultra supports multiple configuration formats and can auto-detect your setup:

### Configuration Priority

1. **conf.py** (Sphinx standard) - Automatically detected and parsed
2. **sphinx-ultra.yaml** - Native YAML configuration  
3. **sphinx-ultra.yml** - Alternative YAML format
4. **sphinx-ultra.json** - JSON configuration
5. **Default settings** - Used if no config file found

### Sphinx conf.py Support

Sphinx Ultra can read existing Sphinx `conf.py` files, **currently limited to
simple single-line assignments** (strings, booleans, integers, single-line lists):

```python
# conf.py — this subset parses today
project = 'My Documentation'
version = '1.0'
extensions = ['sphinx.ext.autodoc', 'sphinx.ext.viewcode']
html_theme = 'sphinx_rtd_theme'
```

> Multi-line lists, dicts, tuples, string concatenation, and triple-quoted
> strings parse natively; every construct the parser cannot handle (computed
> values, f-strings) produces a `conf.py:<line>` warning instead of being
> silently dropped. Executing dynamic conf.py in your project's venv arrives
> with the M5 sidecar.

### YAML Configuration

Create a `sphinx-ultra.yaml` file for native configuration.

> Every field is optional — a partial YAML file loads with sensible defaults
> for whatever it omits.

```yaml
# Project information
project: "My Documentation"
version: "1.0.0"
copyright: "2024, My Company"

# Build settings
parallel_jobs: 8
max_cache_size_mb: 500
cache_expiration_hours: 24

# Output configuration
output:
  html_theme: "sphinx_rtd_theme"
  syntax_highlighting: true
  highlight_theme: "github"
  search_index: true
  minify_html: false

# File pattern matching (Sphinx-style)
include_patterns:
  - "**/*.rst"
  - "**/*.md"
exclude_patterns:
  - "_build/**"
  - "drafts/**"

# Extensions (limited support currently)
extensions:
  - "sphinx.ext.autodoc"
  - "sphinx.ext.viewcode"
  - "sphinx.ext.intersphinx"

# Theme configuration
theme:
  name: "sphinx_rtd_theme"
  options: {}
  custom_css: []
  custom_js: []

# Optimization settings
optimization:
  parallel_processing: true
  incremental_builds: true
  document_caching: true
```

### Configuration Fields

Many standard Sphinx configuration options are **parsed**; note that a number of
them are not yet consumed by the build (see
[docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md) § Configuration).
The options that demonstrably change behavior today are project metadata,
`parallel_jobs`, `include_patterns`/`exclude_patterns`, and `fail_on_warning`.
Parsed categories include:
- Project metadata (project, version, copyright, author)
- HTML output options (theme, static paths, CSS/JS files)  
- Extension configuration
- Template and static file paths
- **File pattern matching** (`include_patterns`, `exclude_patterns`) - [compatibility guide](docs/SPHINX_PATTERNS_COMPATIBILITY.md) (close to Sphinx; remaining verified divergences tracked in [ROADMAP.md](ROADMAP.md) M1)
- Build optimization settings

## 📈 Performance

> **Important caveat:** the numbers below were measured on the **current
> placeholder pipeline** (which does not yet perform full RST rendering, theming,
> or search indexing). They demonstrate the parallel-I/O architecture, not
> end-to-end documentation-build performance. Honest, corpus-based benchmarks with
> regression gates arrive with the real parser (ROADMAP M2, §10).

| Files | Build Time | Processing Rate | Memory Usage |
|-------|------------|-----------------|--------------|
| 2 files | 8ms | 250 files/sec | ~10MB |  
| 51 files | 44ms | 1,159 files/sec | ~15MB |
| 100+ files | ~85ms* | 1,176 files/sec* | ~20MB* |

*Projected based on linear scaling

### Performance Features

- **Parallel Processing**: Utilizes all CPU cores for maximum throughput
- **Change Detection**: blake3-based staleness checks; cache hits write their
  output, and any configuration change invalidates the cache automatically
- **Memory Efficient**: Low memory footprint even for large projects
- **Minimal I/O**: Efficient file operations and batch processing

### Comparison Notes

While we don't have direct Sphinx comparison benchmarks yet, the processing speeds above represent significant performance improvements for documentation builds. The actual performance gain depends on:

- Number of files and their complexity
- Available CPU cores  
- Disk I/O speed
- Whether incremental builds are enabled

## 🏗️ Architecture

The Rust builder consists of several key components:

- **Parser**: RST/Markdown parsing (prototype today; docutils-fidelity parser is ROADMAP M2)
- **Cache**: Incremental build cache with blake3 change detection
- **Renderer**: minijinja-based template engine (built, not yet wired — ROADMAP M2)
- **Builder**: Parallel processing engine (rayon)

## 🔍 Advanced Usage

### Incremental Builds

Enable faster rebuilds by only processing changed files:

```bash
sphinx-ultra build --incremental --source docs --output _build
```

Cache hits always write their output (a cached rebuild produces a complete
output tree), `--clean --incremental` is safe, and any configuration change
invalidates the cache automatically. `max_cache_size_mb` and
`cache_expiration_hours` in the config control retention.

### Parallel Processing

Control the number of parallel jobs:

```bash
# Use 16 parallel jobs for maximum performance on large projects
sphinx-ultra build --jobs 16 --source docs --output _build

# Use 1 job for debugging or memory-constrained environments  
sphinx-ultra build --jobs 1 --source docs --output _build
```

### Warning and Error Handling

```bash
# Save all warnings and errors to a log file
sphinx-ultra build --warning-file build.log --source docs --output _build

# Treat warnings as errors (useful for CI/CD)
sphinx-ultra build --fail-on-warning --source docs --output _build

# Combine both for strict CI builds
sphinx-ultra build -w build.log -W --source docs --output _build
```

### Configuration File Usage

```bash
# Use a specific configuration file
sphinx-ultra build --config my-config.yaml --source docs --output _build

# Configuration auto-detection order:
# 1. conf.py (if present)
# 2. sphinx-ultra.yaml  
# 3. sphinx-ultra.yml
# 4. sphinx-ultra.json
# 5. Default configuration
```

### Clean Builds

```bash
# Clean output directory before building
sphinx-ultra build --clean --source docs --output _build

# Or clean manually
sphinx-ultra clean --output _build
```

### Project Analysis

```bash
# Get detailed project statistics
sphinx-ultra stats --source docs
```

Output includes:
- Number of source files discovered
- Total lines of documentation
- Average and largest file sizes  
- Directory depth analysis
- Cross-reference count

## 🐛 Debugging and Troubleshooting

### Enable Verbose Logging

```bash
# Debug-level logging for detailed build information
# (note: --verbose goes before the subcommand)
sphinx-ultra --verbose build --source docs --output _build
```

### Common Issues

**Configuration Loading Errors**
- Ensure YAML/JSON syntax is valid
- Check that required fields are present
- Use `--config` to specify config file explicitly

**Build Failures**
- Check file permissions in source and output directories
- Verify source files are valid RST/Markdown
- Review warning output for specific issues

**Performance Issues**
- Reduce parallel jobs if memory-constrained: `--jobs 1`
- Check for large files that may slow processing

### Getting Help

- Use `sphinx-ultra --help` for command overview
- Use `sphinx-ultra build --help` for build options
- Check project issues on GitHub
- Enable verbose logging for debugging

## 🤝 Contributing

**We welcome contributors!** This project is in active development and needs help with:

- 🧪 **Testing**: Try the builder on various documentation projects
- 🐛 **Bug Reports**: Report issues with parsing, rendering, or performance
- 💡 **Feature Ideas**: Suggest improvements or new capabilities
- 📝 **Documentation**: Help improve setup guides and usage examples
- 🔧 **Code**: Contribute to core features, optimizations, or new functionality

### Development Setup

```bash
# Clone and build
git clone https://github.com/salioglu/sphinx-ultra.git
cd sphinx-ultra
./dev.sh setup

# Install git pre-commit hooks (recommended)
./dev.sh install-hooks

# Run development commands
./dev.sh fmt           # Format code
./dev.sh clippy        # Run linter
./dev.sh test          # Run tests
./dev.sh pre-commit    # Run all pre-commit checks
./dev.sh check         # Run all checks including tests

# Build documentation
./dev.sh docs
```

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for detailed guidelines.

**Priority Areas**: We need help with:

- 🧪 **Testing**: Try the builder on various documentation projects and report results
- 🐛 **Bug Reports**: Report issues with parsing, rendering, or performance  
- 💡 **Feature Validation**: Test existing features and suggest improvements
- 📝 **Documentation**: Help improve setup guides and usage examples
- 🔧 **Core Features**: Contribute to parsing, theming, or search functionality
- 🎨 **Themes**: Develop modern, responsive documentation themes
- 🔌 **Extensions**: Expand Sphinx extension compatibility

### What Currently Works Well

- Fast parallel file processing (rayon)
- Configuration auto-detection (conf.py subset → YAML → JSON → defaults)
- Pattern-based file discovery with Sphinx-parity `[!…]`/pruning semantics
- Toctree missing-reference/orphan warnings with `-W`/`-w`

### What Needs Development

- Advanced theming and templating
- Search index functionality  
- Live development server
- Full Sphinx directive compatibility

## 📦 Releases

This project uses an automated release system with version validation to ensure consistency.

### For Users

Download pre-built binaries from the [Releases page](https://github.com/salioglu/sphinx-ultra/releases).

### For Maintainers

```bash
# Setup release environment (one-time)
./scripts/setup.sh

# Create a new patch release (0.1.2 → 0.1.3)
./scripts/release.sh --patch

# Create a new minor release (0.1.2 → 0.2.0)
./scripts/release.sh --minor

# Create a new major release (0.1.2 → 1.0.0)
./scripts/release.sh --major

# Preview what a release would do
./scripts/release.sh --dry-run --patch
```

The release script automatically:

- ✅ Runs tests to ensure quality
- ✅ Updates `Cargo.toml` version  
- ✅ Creates and pushes git tags
- ✅ Triggers GitHub Actions to build and publish

**Version Safety**: The system prevents version mismatches between git tags and `Cargo.toml`. See [`scripts/README.md`](scripts/README.md) for detailed documentation.

## 📄 License

This project is licensed under the MIT License - see the LICENSE file for details.
