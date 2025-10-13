//! Content constraint validation system
//!
//! This module implements a constraint validation system inspired by sphinx-needs,
//! providing schema-based validation, custom constraint rules, and severity-based
//! actions for validation failures.

pub mod constraint_engine;
pub mod expression_evaluator;

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::BuildError;

pub use constraint_engine::ConstraintEngine;
pub use expression_evaluator::ExpressionEvaluator;

/// Represents the severity level of a validation failure
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValidationSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

impl fmt::Display for ValidationSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationSeverity::Info => write!(f, "info"),
            ValidationSeverity::Warning => write!(f, "warning"),
            ValidationSeverity::Error => write!(f, "error"),
            ValidationSeverity::Critical => write!(f, "critical"),
        }
    }
}

/// Actions to take when a constraint validation fails
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstraintActions {
    /// Actions to execute on failure (warn, break)
    pub on_fail: Vec<FailureAction>,
    /// Style changes to apply
    pub style_changes: Vec<String>,
    /// Whether to force style changes (replace) or append them
    pub force_style: bool,
}

impl Default for ConstraintActions {
    fn default() -> Self {
        Self {
            on_fail: vec![FailureAction::Warn],
            style_changes: Vec::new(),
            force_style: false,
        }
    }
}

/// Specific actions to take on validation failure
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailureAction {
    /// Log a warning
    Warn,
    /// Break the build (fail with error)
    Break,
    /// Apply style changes
    Style,
}

/// A constraint validation rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationRule {
    /// Unique name/identifier for this rule
    pub name: String,
    /// Human-readable description
    pub description: Option<String>,
    /// Constraint expression (Jinja2-like template)
    pub constraint: String,
    /// Severity level for failures
    pub severity: ValidationSeverity,
    /// Actions to take on failure
    pub actions: ConstraintActions,
    /// Error message template (supports variable substitution)
    pub error_template: Option<String>,
}

/// Result of a single validation check
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether the validation passed
    pub passed: bool,
    /// Error message if validation failed
    pub error_message: Option<String>,
    /// Additional context or details
    pub context: HashMap<String, String>,
}

impl ValidationResult {
    /// Create a successful validation result
    pub fn success() -> Self {
        Self {
            passed: true,
            error_message: None,
            context: HashMap::new(),
        }
    }

    /// Create a failed validation result with message
    pub fn failure(message: String) -> Self {
        Self {
            passed: false,
            error_message: Some(message),
            context: HashMap::new(),
        }
    }

    /// Create a failed validation result with message and context
    pub fn failure_with_context(message: String, context: HashMap<String, String>) -> Self {
        Self {
            passed: false,
            error_message: Some(message),
            context,
        }
    }

    /// Add context to this result
    pub fn with_context(mut self, key: String, value: String) -> Self {
        self.context.insert(key, value);
        self
    }
}

/// Results from applying constraint actions
#[derive(Debug)]
pub struct ActionResult {
    /// Whether actions were applied successfully
    pub success: bool,
    /// Any warnings generated during action application
    pub warnings: Vec<String>,
    /// Any errors that occurred
    pub errors: Vec<BuildError>,
}

impl ActionResult {
    pub fn success() -> Self {
        Self {
            success: true,
            warnings: Vec::new(),
            errors: Vec::new(),
        }
    }

    pub fn failure(error: BuildError) -> Self {
        Self {
            success: false,
            warnings: Vec::new(),
            errors: vec![error],
        }
    }
}

/// A content item that can be validated
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentItem {
    /// Unique identifier
    pub id: String,
    /// Title/name of the item
    pub title: String,
    /// Content body
    pub content: String,
    /// Metadata fields with typed values
    pub metadata: HashMap<String, FieldValue>,
    /// List of constraint names that apply to this item
    pub constraints: Vec<String>,
    /// Relationships to other content items
    pub relationships: HashMap<String, Vec<String>>,
    /// Document location information
    pub location: ItemLocation,
    /// Current style applied to this item
    pub style: Option<String>,
}

/// Typed field value for content item metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Array(Vec<FieldValue>),
    Object(HashMap<String, FieldValue>),
}

impl FieldValue {
    /// Get the value as a string
    pub fn as_string(&self) -> Option<&str> {
        match self {
            FieldValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// Get the value as an integer
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            FieldValue::Integer(i) => Some(*i),
            _ => None,
        }
    }

    /// Get the value as a boolean
    pub fn as_boolean(&self) -> Option<bool> {
        match self {
            FieldValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// Get the value as an array
    pub fn as_array(&self) -> Option<&Vec<FieldValue>> {
        match self {
            FieldValue::Array(arr) => Some(arr),
            _ => None,
        }
    }
}

impl fmt::Display for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FieldValue::String(s) => write!(f, "{}", s),
            FieldValue::Integer(i) => write!(f, "{}", i),
            FieldValue::Float(fl) => write!(f, "{}", fl),
            FieldValue::Boolean(b) => write!(f, "{}", b),
            FieldValue::Array(arr) => {
                write!(f, "[")?;
                for (i, item) in arr.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, "]")
            }
            FieldValue::Object(obj) => {
                write!(f, "{{")?;
                for (i, (key, value)) in obj.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", key, value)?;
                }
                write!(f, "}}")
            }
        }
    }
}

/// Location information for content items
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemLocation {
    /// Document name/path
    pub docname: String,
    /// Line number in the document
    pub lineno: Option<u32>,
    /// Optional source file path
    pub source_path: Option<String>,
}

/// Context for validation operations
#[derive(Debug)]
pub struct ValidationContext<'a> {
    /// The item being validated
    pub current_item: &'a ContentItem,
    /// All content items in the project
    pub all_items: &'a HashMap<String, ContentItem>,
    /// Global configuration values
    pub config: &'a ValidationConfig,
    /// Additional context variables
    pub variables: HashMap<String, FieldValue>,
}

/// Configuration for the validation system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationConfig {
    /// Constraint definitions by name
    pub constraints: HashMap<String, ConstraintDefinition>,
    /// Constraint failure actions by severity
    pub constraint_failed_options: HashMap<String, ConstraintActions>,
    /// Global validation settings
    pub settings: ValidationSettings,
}

impl Default for ValidationConfig {
    fn default() -> Self {
        let mut constraint_failed_options = HashMap::new();

        // Default actions for different severity levels
        constraint_failed_options.insert(
            "info".to_string(),
            ConstraintActions {
                on_fail: vec![],
                style_changes: vec![],
                force_style: false,
            },
        );

        constraint_failed_options.insert(
            "warning".to_string(),
            ConstraintActions {
                on_fail: vec![FailureAction::Warn],
                style_changes: vec!["constraint-warning".to_string()],
                force_style: false,
            },
        );

        constraint_failed_options.insert(
            "error".to_string(),
            ConstraintActions {
                on_fail: vec![FailureAction::Warn, FailureAction::Style],
                style_changes: vec!["constraint-error".to_string()],
                force_style: false,
            },
        );

        constraint_failed_options.insert(
            "critical".to_string(),
            ConstraintActions {
                on_fail: vec![FailureAction::Break],
                style_changes: vec!["constraint-critical".to_string()],
                force_style: true,
            },
        );

        Self {
            constraints: HashMap::new(),
            constraint_failed_options,
            settings: ValidationSettings::default(),
        }
    }
}

/// A constraint definition with multiple checks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstraintDefinition {
    /// Individual check expressions (check_0, check_1, etc.)
    pub checks: HashMap<String, String>,
    /// Severity level for this constraint
    pub severity: ValidationSeverity,
    /// Optional error message template
    pub error_message: Option<String>,
    /// Description of what this constraint validates
    pub description: Option<String>,
}

/// Global validation settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationSettings {
    /// Whether to enable constraint validation
    pub enable_constraints: bool,
    /// Whether to cache validation results
    pub cache_results: bool,
    /// Maximum number of validation errors before stopping
    pub max_errors: Option<usize>,
    /// Whether to continue validation after errors
    pub continue_on_error: bool,
}

impl Default for ValidationSettings {
    fn default() -> Self {
        Self {
            enable_constraints: true,
            cache_results: true,
            max_errors: None,
            continue_on_error: true,
        }
    }
}

/// Core trait for validators
pub trait Validator {
    /// Validate content against rules
    fn validate(&self, context: &ValidationContext) -> ValidationResult;

    /// Get validation rules supported by this validator
    fn get_validation_rules(&self) -> Vec<ValidationRule>;

    /// Get the severity level for this validator
    fn get_severity(&self) -> ValidationSeverity;

    /// Whether this validator supports incremental validation
    fn supports_incremental(&self) -> bool {
        false
    }
}

/// Trait for constraint-specific validation
pub trait ConstraintValidator: Validator {
    /// Validate a specific constraint rule against a content item
    fn validate_constraint(&self, rule: &ValidationRule, item: &ContentItem) -> ValidationResult;

    /// Apply actions based on validation failures
    fn apply_actions(
        &self,
        failures: &[ValidationFailure],
        actions: &ConstraintActions,
    ) -> ActionResult;
}

/// A validation failure with detailed information
#[derive(Debug, Clone)]
pub struct ValidationFailure {
    /// The validation rule that failed
    pub rule: ValidationRule,
    /// The result of the failed validation
    pub result: ValidationResult,
    /// The content item that failed validation
    pub item_id: String,
    /// Severity of the failure
    pub severity: ValidationSeverity,
}

impl ValidationFailure {
    pub fn new(rule: ValidationRule, result: ValidationResult, item_id: String) -> Self {
        let severity = rule.severity;
        Self {
            rule,
            result,
            item_id,
            severity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validation_result_creation() {
        let success = ValidationResult::success();
        assert!(success.passed);
        assert!(success.error_message.is_none());

        let failure = ValidationResult::failure("Test error".to_string());
        assert!(!failure.passed);
        assert_eq!(failure.error_message.unwrap(), "Test error");
    }

    #[test]
    fn test_field_value_display() {
        let string_val = FieldValue::String("test".to_string());
        assert_eq!(format!("{}", string_val), "test");

        let int_val = FieldValue::Integer(42);
        assert_eq!(format!("{}", int_val), "42");

        let bool_val = FieldValue::Boolean(true);
        assert_eq!(format!("{}", bool_val), "true");
    }

    #[test]
    fn test_validation_config_defaults() {
        let config = ValidationConfig::default();
        assert!(config.settings.enable_constraints);
        assert!(config.settings.cache_results);
        assert!(config.constraint_failed_options.contains_key("critical"));
    }

    #[test]
    fn test_content_item_creation() {
        let item = ContentItem {
            id: "test-001".to_string(),
            title: "Test Item".to_string(),
            content: "Test content".to_string(),
            metadata: HashMap::new(),
            constraints: vec!["test-constraint".to_string()],
            relationships: HashMap::new(),
            location: ItemLocation {
                docname: "test.rst".to_string(),
                lineno: Some(10),
                source_path: None,
            },
            style: None,
        };

        assert_eq!(item.id, "test-001");
        assert_eq!(item.constraints.len(), 1);
    }
}
