# Basic Sphinx Ultra Example

This example demonstrates the minimal setup required for a Sphinx Ultra project.

## Files Included

- `sphinx-ultra.yaml` - Main configuration file
- `index.rst` - Documentation entry point
- `getting-started.rst` - Getting started guide
- `configuration.rst` - Configuration options
- `api.rst` - API reference
- `examples.rst` - Usage examples

## Building

```bash
# From this directory
sphinx-ultra build --source . --output _build

# Or with live reload
sphinx-ultra serve --source . --port 8000
```

## Features Demonstrated

- Basic RST syntax
- Code highlighting
- Table of contents
- Cross-references
- Theme configuration

## Performance

This basic example should build in under 1 second and demonstrate the speed advantages of Sphinx Ultra over traditional Sphinx.
