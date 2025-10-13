//! Directive & Role Validation System Example
//!
//! This example demonstrates the comprehensive directive and role validation system,
//! showing how to validate RST content, detect errors and warnings, and get suggestions
//! for fixing issues.

use sphinx_ultra::directives::validation::{
    DirectiveValidationResult, DirectiveValidationSystem, RoleValidationResult,
    StatisticalDirectiveRoleParser,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Sphinx Ultra - Directive & Role Validation System Example");
    println!("=========================================================");
    println!();

    // Create a validation system with built-in validators
    let mut validation_system = DirectiveValidationSystem::new();

    println!("✅ Initialized directive and role validation system");
    println!(
        "📋 Registered {} directives and {} roles",
        validation_system
            .directive_registry()
            .get_registered_directives()
            .len(),
        validation_system
            .role_registry()
            .get_registered_roles()
            .len()
    );
    println!();

    // Sample RST content with various directives and roles
    let sample_content = r#"
Getting Started Guide
=====================

Welcome to our documentation! This guide shows various Sphinx directives and roles.

.. note:: 
   This is an important note about the setup process.
   Make sure to read this carefully.

Installation
------------

First, download the :download:`installer.exe` file from our website.

.. code-block:: python
   :linenos:
   :caption: Example Python code

   def hello_world():
       print("Hello, world!")
       return True

.. warning::
   Be careful when running this command!

Advanced Configuration
----------------------

See the :doc:`configuration` guide for details. You can also refer to 
:ref:`advanced-settings` for more options.

For keyboard shortcuts, use :kbd:`Ctrl+C` to copy and :kbd:`Ctrl+V` to paste.

.. figure:: architecture.png
   :width: 500px
   :alt: System architecture diagram
   :align: center

   This figure shows the overall system architecture.

Mathematical expressions can be written as :math:`x = \frac{a + b}{c}`.

.. admonition:: Custom Note
   :class: tip

   This is a custom admonition with additional styling.

Common Issues
-------------

.. literalinclude:: examples/config.py
   :language: python
   :lines: 1-20
   :emphasize-lines: 5,10

For more help, see :doc:`troubleshooting` or contact support.

Problematic Examples
--------------------

.. note::

.. code-block::
   
   print("No language specified")

.. image::

See :doc:`` and :ref:``.

Download :download:`nonexistent` file.

.. unknowndirective:: test

Use :unknownrole:`something` here.

.. math::

.. figure:: image.png
   :width: invalid-width
   :align: invalid-alignment

.. toctree::
   :maxdepth: not-a-number
"#;

    println!("📄 Parsing sample RST content...");

    // Parse the content to extract directives and roles
    let mut parser = StatisticalDirectiveRoleParser::new("getting-started.rst".to_string());
    let (directives, roles) = parser.parse_with_statistics(sample_content);

    let parse_stats = parser.statistics();
    println!(
        "🔍 Found {} directives and {} roles:",
        parse_stats.directive_count, parse_stats.role_count
    );

    // Display found directives
    for (i, directive) in directives.iter().enumerate() {
        println!(
            "  {}. {} -> '{}' (line {}, {} args, {} options)",
            i + 1,
            directive.name,
            directive.arguments.join(" "),
            directive.location.line,
            directive.arguments.len(),
            directive.options.len()
        );
    }

    // Display found roles
    for (i, role) in roles.iter().enumerate() {
        println!(
            "  {}. {} -> '{}' (line {}{})",
            i + 1 + directives.len(),
            role.name,
            role.target,
            role.location.line,
            if role.display_text.is_some() {
                " with display text"
            } else {
                ""
            }
        );
    }
    println!();

    println!("🔍 Validating directives and roles...");
    println!();

    // Track validation issues
    let mut valid_count = 0;
    let mut warning_count = 0;
    let mut error_count = 0;
    let mut unknown_count = 0;
    let mut issues = Vec::new();

    // Validate all directives
    for directive in &directives {
        let result = validation_system.validate_directive(directive);

        match &result {
            DirectiveValidationResult::Valid => {
                valid_count += 1;
                println!(
                    "✅ VALID: {} directive '{}' (line {})",
                    directive.name,
                    directive.arguments.join(" "),
                    directive.location.line
                );
            }
            DirectiveValidationResult::Warning(msg) => {
                warning_count += 1;
                println!(
                    "⚠️  WARNING: {} directive '{}' (line {})",
                    directive.name,
                    directive.arguments.join(" "),
                    directive.location.line
                );
                println!("    Issue: {}", msg);
                issues.push(format!(
                    "Directive '{}' at line {}: {}",
                    directive.name, directive.location.line, msg
                ));
            }
            DirectiveValidationResult::Error(msg) => {
                error_count += 1;
                println!(
                    "❌ ERROR: {} directive '{}' (line {})",
                    directive.name,
                    directive.arguments.join(" "),
                    directive.location.line
                );
                println!("    Error: {}", msg);
                issues.push(format!(
                    "Directive '{}' at line {}: {}",
                    directive.name, directive.location.line, msg
                ));
            }
            DirectiveValidationResult::Unknown => {
                unknown_count += 1;
                println!(
                    "❓ UNKNOWN: {} directive '{}' (line {})",
                    directive.name,
                    directive.arguments.join(" "),
                    directive.location.line
                );

                // Get suggestions for unknown directives
                let suggestions = validation_system.get_directive_suggestions(directive);
                if !suggestions.is_empty() {
                    println!("    Suggestions: {}", suggestions.join("; "));
                }
                issues.push(format!(
                    "Unknown directive '{}' at line {}",
                    directive.name, directive.location.line
                ));
            }
        }
    }

    println!();

    // Validate all roles
    for role in &roles {
        let result = validation_system.validate_role(role);

        match &result {
            RoleValidationResult::Valid => {
                valid_count += 1;
                println!(
                    "✅ VALID: {} role '{}' (line {})",
                    role.name, role.target, role.location.line
                );
            }
            RoleValidationResult::Warning(msg) => {
                warning_count += 1;
                println!(
                    "⚠️  WARNING: {} role '{}' (line {})",
                    role.name, role.target, role.location.line
                );
                println!("    Issue: {}", msg);
                issues.push(format!(
                    "Role '{}' at line {}: {}",
                    role.name, role.location.line, msg
                ));
            }
            RoleValidationResult::Error(msg) => {
                error_count += 1;
                println!(
                    "❌ ERROR: {} role '{}' (line {})",
                    role.name, role.target, role.location.line
                );
                println!("    Error: {}", msg);
                issues.push(format!(
                    "Role '{}' at line {}: {}",
                    role.name, role.location.line, msg
                ));
            }
            RoleValidationResult::Unknown => {
                unknown_count += 1;
                println!(
                    "❓ UNKNOWN: {} role '{}' (line {})",
                    role.name, role.target, role.location.line
                );

                // Get suggestions for unknown roles
                let suggestions = validation_system.get_role_suggestions(role);
                if !suggestions.is_empty() {
                    println!("    Suggestions: {}", suggestions.join("; "));
                }
                issues.push(format!(
                    "Unknown role '{}' at line {}",
                    role.name, role.location.line
                ));
            }
        }
    }

    println!();
    println!("📊 Validation Summary");
    println!("==================");
    let total_items = directives.len() + roles.len();
    println!("Total items: {}", total_items);
    println!(
        "Valid: {} ({:.1}%)",
        valid_count,
        valid_count as f64 / total_items as f64 * 100.0
    );
    println!(
        "Warnings: {} ({:.1}%)",
        warning_count,
        warning_count as f64 / total_items as f64 * 100.0
    );
    println!(
        "Errors: {} ({:.1}%)",
        error_count,
        error_count as f64 / total_items as f64 * 100.0
    );
    println!(
        "Unknown: {} ({:.1}%)",
        unknown_count,
        unknown_count as f64 / total_items as f64 * 100.0
    );
    println!();

    // Display detailed statistics
    println!("📈 Detailed Statistics");
    println!("=====================");
    let stats = validation_system.statistics();
    println!("{}", stats);

    // Parse statistics
    println!("📊 Parser Statistics");
    println!("===================");
    println!("Lines processed: {}", parse_stats.lines_processed);
    println!("Items found: {}", parse_stats.total_items());
    println!();

    println!("Directives by type:");
    for (directive_type, count) in &parse_stats.directives_by_type {
        println!("  {}: {}", directive_type, count);
    }
    println!();

    println!("Roles by type:");
    for (role_type, count) in &parse_stats.roles_by_type {
        println!("  {}: {}", role_type, count);
    }
    println!();

    // Show issues summary
    if !issues.is_empty() {
        println!("🚨 Issues Found");
        println!("===============");
        for (i, issue) in issues.iter().enumerate() {
            println!("{}. {}", i + 1, issue);
        }
        println!();
    }

    // Demonstrate directive suggestions
    println!("💡 Suggestion Examples");
    println!("=====================");

    // Create some problematic directives to show suggestions
    let problematic_directives = vec![
        ("note", vec![], std::collections::HashMap::new(), ""), // Note without content
        (
            "code-block",
            vec![],
            std::collections::HashMap::new(),
            "print('test')",
        ), // Code block without language
        ("image", vec![], std::collections::HashMap::new(), ""), // Image without path
    ];

    for (name, args, options, content) in problematic_directives {
        let directive = sphinx_ultra::directives::validation::ParsedDirective {
            name: name.to_string(),
            arguments: args,
            options,
            content: content.to_string(),
            location: sphinx_ultra::directives::validation::SourceLocation {
                file: "example.rst".to_string(),
                line: 1,
                column: 1,
            },
        };

        let suggestions = validation_system.get_directive_suggestions(&directive);
        if !suggestions.is_empty() {
            println!("Directive '{}': {}", name, suggestions.join("; "));
        }
    }

    println!();
    println!("🎉 Directive and role validation completed successfully!");

    let success_rate = valid_count as f64 / total_items as f64 * 100.0;
    println!("Overall success rate: {:.1}%", success_rate);

    if error_count > 0 || unknown_count > 0 {
        println!(
            "⚠️  Please fix the {} error(s) and {} unknown item(s) before building.",
            error_count, unknown_count
        );
        std::process::exit(1);
    } else if warning_count > 0 {
        println!(
            "⚠️  Consider addressing the {} warning(s) to improve documentation quality.",
            warning_count
        );
    } else {
        println!("✅ All directives and roles are valid!");
    }

    Ok(())
}
