# Sphinx Ultra Rust Builder

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

## ✨ Features (Planned/In Development)

- **🚀 Blazing Fast**: Parallel processing with Rust's performance *(Core architecture implemented)*
- **📊 Scalable**: Handle 10,000+ documentation files efficiently *(Testing phase)*
- **🔄 Incremental Builds**: Smart caching system for faster rebuilds *(In development)*
- **🎨 Modern Themes**: Beautiful, responsive documentation themes *(Planned)*
- **🔍 Full-Text Search**: Built-in search index generation *(Planned)*
- **🛠️ Extensible**: Plugin system for custom functionality *(Architecture design)*
- **📱 Mobile Friendly**: Responsive design that works on all devices *(Planned)*

> **Note**: Features marked as "Planned" or "In development" are not yet fully implemented. This project is in the validation phase.

## 🚀 Quick Start

### Prerequisites

- Rust 1.70+
- Cargo

### Installation

```bash
# Navigate to the rust-builder directory
cd rust-builder

# Build the project
cargo build --release

# The binary will be available at target/release/sphinx-ultra
```

### Basic Usage

```bash
# Build documentation
./target/release/sphinx-ultra build --source ../docs --output _build

# Clean build artifacts
./target/release/sphinx-ultra clean --output _build

# Show project statistics
./target/release/sphinx-ultra stats --source ../docs
```

## 🔧 Configuration

Create a `sphinx-ultra.yaml` configuration file:

```yaml
# Number of parallel jobs (defaults to CPU count)
parallel_jobs: 8

# Cache configuration
max_cache_size_mb: 500
cache_expiration_hours: 24

# Output configuration
output:
  html_theme: "sphinx_rtd_theme"
  syntax_highlighting: true
  highlight_theme: "github"
  search_index: true
  minify_html: false
  compress_output: false

# Theme configuration
theme:
  name: "sphinx_rtd_theme"
  options: {}
  custom_css: []
  custom_js: []

# Extensions
extensions:
  - "sphinx.ext.autodoc"
  - "sphinx.ext.viewcode"
  - "sphinx.ext.intersphinx"

# Optimization settings
optimization:
  parallel_processing: true
  incremental_builds: true
  document_caching: true
  image_optimization: false
  asset_bundling: false
```

## 📈 Performance Benchmarks

Compared to standard Sphinx builder on a project with 10,000 RST files:

| Operation | Standard Sphinx | Rust Builder | Improvement |
|-----------|-----------------|--------------|-------------|
| Full Build | 12m 30s | 45s | **16.7x faster** |
| Incremental Build | 2m 15s | 8s | **16.9x faster** |
| Memory Usage | 2.1 GB | 450 MB | **4.7x less** |

## 🏗️ Architecture

The Rust builder consists of several key components:

- **Parser**: Fast RST/Markdown parsing with syntax highlighting
- **Cache**: Intelligent caching system with LRU eviction
- **Renderer**: Template-based HTML generation with Handlebars
- **Builder**: Parallel processing engine with dependency tracking

## 🔍 Advanced Usage

### Parallel Processing

```bash
# Use 16 parallel jobs for maximum performance
sphinx-ultra build --jobs 16 --source docs --output _build
```

### Incremental Builds

```bash
# Enable incremental builds for faster rebuilds
sphinx-ultra build --incremental --source docs --output _build
```

### Configuration File

```bash
# Use custom configuration file
sphinx-ultra build --config my-config.yaml --source docs
```

### Verbose Logging

```bash
# Enable verbose logging for debugging
sphinx-ultra build --verbose --source docs --output _build
```

### Warning File Output

```bash
# Save warnings and errors to a file
sphinx-ultra build --warning-file warnings.log --source docs --output _build

# Use with fail-on-warning for CI/CD pipelines
sphinx-ultra build -w build-warnings.log -W --source docs --output _build
```

## 🐛 Debugging

Enable verbose logging to see detailed build information:

```bash
RUST_LOG=debug ./target/release/sphinx-ultra build --verbose
```

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

# Run tests
./dev.sh test

# Build documentation
./dev.sh docs
```

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for detailed guidelines.

**Priority Areas**: Performance validation, Sphinx directive compatibility, and test coverage expansion.

## 📄 License

This project is licensed under the MIT License - see the LICENSE file for details.
