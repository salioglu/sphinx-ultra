/// Domain System Integration Example
///
/// This example demonstrates the full domain system functionality:
/// - Registering domains (Python and RST)
/// - Parsing content for cross-references
/// - Registering domain objects
/// - Validating references and detecting broken links
use sphinx_ultra::domains::{
    parser::ReferenceParser, python::PythonDomain, rst::RstDomain, DomainRegistry,
    ReferenceLocation,
};
use std::collections::HashMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Sphinx Ultra - Domain System & Cross-Reference Validation Example");
    println!("==================================================================");

    // Create domain registry
    let mut registry = DomainRegistry::new();

    // Register Python and RST domains
    registry.register_domain(Box::new(PythonDomain::new()))?;
    registry.register_domain(Box::new(RstDomain::new()))?;

    println!("✅ Registered Python and RST domains");

    // Create reference parser
    let parser = ReferenceParser::new();

    // Sample RST content with various cross-references
    let sample_content = r#"
Getting Started
===============

This guide shows how to use our API.

Installation
------------

See :doc:`installation` for setup instructions.

API Reference
-------------

The main functions are:

- :func:`myproject.process_data` - Processes input data
- :class:`myproject.DataProcessor` - Main processing class
- :mod:`myproject.utils` - Utility functions

For advanced usage, see :ref:`advanced-usage`.

External References
-------------------

Python built-ins like :func:`os.path.join` are supported.
Also see :doc:`https://docs.python.org/3/` for more.

Broken References
-----------------

This references a non-existent function: :func:`myproject.missing_function`.
And a broken document: :doc:`nonexistent-doc`.
And a missing section: :ref:`missing-section`.
"#;

    println!("\n📄 Parsing sample RST content...");

    // Parse content for cross-references
    let references = parser.parse_content(
        sample_content,
        "getting-started",
        Some("getting-started.rst".to_string()),
    );

    println!("🔍 Found {} cross-references:", references.len());
    for (i, ref_obj) in references.iter().enumerate() {
        println!(
            "  {}. {:?} -> '{}' (line {}, external: {})",
            i + 1,
            ref_obj.ref_type,
            ref_obj.target,
            ref_obj.source_location.lineno.unwrap_or(0),
            ref_obj.is_external
        );

        // Add reference to registry for validation
        registry.add_cross_reference(ref_obj.clone());
    }

    // Register some domain objects to resolve references
    println!("\n🏗️  Registering domain objects...");

    // Register Python objects
    let location = ReferenceLocation {
        docname: "api".to_string(),
        lineno: Some(10),
        column: Some(0),
        source_path: Some("api.rst".to_string()),
    };

    registry.register_object(sphinx_ultra::domains::DomainObject {
        id: "py:func:myproject.process_data".to_string(),
        name: "process_data".to_string(),
        object_type: "function".to_string(),
        domain: "python".to_string(),
        definition_location: location.clone(),
        qualified_name: "myproject.process_data".to_string(),
        metadata: HashMap::new(),
        signature: Some("process_data(data: str) -> dict".to_string()),
        docstring: Some("Processes input data and returns structured result.".to_string()),
    })?;

    registry.register_object(sphinx_ultra::domains::DomainObject {
        id: "py:class:myproject.DataProcessor".to_string(),
        name: "DataProcessor".to_string(),
        object_type: "class".to_string(),
        domain: "python".to_string(),
        definition_location: location.clone(),
        qualified_name: "myproject.DataProcessor".to_string(),
        metadata: HashMap::new(),
        signature: None,
        docstring: Some("Main class for processing data.".to_string()),
    })?;

    // Register RST objects
    registry.register_object(sphinx_ultra::domains::DomainObject {
        id: "doc:installation".to_string(),
        name: "installation".to_string(),
        object_type: "document".to_string(),
        domain: "rst".to_string(),
        definition_location: location.clone(),
        qualified_name: "installation".to_string(),
        metadata: {
            let mut meta = HashMap::new();
            meta.insert("title".to_string(), "Installation Guide".to_string());
            meta
        },
        signature: None,
        docstring: None,
    })?;

    registry.register_object(sphinx_ultra::domains::DomainObject {
        id: "ref:advanced-usage".to_string(),
        name: "advanced-usage".to_string(),
        object_type: "section".to_string(),
        domain: "rst".to_string(),
        definition_location: location,
        qualified_name: "getting-started#advanced-usage".to_string(),
        metadata: {
            let mut meta = HashMap::new();
            meta.insert("title".to_string(), "Advanced Usage".to_string());
            meta.insert("docname".to_string(), "getting-started".to_string());
            meta
        },
        signature: None,
        docstring: None,
    })?;

    println!("✅ Registered objects in Python and RST domains");

    // Validate all references
    println!("\n🔍 Validating cross-references...");

    let validation_results = registry.validate_all_references();
    let mut valid_count = 0;
    let mut broken_count = 0;
    let mut external_count = 0;

    for result in &validation_results {
        if result.reference.is_external {
            external_count += 1;
            println!(
                "🌐 EXTERNAL: {:?} -> '{}' ({})",
                result.reference.ref_type,
                result.reference.target,
                result.reference.source_location.lineno.unwrap_or(0)
            );
        } else if result.is_valid {
            valid_count += 1;
            println!(
                "✅ VALID: {:?} -> '{}' (line {})",
                result.reference.ref_type,
                result.reference.target,
                result.reference.source_location.lineno.unwrap_or(0)
            );
            if let Some(obj) = &result.target_object {
                println!(
                    "    Resolved to: {} ({})",
                    obj.qualified_name, obj.object_type
                );
            }
        } else {
            broken_count += 1;
            println!(
                "❌ BROKEN: {:?} -> '{}' (line {})",
                result.reference.ref_type,
                result.reference.target,
                result.reference.source_location.lineno.unwrap_or(0)
            );
            if let Some(error) = &result.error_message {
                println!("    Error: {}", error);
            }
            if !result.suggestions.is_empty() {
                println!("    Suggestions: {}", result.suggestions.join(", "));
            }
        }
    }

    // Display validation statistics
    println!("\n📊 Validation Summary");
    println!("===================");
    let stats = registry.get_validation_stats();
    println!("Total references: {}", stats.total_references);
    println!(
        "Valid references: {} ({}%)",
        stats.valid_references,
        if stats.total_references > 0 {
            (stats.valid_references * 100) / stats.total_references
        } else {
            0
        }
    );
    println!(
        "Broken references: {} ({}%)",
        stats.broken_references,
        if stats.total_references > 0 {
            (stats.broken_references * 100) / stats.total_references
        } else {
            0
        }
    );
    println!(
        "External references: {} ({}%)",
        stats.external_references,
        if stats.total_references > 0 {
            (stats.external_references * 100) / stats.total_references
        } else {
            0
        }
    );

    println!("\nReferences by type:");
    for (ref_type, count) in &stats.references_by_type {
        println!("  {}: {}", ref_type, count);
    }

    // Show broken references with suggestions
    let broken_refs = registry.get_broken_references();
    if !broken_refs.is_empty() {
        println!("\n🚨 Broken References Detailed Report");
        println!("====================================");

        for (i, broken) in broken_refs.iter().enumerate() {
            println!(
                "{}. {} reference '{}' in {}:{}",
                i + 1,
                broken.reference.ref_type,
                broken.reference.target,
                broken.reference.source_location.docname,
                broken.reference.source_location.lineno.unwrap_or(0)
            );

            if let Some(error) = &broken.error_message {
                println!("   Error: {}", error);
            }

            if !broken.suggestions.is_empty() {
                println!("   Fix suggestions:");
                for suggestion in &broken.suggestions {
                    println!("     - Use '{}' instead", suggestion);
                }
            }
            println!();
        }
    }

    // Test object search functionality
    println!("🔍 Object Search Examples");
    println!("========================");

    let search_results = registry.search_objects("process");
    println!(
        "Search for 'process': {} objects found",
        search_results.len()
    );
    for obj in search_results {
        println!(
            "  - {} ({}) in {}",
            obj.qualified_name, obj.object_type, obj.domain
        );
    }

    let search_results = registry.search_objects("Data");
    println!(
        "\nSearch for 'Data': {} objects found",
        search_results.len()
    );
    for obj in search_results {
        println!(
            "  - {} ({}) in {}",
            obj.qualified_name, obj.object_type, obj.domain
        );
    }

    // Demonstrate reference statistics from parser
    println!("\n📈 Parser Statistics");
    println!("===================");
    let parser_stats = parser.get_reference_stats(&references);
    for (ref_type, count) in parser_stats {
        println!("{}: {}", ref_type, count);
    }

    println!("\n🎉 Domain system validation completed successfully!");
    println!(
        "Summary: {} valid, {} broken, {} external references",
        valid_count, broken_count, external_count
    );

    if broken_count > 0 {
        println!(
            "⚠️  Please fix the {} broken reference(s) before building.",
            broken_count
        );
        return Err("Broken references found".into());
    }

    Ok(())
}
