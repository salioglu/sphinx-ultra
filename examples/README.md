# Sphinx Ultra Examples

This directory contains example projects demonstrating various features of Sphinx Ultra.

## Available Examples

### 1. Basic Project (`basic/`)
A minimal example showing the basic structure and configuration.

### 2. API Documentation (`api-docs/`)
Example of documenting a Python API with autodoc.

### 3. Multi-language Project (`multi-lang/`)
Documentation project with multiple languages.

### 4. Custom Theme (`custom-theme/`)
Example of creating and using a custom theme.

### 5. Plugin Example (`plugin-example/`)
Demonstration of creating and using Sphinx Ultra plugins.

## Running Examples

Each example includes its own README with specific instructions. Generally:

1. Navigate to the example directory
2. Build the documentation:

   ```bash
   sphinx-ultra build --source . --output _build
   ```

3. Serve locally:

   ```bash
   sphinx-ultra serve --source . --port 8000
   ```

## Configuration Files

Each example includes:
- `sphinx-ultra.yaml` - Main configuration
- `index.rst` or `index.md` - Entry point
- Sample content files
- Custom assets (if applicable)

## Performance Benchmarks

The `benchmarks/` directory contains projects of various sizes for performance testing:
- Small (10 files)
- Medium (100 files)
- Large (1,000 files)
- Extra Large (10,000 files)

Run benchmarks with:

```bash
cd benchmarks/large
time sphinx-ultra build --source . --output _build
```
