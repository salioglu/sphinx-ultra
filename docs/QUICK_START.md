# Quick Start Guide

This guide will help you get started with Sphinx Ultra quickly.

## Installation

### Option 1: Using the Install Script (Recommended)

```bash
curl -sSL https://raw.githubusercontent.com/salioglu/sphinx-ultra/main/install.sh | bash
```

### Option 2: Download Pre-built Binary

1. Go to the [releases page](https://github.com/salioglu/sphinx-ultra/releases)
2. Download the appropriate binary for your platform
3. Extract and place in your PATH

### Option 3: Build from Source

```bash
git clone https://github.com/salioglu/sphinx-ultra.git
cd sphinx-ultra
cargo build --release
```

## Basic Usage

### 1. Initialize a New Project

```bash
sphinx-ultra init my-docs
cd my-docs
```

### 2. Build Documentation

```bash
sphinx-ultra build --source . --output _build
```

### 3. Save Warnings to File

```bash
# Save all warnings and errors to a log file
sphinx-ultra build --source . --output _build --warning-file warnings.log

# Short form
sphinx-ultra build -w warnings.log --source . --output _build
```

### 4. Start Development Server

```bash
sphinx-ultra serve --source . --port 8000
```

### 5. Watch for Changes

```bash
sphinx-ultra watch --source . --output _build
```

## Configuration

Create a `sphinx-ultra.yaml` file in your project root:

```yaml
# Basic configuration
source_dir: "."
output_dir: "_build"
parallel_jobs: 4

# Theme settings
theme:
  name: "sphinx_rtd_theme"

# Extensions
extensions:
  - "sphinx.ext.autodoc"
  - "sphinx.ext.viewcode"

# Build settings
build:
  incremental: true
  cache_enabled: true
  minify_html: false
```

## Project Structure

```text
my-docs/
├── sphinx-ultra.yaml    # Configuration file
├── index.rst           # Main documentation file
├── _static/            # Static assets
├── _templates/         # Custom templates
└── _build/             # Generated output
```

## Writing Documentation

### RST Example

```rst
Welcome to My Project
=====================

This is the main documentation page.

Features
--------

* Fast builds
* Live reload
* Modern themes

Code Example
------------

.. code-block:: python

   def hello_world():
       print("Hello, World!")
```

### Markdown Example (if enabled)

```markdown
# Welcome to My Project

This is the main documentation page.

## Features

- Fast builds
- Live reload
- Modern themes

## Code Example

```python
def hello_world():
    print("Hello, World!")
```

## Next Steps

- Read the [Configuration Guide](configuration.md)
- Explore [Advanced Features](advanced.md)
- Check out [Examples](examples/)
- Join our [Community](https://github.com/salioglu/sphinx-ultra/discussions)
