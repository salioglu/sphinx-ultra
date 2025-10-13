# Sphinx Ultra Rust Builder

[![Crates.io](https://img.shields.io/crates/v/sphinx-ultra.svg)](https://crates.io/crates/sphinx-ultra)
[![CI](https://github.com/salioglu/sphinx-ultra/actions/workflows/ci.yml/badge.svg)](https://github.com/salioglu/sphinx-ultra/actions/workflows/ci.yml)
[![Documentation](https://github.com/salioglu/sphinx-ultra/actions/workflows/docs.yml/badge.svg)](https://salioglu.github.io/sphinx-ultra)
[![Release](https://github.com/salioglu/sphinx-ultra/actions/workflows/release.yml/badge.svg)](https://github.com/salioglu/sphinx-ultra/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org)
[![Sponsor](https://img.shields.io/badge/sponsor-GitHub-pink.svg)](https://github.com/sponsors/salioglu)

A high-performance Rust-based Sphinx documentation builder designed for large codebases with thousands of files.

## ⚠️ Development Status

**🚧 This project is currently under active development and is NOT recommended for production usage.**

**Current Focus**: The primary goal is **validation and experimentation** rather than producing perfectly matched Sphinx builds. We are:

- ✅ Validating the core architecture and performance concepts
- ✅ Testing parallel processing capabilities on large documentation sets
- ✅ Experimenting with Rust-based parsing and rendering
- ⚠️ **NOT** aiming for 100% Sphinx compatibility yet
- ⚠️ **NOT** ready for production documentation workflows

**Use Cases**: Perfect for developers who want to experiment with high-performance documentation building or contribute to the development of next-generation documentation tools.

## ✨ Features

### ✅ Currently Implemented

- **🚀 Blazing Fast**: Parallel processing with Rust's performance
- **📊 Scalable**: Handle large documentation projects efficiently (tested with 50+ files in ~44ms)
- **🔄 Incremental Builds**: Smart caching system for faster rebuilds
- **📁 File Processing**: Support for RST and Markdown files
- **🔧 Configuration**: Multiple configuration formats (conf.py, YAML, JSON)
- **📂 File Pattern Matching**: 100% Sphinx-compatible `include_patterns` and `exclude_patterns` support
- **📊 Statistics**: Project analysis and build metrics
- **⚠️ Validation**: Document validation with warning/error reporting
- **🏗️ CLI Interface**: Complete command-line interface (build, clean, stats)
- **📦 Static Assets**: Automatic copying of static files and assets
- **🎯 Domain System**: Complete cross-reference validation with Python and RST domains
- **🔗 Reference Validation**: Comprehensive validation of :func:, :class:, :doc:, :ref: references
- **💡 Smart Suggestions**: Intelligent suggestions for broken references

### 🚧 Partially Implemented

- **🔍 Search Index**: Framework exists but search functionality not active
- **🛠️ Extensions**: Basic extension system with limited Sphinx extension support
- **🎨 Themes**: Basic theme structure but no advanced theming

### 📋 Planned Features

For detailed development roadmap, see **[Validation Features Plan](VALIDATION_FEATURES_PLAN.md)** which outlines our validation-focused approach.

**Phase 1 (Next 2 months)**:
- **🏗️ Domain System**: Sphinx-compatible domain registration and cross-reference validation
- **📝 Directive Validation**: Complete directive/role validation system
- **📖 Document Structure**: TOC tree and hierarchy validation

**Phase 2-4 (Months 3-8)**:
- **🔍 Content Constraints**: Field validation and workflow checking
- **� Extension Framework**: Plugin validation and compatibility
- **📚 Code Documentation**: Autodoc-style validation
- **🌍 Internationalization**: Translation completeness validation

**Advanced UI Features** (search indexing, complex templating, etc.) are intentionally deferred until validation foundation is solid.
- **🌐 Live Server**: Development server with live reload
- **�️ File Watching**: Automatic rebuilds on file changes
- **🔌 Plugin System**: Full plugin architecture for custom functionality
- **📱 Mobile Friendly**: Responsive design optimization
- **🖼️ Image Optimization**: Automatic image processing and optimization
- **📦 Asset Bundling**: Advanced asset optimization and bundling

> **Note**: This project is in active development. The core build functionality works reliably, but advanced features are still being developed.

## 🚀 Quick Start

### Prerequisites

- Rust 1.70+
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

### Build Options

```bash
# Parallel processing
sphinx-ultra build --jobs 8 --source docs --output _build

# Incremental builds (faster rebuilds)
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

Sphinx Ultra can read and parse existing Sphinx `conf.py` files:

```python
# conf.py (existing Sphinx configuration works)
project = 'My Documentation'
version = '1.0'
extensions = ['sphinx.ext.autodoc', 'sphinx.ext.viewcode']
html_theme = 'sphinx_rtd_theme'
```

### YAML Configuration

Create a `sphinx-ultra.yaml` file for native configuration:

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

# File pattern matching (Sphinx-compatible)
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

Most standard Sphinx configuration options are supported including:
- Project metadata (project, version, copyright, author)
- HTML output options (theme, static paths, CSS/JS files)  
- Extension configuration
- Template and static file paths
- **File pattern matching** (`include_patterns`, `exclude_patterns`) - [Full compatibility guide](docs/SPHINX_PATTERNS_COMPATIBILITY.md)
- Build optimization settings

## 📈 Performance Benchmarks

Real performance test results on documentation projects:

| Files | Build Time | Processing Rate | Memory Usage |
|-------|------------|-----------------|--------------|
| 2 files | 8ms | 250 files/sec | ~10MB |  
| 51 files | 44ms | 1,159 files/sec | ~15MB |
| 100+ files | ~85ms* | 1,176 files/sec* | ~20MB* |

*Projected based on linear scaling

### Performance Features

- **Parallel Processing**: Utilizes all CPU cores for maximum throughput
- **Smart Caching**: Incremental builds only process changed files
- **Memory Efficient**: Low memory footprint even for large projects
- **Fast Parsing**: Optimized RST and Markdown parsing
- **Minimal I/O**: Efficient file operations and batch processing

### Comparison Notes

While we don't have direct Sphinx comparison benchmarks yet, the processing speeds above represent significant performance improvements for documentation builds. The actual performance gain depends on:

- Number of files and their complexity
- Available CPU cores  
- Disk I/O speed
- Whether incremental builds are enabled

## 🏗️ Architecture

The Rust builder consists of several key components:

- **Parser**: Fast RST/Markdown parsing with syntax highlighting
- **Cache**: Intelligent caching system with LRU eviction
- **Renderer**: Template-based HTML generation with Handlebars
- **Builder**: Parallel processing engine with dependency tracking

## 🔍 Advanced Usage

### Incremental Builds

Enable faster rebuilds by only processing changed files:

```bash
sphinx-ultra build --incremental --source docs --output _build
```

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
sphinx-ultra --verbose build --source docs --output _build

# Or set environment variable
RUST_LOG=debug sphinx-ultra build --source docs --output _build
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
- Enable incremental builds: `--incremental`
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

- Basic RST and Markdown processing
- Fast parallel builds  
- Configuration auto-detection
- File validation and warning systems
- Incremental caching

### What Needs Development

- Advanced theming and templating
- Search index functionality  
- Live development server
- Full Sphinx directive compatibility

## � Releases

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

## �📄 License

This project is licensed under the MIT License - see the LICENSE file for details.
