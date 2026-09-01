# Sphinx Ultra Documentation

Welcome to the Sphinx Ultra documentation! This site contains the complete API documentation for the Sphinx Ultra project.

## 📚 Documentation

- **[API Documentation](./api/sphinx_ultra/index.html)** - Complete Rust API documentation
- **[Quick Start Guide](./QUICK_START.md)** - Get started with Sphinx Ultra
- **[Implementation Status](./IMPLEMENTATION_STATUS.md)** - Current feature implementation status
- **[GitHub Repository](https://github.com/salioglu/sphinx-ultra)** - Source code and issues

## 🚀 About Sphinx Ultra

Sphinx Ultra is a high-performance Rust-based Sphinx documentation builder designed for large codebases with thousands of files.

### Key Features

Per-subsystem reality (what the binary actually executes, with evidence) lives in
[Implementation Status](./IMPLEMENTATION_STATUS.md); the sequencing plan is
[ROADMAP.md](../ROADMAP.md).

- **🚀 Blazing Fast**: Parallel processing with Rust's performance
- **📊 Scalable**: Handle 10,000+ documentation files efficiently
- **🔄 Incremental Builds**: Dependency-driven caching, invalidated by the files
  and config a document actually depends on
- **📐 Docutils-fidelity RST**: block + inline grammar and the Sphinx directive
  set, differentially verified against docutils 0.22.4 and sphinx-build 9.1.0
- **🔗 Cross-references**: `BuildEnvironment` with the std domain, toctree graph,
  numfig numbering, genindex data, `objects.inv`, and intersphinx resolution
- **🎨 Modern Themes** *(planned, ROADMAP M3)*: theme engine and HTML writer
- **🔍 Full-Text Search** *(planned, ROADMAP M3)*: Sphinx-format search index

## �️ Development

To build documentation locally:

```bash
# For GitHub Pages (creates docs/api/ - gitignored)
./dev.sh docs

# For development (opens in browser)
./dev.sh docs-dev
```

**Note**: The `api/` folder contains generated Rust documentation and is gitignored to keep the repository clean.

## �📞 Contact

- **Author**: Sinan Alioglu
- **Email**: [sinan@alioglu.org](mailto:sinan@alioglu.org)
- **GitHub**: [@salioglu](https://github.com/salioglu)

---

*This documentation is automatically generated and deployed using GitHub Actions.*
