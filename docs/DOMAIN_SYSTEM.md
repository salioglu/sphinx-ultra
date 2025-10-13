# Domain System & Cross-Reference Validation

## Overview

The Domain System is a comprehensive validation framework for cross-references in documentation. It provides pluggable domain-specific validators that can track objects (functions, classes, documents, etc.) and validate references to them throughout your documentation.

## Architecture

### Core Components

```
Domain System
├── DomainValidator Trait      - Interface for domain implementations
├── DomainRegistry            - Central registry managing all domains
├── CrossReference            - Represents a parsed cross-reference
├── ReferenceParser           - Extracts references from RST content
└── Domain Implementations
    ├── PythonDomain          - Python object references
    └── RstDomain             - RST document references
```

### Domain Validator Trait

All domain validators implement the `DomainValidator` trait:

```rust
pub trait DomainValidator: Send + Sync {
    fn name(&self) -> &str;
    fn validate_reference(&self, reference: &CrossReference) -> ReferenceValidationResult;
    fn register_objects(&mut self, content: &str);
    fn get_suggestions(&self, reference: &CrossReference) -> Vec<String>;
}
```

## Supported Domains

### Python Domain

Validates Python object references:

- **`:func:`** - Function references
- **`:class:`** - Class references  
- **`:mod:`** - Module references
- **`:meth:`** - Method references
- **`:attr:`** - Attribute references
- **`:data:`** - Data references
- **`:exc:`** - Exception references

**Example Usage:**
```rst
See the :func:`calculate_total` function in :mod:`utils.math`.
The :class:`User` class has a :meth:`User.save` method.
```

### RST Domain

Validates RST document and section references:

- **`:doc:`** - Document references
- **`:ref:`** - Section/label references
- **`:numref:`** - Numbered references

**Example Usage:**
```rst
See :doc:`installation` for setup instructions.
Refer to :ref:`advanced-config` for details.
Use :numref:`table-results` for the data.
```

## Reference Parser

The `ReferenceParser` uses comprehensive regex patterns to extract cross-references:

```rust
// Matches patterns like :role:`target` or :role:`text <target>`
static CROSS_REF_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r":([a-zA-Z][a-zA-Z0-9_-]*):(`[^`]+`|\S+)").unwrap()
});
```

### External Reference Detection

The parser automatically identifies external references (http/https URLs, email addresses) and excludes them from validation:

```rust
pub fn is_external_reference(target: &str) -> bool {
    target.starts_with("http://") 
        || target.starts_with("https://")
        || target.contains('@')
}
```

## Usage Example

```rust
use sphinx_ultra::domains::{DomainRegistry, ReferenceParser};

// Initialize the domain system
let mut registry = DomainRegistry::new();

// Register some objects
registry.python_domain_mut().register_function("calculate_total");
registry.python_domain_mut().register_class("User");
registry.rst_domain_mut().register_document("installation");

// Parse and validate references
let parser = ReferenceParser::new();
let content = "See :func:`calculate_total` and :doc:`installation`.";
let references = parser.extract_references(content);

// Validate all references
for reference in &references {
    let result = registry.validate_reference(reference);
    match result {
        ReferenceValidationResult::Valid => println!("✓ Valid: {}", reference.target),
        ReferenceValidationResult::Broken => {
            println!("✗ Broken: {}", reference.target);
            let suggestions = registry.get_suggestions(reference);
            if !suggestions.is_empty() {
                println!("  Suggestions: {}", suggestions.join(", "));
            }
        }
        ReferenceValidationResult::External => println!("→ External: {}", reference.target),
    }
}
```

## Validation Results

The system provides three types of validation results:

- **`Valid`**: Reference successfully resolves to a registered object
- **`Broken`**: Reference cannot be resolved, with intelligent suggestions provided
- **`External`**: Reference points to external resource (URLs, emails)

## Suggestion System

When references are broken, the system provides intelligent suggestions using fuzzy string matching:

```rust
pub fn get_suggestions(&self, reference: &CrossReference) -> Vec<String> {
    let mut suggestions = Vec::new();
    let target_lower = reference.target.to_lowercase();
    
    for (name, _) in &self.functions {
        if name.to_lowercase().contains(&target_lower) || 
           target_lower.contains(&name.to_lowercase()) {
            suggestions.push(format!("{}:{}", reference.role, name));
        }
    }
    
    suggestions.sort();
    suggestions.truncate(3); // Limit to top 3 suggestions
    suggestions
}
```

## Statistics and Reporting

The domain system provides comprehensive statistics:

```rust
pub struct ValidationStatistics {
    pub total_references: usize,
    pub valid_references: usize,
    pub broken_references: usize,
    pub external_references: usize,
    pub references_by_role: HashMap<String, usize>,
}
```

## Integration with Build System

The domain system integrates seamlessly with the main build process:

1. **Object Registration**: During parsing, objects are automatically registered with appropriate domains
2. **Reference Extraction**: Cross-references are extracted from all RST content
3. **Validation**: All references are validated against registered objects
4. **Reporting**: Broken references are reported with suggestions and statistics

## Extensibility

New domains can be easily added by implementing the `DomainValidator` trait:

```rust
struct CppDomain {
    functions: HashMap<String, String>,
    classes: HashMap<String, String>,
}

impl DomainValidator for CppDomain {
    fn name(&self) -> &str { "cpp" }
    
    fn validate_reference(&self, reference: &CrossReference) -> ReferenceValidationResult {
        // Implementation for C++ object validation
    }
    
    // ... other trait methods
}
```

## Performance

- **Efficient Lookup**: HashMap-based object registry for O(1) validation
- **Regex Optimization**: Compiled regex patterns using `lazy_static`
- **Memory Efficient**: Minimal overhead for object tracking
- **Concurrent Safe**: Thread-safe design with `Send + Sync` traits

## Testing

The domain system includes comprehensive tests (21 domain-specific tests):

- Unit tests for each domain validator
- Integration tests for the complete validation pipeline
- Edge case handling (malformed references, empty content)
- Performance benchmarks for large document sets

## Future Extensions

Planned domain additions:

- **C++ Domain**: Support for C++ namespaces, classes, functions
- **JavaScript Domain**: ES6 modules, classes, functions
- **Generic Domain**: User-configurable domain for custom object types
- **Math Domain**: LaTeX math references and equation numbering

## Error Handling

The system provides detailed error reporting:

```rust
#[derive(Debug)]
pub struct BrokenReference {
    pub reference: CrossReference,
    pub suggestions: Vec<String>,
    pub context: String,
}
```

This comprehensive domain system provides the foundation for advanced documentation validation, ensuring all cross-references are accurate and helping maintain high-quality documentation.