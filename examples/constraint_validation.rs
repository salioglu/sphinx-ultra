//! Example demonstrating the constraint validation system
//!
//! This example shows how to use the sphinx-ultra constraint validation system
//! to validate documentation content items similar to sphinx-needs.

use std::collections::HashMap;

use sphinx_ultra::validation::{
    ConstraintDefinition, ContentItem, FieldValue, ItemLocation, ValidationConfig,
    ValidationContext, ValidationSettings, ValidationSeverity,
};
use sphinx_ultra::ConstraintEngine;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    env_logger::init();

    println!("Sphinx Ultra - Constraint Validation Example");
    println!("============================================");

    // Create a sample content item (similar to a sphinx-needs requirement)
    let mut content_item = create_sample_requirement();
    println!("\nCreated content item: {}", content_item.id);
    println!("Title: {}", content_item.title);
    println!("Status: {:?}", content_item.metadata.get("status"));
    println!("Priority: {:?}", content_item.metadata.get("priority"));

    // Create validation configuration with constraints
    let config = create_validation_config();
    println!(
        "\nCreated validation config with {} constraints",
        config.constraints.len()
    );

    // Create a collection of all items (in a real scenario, this would come from parsed documents)
    let mut all_items = HashMap::new();
    all_items.insert(content_item.id.clone(), content_item.clone());

    // Create constraint engine
    let mut engine = ConstraintEngine::new();
    println!("\nInitialized constraint engine");

    // Process constraints for the content item
    println!("\nProcessing constraints...");
    {
        // Create validation context in a separate scope
        let context = ValidationContext {
            current_item: &content_item,
            all_items: &all_items,
            config: &config,
            variables: HashMap::new(),
        };

        match engine.process_constraints(&content_item, &context) {
            Ok((modified_item, failures)) => {
                // Update the original item with any style changes
                content_item = modified_item;

                if failures.is_empty() {
                    println!("✅ All constraints passed!");
                } else {
                    println!("❌ Found {} constraint failures:", failures.len());
                    for failure in &failures {
                        println!("  - Rule: {}", failure.rule.name);
                        println!("    Severity: {}", failure.severity);
                        if let Some(msg) = &failure.result.error_message {
                            println!("    Message: {}", msg);
                        }
                    }
                }
            }
            Err(e) => {
                println!("❌ Error processing constraints: {}", e);
            }
        }
    }

    // Show final item state (styles may have been applied)
    println!("\nFinal item state:");
    println!("  Style: {:?}", content_item.style);

    // Now let's test with a compliant item
    println!("\n{}", "=".repeat(50));
    println!("Testing with compliant requirement...");

    let compliant_item = create_compliant_requirement();
    {
        let context2 = ValidationContext {
            current_item: &compliant_item,
            all_items: &all_items,
            config: &config,
            variables: HashMap::new(),
        };

        match engine.process_constraints(&compliant_item, &context2) {
            Ok((_modified_compliant, failures)) => {
                if failures.is_empty() {
                    println!("✅ All constraints passed for compliant item!");
                } else {
                    println!(
                        "❌ Unexpected failures for compliant item: {}",
                        failures.len()
                    );
                }
            }
            Err(e) => {
                println!("❌ Error: {}", e);
            }
        }
    }

    println!("\nExample completed successfully!");
    Ok(())
}

fn create_sample_requirement() -> ContentItem {
    let mut metadata = HashMap::new();
    metadata.insert("status".to_string(), FieldValue::String("open".to_string()));
    metadata.insert(
        "priority".to_string(),
        FieldValue::String("high".to_string()),
    );
    metadata.insert(
        "type".to_string(),
        FieldValue::String("requirement".to_string()),
    );
    metadata.insert(
        "tags".to_string(),
        FieldValue::Array(vec![
            FieldValue::String("security".to_string()),
            FieldValue::String("performance".to_string()),
        ]),
    );

    ContentItem {
        id: "REQ-001".to_string(),
        title: "User Authentication Security".to_string(),
        content: "The system shall implement secure user authentication with multi-factor support."
            .to_string(),
        metadata,
        constraints: vec!["status_complete".to_string(), "priority_valid".to_string()],
        relationships: HashMap::new(),
        location: ItemLocation {
            docname: "requirements/security.rst".to_string(),
            lineno: Some(15),
            source_path: Some("docs/requirements/security.rst".to_string()),
        },
        style: None,
    }
}

fn create_compliant_requirement() -> ContentItem {
    let mut metadata = HashMap::new();
    metadata.insert(
        "status".to_string(),
        FieldValue::String("complete".to_string()),
    );
    metadata.insert(
        "priority".to_string(),
        FieldValue::String("high".to_string()),
    );
    metadata.insert(
        "type".to_string(),
        FieldValue::String("requirement".to_string()),
    );

    ContentItem {
        id: "REQ-002".to_string(),
        title: "Data Encryption".to_string(),
        content: "All sensitive data shall be encrypted at rest and in transit.".to_string(),
        metadata,
        constraints: vec!["status_complete".to_string(), "priority_valid".to_string()],
        relationships: HashMap::new(),
        location: ItemLocation {
            docname: "requirements/security.rst".to_string(),
            lineno: Some(25),
            source_path: Some("docs/requirements/security.rst".to_string()),
        },
        style: None,
    }
}

fn create_validation_config() -> ValidationConfig {
    let mut constraints = HashMap::new();

    // Constraint to check that critical/high priority items are complete
    constraints.insert(
        "status_complete".to_string(),
        ConstraintDefinition {
            checks: {
                let mut checks = HashMap::new();
                checks.insert(
                    "check_0".to_string(),
                    "priority != 'critical' or status == 'complete'".to_string(),
                );
                checks.insert(
                    "check_1".to_string(),
                    "priority != 'high' or status == 'complete' or status == 'verified'".to_string(),
                );
                checks
            },
            severity: ValidationSeverity::Error,
            error_message: Some("High/critical priority item {{id}} '{{title}}' must be complete (current status: {{status}})".to_string()),
            description: Some("Ensures high and critical priority items are completed".to_string()),
        },
    );

    // Constraint to validate priority values
    constraints.insert(
        "priority_valid".to_string(),
        ConstraintDefinition {
            checks: {
                let mut checks = HashMap::new();
                checks.insert(
                    "check_0".to_string(),
                    "priority in ['low', 'medium', 'high', 'critical']".to_string(),
                );
                checks
            },
            severity: ValidationSeverity::Warning,
            error_message: Some("Item {{id}} has invalid priority '{{priority}}' - must be one of: low, medium, high, critical".to_string()),
            description: Some("Validates that priority field contains valid values".to_string()),
        },
    );

    ValidationConfig {
        constraints,
        constraint_failed_options: ValidationConfig::default().constraint_failed_options,
        settings: ValidationSettings {
            enable_constraints: true,
            cache_results: true,
            max_errors: Some(10),
            continue_on_error: true,
        },
    }
}
