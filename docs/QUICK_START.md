# Quick Start Guide

This guide will help you get started with Sphinx Ultra, a high-performance Rust-based documentation builder.

## Current Status

**Sphinx Ultra is currently in active development.** The core build functionality is working and tested, but advanced features like live server and full theming are still being developed.

### What Works Now ✅
- Fast parallel documentation builds
- RST and Markdown file processing  
- Incremental builds with caching
- Configuration auto-detection (conf.py, YAML, JSON)
- Document validation and warnings
- CLI interface (build, clean, stats commands)

### In Development 🚧
- Live development server
- Advanced theming system
- Search functionality
- Full Sphinx directive compatibility

## Installation

### Build from Source (Recommended)

```bash
git clone https://github.com/salioglu/sphinx-ultra.git
cd sphinx-ultra
cargo build --release

# Binary will be at target/release/sphinx-ultra
```

### Alternative: Download Pre-built Binary

1. Go to the [releases page](https://github.com/salioglu/sphinx-ultra/releases)
2. Download the appropriate binary for your platform
3. Extract and place in your PATH

## Basic Usage

### 1. Build Existing Documentation

Sphinx Ultra can build existing Sphinx projects:

```bash
# If you have a conf.py file (Sphinx project)
sphinx-ultra build --source . --output _build

# For any RST/Markdown files
sphinx-ultra build --source docs --output _build
```

### 2. Check Project Statistics

```bash
sphinx-ultra stats --source .
```

### 3. Clean Build Artifacts

```bash
sphinx-ultra clean --output _build
```

### 4. Advanced Build Options

```bash
# Fast incremental builds
sphinx-ultra build --incremental --source . --output _build

# Parallel processing control
sphinx-ultra build --jobs 8 --source . --output _build

# Save warnings to file  
sphinx-ultra build --warning-file warnings.log --source . --output _build

# Treat warnings as errors (for CI)
sphinx-ultra build --fail-on-warning --source . --output _build
```

## Configuration

Sphinx Ultra supports multiple configuration formats and auto-detects them:

### Automatic Configuration Detection

Sphinx Ultra will automatically find and use configuration in this order:
1. `conf.py` (existing Sphinx configuration)
2. `sphinx-ultra.yaml` 
3. `sphinx-ultra.yml`
4. `sphinx-ultra.json`
5. Default settings (if no config found)

### Using Existing Sphinx Projects

If you have a `conf.py` file, Sphinx Ultra can read it directly:

```python
# conf.py - your existing Sphinx configuration works!
project = 'My Documentation'
version = '1.0.0'
extensions = ['sphinx.ext.autodoc', 'sphinx.ext.viewcode']
html_theme = 'sphinx_rtd_theme'
```

### Creating New Configuration

Create a `sphinx-ultra.yaml` file for new projects:

```yaml
# Project information  
project: "My Documentation"
version: "1.0.0"
copyright: "2024, My Company"

# Build settings
parallel_jobs: 8
max_cache_size_mb: 500

# Extensions (basic support)
extensions:
  - "sphinx.ext.autodoc"
  - "sphinx.ext.viewcode"

# Output settings
output:
  html_theme: "sphinx_rtd_theme"
  syntax_highlighting: true
  minify_html: false

# Optimization
optimization:
  parallel_processing: true
  incremental_builds: true
  document_caching: true
```

## Performance

Sphinx Ultra is designed for speed. Here are real performance results:

| Project Size | Build Time | Processing Rate |
|--------------|------------|-----------------|
| 2 files | 8ms | 250 files/sec |
| 51 files | 44ms | 1,159 files/sec |
| 100+ files* | ~85ms | 1,176 files/sec |

*Projected based on testing

### Performance Features
- **Parallel Processing**: Uses all CPU cores
- **Smart Caching**: Only rebuilds changed files
- **Memory Efficient**: Low memory usage even for large projects
- **Fast I/O**: Optimized file operations

## Limitations and Roadmap

### Current Limitations
- **Basic HTML Output**: Simple HTML generation (no advanced theming)
- **Limited Directives**: Basic RST/Markdown support only
- **No Live Server**: Manual rebuilds required
- **Basic Search**: Search index framework exists but not functional

### Coming Soon
- Live development server with auto-reload
- Advanced theming system  
- Full-text search functionality
- Extended Sphinx directive support
- Modern responsive themes

## Getting Help

- **Documentation**: See [Implementation Status](IMPLEMENTATION_STATUS.md) for detailed feature status
- **Issues**: Report bugs on [GitHub Issues](https://github.com/salioglu/sphinx-ultra/issues)
- **Discussions**: Join conversations on [GitHub Discussions](https://github.com/salioglu/sphinx-ultra/discussions)
- **CLI Help**: Use `sphinx-ultra --help` for command reference

## Next Steps

1. **Try it out**: Build your existing documentation with Sphinx Ultra
2. **Report feedback**: Let us know what works and what doesn't
3. **Contribute**: Help improve the project ([Contributing Guide](../CONTRIBUTING.md))
4. **Follow progress**: Watch the repository for updates

Remember: Sphinx Ultra is in active development. The core functionality works well, but expect rapid improvements in advanced features!
