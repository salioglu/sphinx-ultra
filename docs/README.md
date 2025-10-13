# Sphinx Ultra Documentation

Welcome to the Sphinx Ultra documentation! This site contains the complete API documentation for the Sphinx Ultra project.

## 📚 Documentation

- **[API Documentation](./api/sphinx_ultra/index.html)** - Complete Rust API documentation
- **[Quick Start Guide](./QUICK_START.md)** - Get started with Sphinx Ultra
- **[Implementation Status](./IMPLEMENTATION_STATUS.md)** - Current feature implementation status
- **[Domain System Guide](./DOMAIN_SYSTEM.md)** - Cross-reference validation system documentation
- **[GitHub Repository](https://github.com/salioglu/sphinx-ultra)** - Source code and issues

## 🚀 About Sphinx Ultra

Sphinx Ultra is a high-performance Rust-based Sphinx documentation builder designed for large codebases with thousands of files.

### Key Features

- **🚀 Blazing Fast**: Parallel processing with Rust's performance
- **📊 Scalable**: Handle 10,000+ documentation files efficiently
- **🔄 Incremental Builds**: Smart caching system for faster rebuilds
- **🎨 Modern Themes**: Beautiful, responsive documentation themes
- **🔍 Full-Text Search**: Built-in search index generation
- **� Domain System**: Complete cross-reference validation with Python and RST domains
- **💡 Smart Validation**: Intelligent suggestions for broken references with fuzzy matching

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
