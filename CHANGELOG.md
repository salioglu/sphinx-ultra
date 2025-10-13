# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- High-performance parallel documentation building ✅
- Smart caching system with LRU eviction ✅  
- Incremental builds for faster rebuilds ✅
- Support for RST and Markdown parsing ✅
- Template-based HTML generation framework (basic implementation) ⚠️
- File system monitoring for automatic rebuilds (planned)
- Configurable themes and extensions (framework only) ⚠️
- Comprehensive CLI interface ✅
- Performance benchmarking tools ✅
- Warning file output support (`--warning-file` / `-w` option) ✅
- Document validation with orphan and reference checking ✅
- **Domain System & Cross-Reference Validation** ✅
  - Pluggable domain architecture with trait-based validators
  - Python domain for :func:, :class:, :mod:, :meth:, :attr:, :data:, :exc: references
  - RST domain for :doc:, :ref:, :numref: references
  - Comprehensive reference parser with regex-based extraction
  - External reference detection (URLs, email addresses)
  - Intelligent suggestion system for broken references using fuzzy matching
  - Validation statistics and detailed reporting
  - 21 comprehensive domain-specific tests
- **Constraint Validation System** ✅
  - Content item validation framework inspired by sphinx-needs
  - Expression evaluator for constraint logic (supports ==, !=, and, or, in)
  - Severity-based failure actions (info, warning, error, critical)
  - Template-based error messages with variable substitution
  - Automatic style application based on constraint failures
  - Comprehensive validation configuration system
- Multi-format configuration support (conf.py, YAML, JSON) ✅
- Project statistics and analysis tools ✅

### Implementation Status Legend
- ✅ **Fully Implemented**: Feature is working and tested
- ⚠️ **Partially Implemented**: Basic framework exists, needs development  
- ❌ **Planned**: Not yet implemented

### Currently Working Features
- Fast parallel builds (1000+ files/second processing rate)
- RST and Markdown file processing
- Incremental caching with change detection
- Comprehensive CLI with build, clean, and stats commands
- Configuration auto-detection (conf.py, YAML, JSON)
- Document validation and warning reporting
- Static asset copying and management

### In Development
- Full-text search index generation ⚠️
- Live reload development server ❌
- Advanced theming system ⚠️ 
- Sphinx extension compatibility ⚠️
- Modern responsive themes ❌

### Changed
- Updated documentation to clearly separate implemented vs planned features
- Improved performance benchmarking and reporting
- Enhanced error messages and validation feedback

### Performance Metrics
- 2 files: ~8ms build time
- 51 files: ~44ms build time  
- Processing rate: ~1,159 files/second
- Memory usage: 10-20MB for typical projects

## [0.1.0] - 2024-09-07

### Added
- Initial project setup
- Core architecture implementation
- Basic documentation and configuration
