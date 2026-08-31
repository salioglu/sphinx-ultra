//! Sphinx Ultra Builder
//!
//! A high-performance Rust-based Sphinx documentation builder designed for large codebases.

pub mod builder;
pub mod cache;
pub mod config;
pub mod directives;
pub mod doctree;
pub mod document;
pub mod domains;
pub mod env;
pub mod error;
pub mod extensions;
pub mod html_builder;
pub mod intersphinx;
pub mod inventory;
pub mod matching;
pub mod parser;
pub mod python_config;
pub mod rst;
pub mod search;
pub mod template;
pub mod utils;
pub mod validation;

pub use builder::{BuildStats, SphinxBuilder};
pub use config::BuildConfig;
pub use directives::{
    validation::{
        DirectiveValidationResult, DirectiveValidationSystem, DirectiveValidator, ParsedDirective,
        ParsedRole, RoleValidationResult, RoleValidator,
        ValidationStatistics as DirectiveValidationStatistics,
    },
    Directive, DirectiveRegistry,
};
pub use document::Document;
pub use domains::{CrossReference, DomainObject, DomainRegistry, DomainValidator, ReferenceType};
pub use env::BuildEnvironment;
pub use error::BuildError;
pub use extensions::{ExtensionLoader, SphinxApp, SphinxExtension};
pub use html_builder::HTMLBuilder;
pub use inventory::{posix_join, InvObject, Inventory, InventoryFile, InventoryItem};
pub use parser::Parser;
pub use python_config::{ConfPyConfig, PythonConfigParser};
pub use search::SearchIndex;
pub use template::TemplateEngine;
pub use utils::{analyze_project, ProjectStats};
pub use validation::{
    ConstraintEngine, ConstraintValidator, ContentItem, FieldValue, ValidationConfig,
    ValidationContext, ValidationResult, ValidationRule, ValidationSeverity, Validator,
};
