//! Directive and Role Validation System
//!
//! This module provides comprehensive validation for Sphinx directives and roles,
//! including option validation, content requirements, and parameter checking.

use std::collections::HashMap;
use std::fmt;

pub mod builtin;
pub mod parser;
pub mod roles;

pub use builtin::*;
pub use parser::*;
pub use roles::*;

/// Source location information for diagnostics
#[derive(Debug, Clone, PartialEq)]
pub struct SourceLocation {
    /// File path where the directive/role was found
    pub file: String,
    /// Line number (1-based)
    pub line: usize,
    /// Column number (1-based)
    pub column: usize,
}

/// Represents a parsed directive with validation context
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedDirective {
    /// The directive name (e.g., "code-block", "note", "warning")
    pub name: String,
    /// Arguments provided to the directive
    pub arguments: Vec<String>,
    /// Options specified for the directive (key-value pairs)
    pub options: HashMap<String, String>,
    /// The content body of the directive
    pub content: String,
    /// Source location information
    pub location: SourceLocation,
}

/// Represents a parsed role with validation context
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedRole {
    /// The role name (e.g., "doc", "ref", "download")
    pub name: String,
    /// The target of the role
    pub target: String,
    /// Display text (if different from target)
    pub display_text: Option<String>,
    /// Source location information
    pub location: SourceLocation,
}

/// Result of directive validation
#[derive(Debug, Clone, PartialEq)]
pub enum DirectiveValidationResult {
    /// Directive is valid
    Valid,
    /// Directive has warnings but is acceptable
    Warning(String),
    /// Directive has errors and should be fixed
    Error(String),
    /// Directive is unknown/unregistered
    Unknown,
}

/// Result of role validation
#[derive(Debug, Clone, PartialEq)]
pub enum RoleValidationResult {
    /// Role is valid
    Valid,
    /// Role has warnings but is acceptable
    Warning(String),
    /// Role has errors and should be fixed
    Error(String),
    /// Role is unknown/unregistered
    Unknown,
}

/// Trait for implementing directive validators
pub trait DirectiveValidator: Send + Sync {
    /// Returns the name of the directive this validator handles
    fn name(&self) -> &str;

    /// Validates a parsed directive
    fn validate(&self, directive: &ParsedDirective) -> DirectiveValidationResult;

    /// Returns expected arguments for this directive
    fn expected_arguments(&self) -> Vec<String>;

    /// Returns valid options for this directive
    fn valid_options(&self) -> Vec<String>;

    /// Returns whether this directive requires content
    fn requires_content(&self) -> bool;

    /// Returns whether this directive allows content
    fn allows_content(&self) -> bool;

    /// Provides suggestions for fixing directive issues
    fn get_suggestions(&self, directive: &ParsedDirective) -> Vec<String> {
        let mut suggestions = Vec::new();

        // Check for common issues and provide suggestions
        if directive.content.is_empty() && self.requires_content() {
            suggestions.push(format!("The '{}' directive requires content", self.name()));
        }

        if !directive.content.is_empty() && !self.allows_content() {
            suggestions.push(format!(
                "The '{}' directive does not allow content",
                self.name()
            ));
        }

        // Check for invalid options
        let valid_options = self.valid_options();
        for option in directive.options.keys() {
            if !valid_options.contains(&option.to_string()) {
                suggestions.push(format!(
                    "Unknown option '{}' for directive '{}'",
                    option,
                    self.name()
                ));

                // Suggest similar options
                for valid_option in &valid_options {
                    if valid_option.contains(option) || option.contains(valid_option) {
                        suggestions.push(format!("Did you mean '{}'?", valid_option));
                        break;
                    }
                }
            }
        }

        suggestions
    }
}

/// Trait for implementing role validators
pub trait RoleValidator: Send + Sync {
    /// Returns the name of the role this validator handles
    fn name(&self) -> &str;

    /// Validates a parsed role
    fn validate(&self, role: &ParsedRole) -> RoleValidationResult;

    /// Returns whether this role requires a target
    fn requires_target(&self) -> bool;

    /// Returns whether this role allows display text
    fn allows_display_text(&self) -> bool;

    /// Provides suggestions for fixing role issues
    fn get_suggestions(&self, role: &ParsedRole) -> Vec<String> {
        let mut suggestions = Vec::new();

        if role.target.is_empty() && self.requires_target() {
            suggestions.push(format!("The '{}' role requires a target", self.name()));
        }

        if role.display_text.is_some() && !self.allows_display_text() {
            suggestions.push(format!(
                "The '{}' role does not support display text",
                self.name()
            ));
        }

        suggestions
    }
}

/// Registry for managing directive validators
#[derive(Default)]
pub struct DirectiveRegistry {
    validators: HashMap<String, Box<dyn DirectiveValidator>>,
}

impl DirectiveRegistry {
    /// Creates a new directive registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a registry with built-in validators
    pub fn with_builtin_validators() -> Self {
        let mut registry = Self::new();
        registry.register_builtin_validators();
        registry
    }

    /// Registers a directive validator
    pub fn register_validator(&mut self, validator: Box<dyn DirectiveValidator>) {
        let name = validator.name().to_string();
        self.validators.insert(name, validator);
    }

    /// Registers all built-in validators
    pub fn register_builtin_validators(&mut self) {
        // Register built-in directive validators
        self.register_validator(Box::new(builtin::CodeBlockValidator::new()));
        self.register_validator(Box::new(builtin::NoteValidator::new()));
        self.register_validator(Box::new(builtin::WarningValidator::new()));
        self.register_validator(Box::new(builtin::ImageValidator::new()));
        self.register_validator(Box::new(builtin::FigureValidator::new()));
        self.register_validator(Box::new(builtin::TocTreeValidator::new()));
        self.register_validator(Box::new(builtin::IncludeValidator::new()));
        self.register_validator(Box::new(builtin::LiteralIncludeValidator::new()));
        self.register_validator(Box::new(builtin::AdmonitionValidator::new()));
        self.register_validator(Box::new(builtin::MathValidator::new()));
    }

    /// Validates a directive
    pub fn validate_directive(&self, directive: &ParsedDirective) -> DirectiveValidationResult {
        match self.validators.get(&directive.name) {
            Some(validator) => validator.validate(directive),
            None => DirectiveValidationResult::Unknown,
        }
    }

    /// Gets suggestions for a directive
    pub fn get_directive_suggestions(&self, directive: &ParsedDirective) -> Vec<String> {
        match self.validators.get(&directive.name) {
            Some(validator) => validator.get_suggestions(directive),
            None => {
                let mut suggestions = vec![format!("Unknown directive '{}'", directive.name)];

                // Suggest similar directive names
                for validator_name in self.validators.keys() {
                    if validator_name.contains(&directive.name)
                        || directive.name.contains(validator_name)
                    {
                        suggestions.push(format!("Did you mean '{}'?", validator_name));
                    }
                }

                suggestions
            }
        }
    }

    /// Returns all registered directive names
    pub fn get_registered_directives(&self) -> Vec<String> {
        self.validators.keys().cloned().collect()
    }

    /// Checks if a directive is registered
    pub fn is_directive_registered(&self, name: &str) -> bool {
        self.validators.contains_key(name)
    }
}

/// Registry for managing role validators
#[derive(Default)]
pub struct RoleRegistry {
    validators: HashMap<String, Box<dyn RoleValidator>>,
}

impl RoleRegistry {
    /// Creates a new role registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a registry with built-in validators
    pub fn with_builtin_validators() -> Self {
        let mut registry = Self::new();
        registry.register_builtin_validators();
        registry
    }

    /// Registers a role validator
    pub fn register_validator(&mut self, validator: Box<dyn RoleValidator>) {
        let name = validator.name().to_string();
        self.validators.insert(name, validator);
    }

    /// Registers all built-in validators
    pub fn register_builtin_validators(&mut self) {
        // Register built-in role validators
        self.register_validator(Box::new(roles::DocRoleValidator::new()));
        self.register_validator(Box::new(roles::RefRoleValidator::new()));
        self.register_validator(Box::new(roles::DownloadRoleValidator::new()));
        self.register_validator(Box::new(roles::MathRoleValidator::new()));
        self.register_validator(Box::new(roles::AbbreviationRoleValidator::new()));
        self.register_validator(Box::new(roles::CommandRoleValidator::new()));
        self.register_validator(Box::new(roles::FileRoleValidator::new()));
        self.register_validator(Box::new(roles::KbdRoleValidator::new()));
        self.register_validator(Box::new(roles::MenuSelectionRoleValidator::new()));
        self.register_validator(Box::new(roles::GuiLabelRoleValidator::new()));
    }

    /// Validates a role
    pub fn validate_role(&self, role: &ParsedRole) -> RoleValidationResult {
        match self.validators.get(&role.name) {
            Some(validator) => validator.validate(role),
            None => RoleValidationResult::Unknown,
        }
    }

    /// Gets suggestions for a role
    pub fn get_role_suggestions(&self, role: &ParsedRole) -> Vec<String> {
        match self.validators.get(&role.name) {
            Some(validator) => validator.get_suggestions(role),
            None => {
                let mut suggestions = vec![format!("Unknown role '{}'", role.name)];

                // Suggest similar role names
                for validator_name in self.validators.keys() {
                    if validator_name.contains(&role.name) || role.name.contains(validator_name) {
                        suggestions.push(format!("Did you mean '{}'?", validator_name));
                    }
                }

                suggestions
            }
        }
    }

    /// Returns all registered role names
    pub fn get_registered_roles(&self) -> Vec<String> {
        self.validators.keys().cloned().collect()
    }

    /// Checks if a role is registered
    pub fn is_role_registered(&self, name: &str) -> bool {
        self.validators.contains_key(name)
    }
}

/// Combined validation statistics
#[derive(Debug, Default, Clone)]
pub struct ValidationStatistics {
    /// Total number of directives processed
    pub total_directives: usize,
    /// Number of valid directives
    pub valid_directives: usize,
    /// Number of directives with warnings
    pub warning_directives: usize,
    /// Number of directives with errors
    pub error_directives: usize,
    /// Number of unknown directives
    pub unknown_directives: usize,

    /// Total number of roles processed
    pub total_roles: usize,
    /// Number of valid roles
    pub valid_roles: usize,
    /// Number of roles with warnings
    pub warning_roles: usize,
    /// Number of roles with errors
    pub error_roles: usize,
    /// Number of unknown roles
    pub unknown_roles: usize,

    /// Breakdown by directive type
    pub directives_by_type: HashMap<String, usize>,
    /// Breakdown by role type
    pub roles_by_type: HashMap<String, usize>,
}

impl ValidationStatistics {
    /// Creates new validation statistics
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a directive validation result
    pub fn record_directive(
        &mut self,
        directive: &ParsedDirective,
        result: &DirectiveValidationResult,
    ) {
        self.total_directives += 1;
        *self
            .directives_by_type
            .entry(directive.name.clone())
            .or_insert(0) += 1;

        match result {
            DirectiveValidationResult::Valid => self.valid_directives += 1,
            DirectiveValidationResult::Warning(_) => self.warning_directives += 1,
            DirectiveValidationResult::Error(_) => self.error_directives += 1,
            DirectiveValidationResult::Unknown => self.unknown_directives += 1,
        }
    }

    /// Records a role validation result
    pub fn record_role(&mut self, role: &ParsedRole, result: &RoleValidationResult) {
        self.total_roles += 1;
        *self.roles_by_type.entry(role.name.clone()).or_insert(0) += 1;

        match result {
            RoleValidationResult::Valid => self.valid_roles += 1,
            RoleValidationResult::Warning(_) => self.warning_roles += 1,
            RoleValidationResult::Error(_) => self.error_roles += 1,
            RoleValidationResult::Unknown => self.unknown_roles += 1,
        }
    }

    /// Returns validation success rate for directives (0.0 to 1.0)
    pub fn directive_success_rate(&self) -> f64 {
        if self.total_directives == 0 {
            return 1.0;
        }
        self.valid_directives as f64 / self.total_directives as f64
    }

    /// Returns validation success rate for roles (0.0 to 1.0)
    pub fn role_success_rate(&self) -> f64 {
        if self.total_roles == 0 {
            return 1.0;
        }
        self.valid_roles as f64 / self.total_roles as f64
    }

    /// Returns overall validation success rate (0.0 to 1.0)
    pub fn overall_success_rate(&self) -> f64 {
        let total = self.total_directives + self.total_roles;
        if total == 0 {
            return 1.0;
        }
        (self.valid_directives + self.valid_roles) as f64 / total as f64
    }
}

impl fmt::Display for ValidationStatistics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Directive & Role Validation Statistics")?;
        writeln!(f, "=======================================")?;
        writeln!(f)?;

        writeln!(f, "Directives:")?;
        writeln!(f, "  Total: {}", self.total_directives)?;
        if self.total_directives > 0 {
            writeln!(
                f,
                "  Valid: {} ({:.1}%)",
                self.valid_directives,
                self.valid_directives as f64 / self.total_directives as f64 * 100.0
            )?;
            writeln!(
                f,
                "  Warnings: {} ({:.1}%)",
                self.warning_directives,
                self.warning_directives as f64 / self.total_directives as f64 * 100.0
            )?;
            writeln!(
                f,
                "  Errors: {} ({:.1}%)",
                self.error_directives,
                self.error_directives as f64 / self.total_directives as f64 * 100.0
            )?;
            writeln!(
                f,
                "  Unknown: {} ({:.1}%)",
                self.unknown_directives,
                self.unknown_directives as f64 / self.total_directives as f64 * 100.0
            )?;
        }
        writeln!(f)?;

        writeln!(f, "Roles:")?;
        writeln!(f, "  Total: {}", self.total_roles)?;
        if self.total_roles > 0 {
            writeln!(
                f,
                "  Valid: {} ({:.1}%)",
                self.valid_roles,
                self.valid_roles as f64 / self.total_roles as f64 * 100.0
            )?;
            writeln!(
                f,
                "  Warnings: {} ({:.1}%)",
                self.warning_roles,
                self.warning_roles as f64 / self.total_roles as f64 * 100.0
            )?;
            writeln!(
                f,
                "  Errors: {} ({:.1}%)",
                self.error_roles,
                self.error_roles as f64 / self.total_roles as f64 * 100.0
            )?;
            writeln!(
                f,
                "  Unknown: {} ({:.1}%)",
                self.unknown_roles,
                self.unknown_roles as f64 / self.total_roles as f64 * 100.0
            )?;
        }
        writeln!(f)?;

        writeln!(
            f,
            "Overall Success Rate: {:.1}%",
            self.overall_success_rate() * 100.0
        )?;

        Ok(())
    }
}

/// Comprehensive directive and role validation system
pub struct DirectiveValidationSystem {
    directive_registry: DirectiveRegistry,
    role_registry: RoleRegistry,
    statistics: ValidationStatistics,
}

impl Default for DirectiveValidationSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl DirectiveValidationSystem {
    /// Creates a new validation system with built-in validators
    pub fn new() -> Self {
        Self {
            directive_registry: DirectiveRegistry::with_builtin_validators(),
            role_registry: RoleRegistry::with_builtin_validators(),
            statistics: ValidationStatistics::new(),
        }
    }

    /// Gets a reference to the directive registry
    pub fn directive_registry(&self) -> &DirectiveRegistry {
        &self.directive_registry
    }

    /// Gets a mutable reference to the directive registry
    pub fn directive_registry_mut(&mut self) -> &mut DirectiveRegistry {
        &mut self.directive_registry
    }

    /// Gets a reference to the role registry
    pub fn role_registry(&self) -> &RoleRegistry {
        &self.role_registry
    }

    /// Gets a mutable reference to the role registry
    pub fn role_registry_mut(&mut self) -> &mut RoleRegistry {
        &mut self.role_registry
    }

    /// Validates a directive and updates statistics
    pub fn validate_directive(&mut self, directive: &ParsedDirective) -> DirectiveValidationResult {
        let result = self.directive_registry.validate_directive(directive);
        self.statistics.record_directive(directive, &result);
        result
    }

    /// Validates a role and updates statistics
    pub fn validate_role(&mut self, role: &ParsedRole) -> RoleValidationResult {
        let result = self.role_registry.validate_role(role);
        self.statistics.record_role(role, &result);
        result
    }

    /// Gets directive suggestions
    pub fn get_directive_suggestions(&self, directive: &ParsedDirective) -> Vec<String> {
        self.directive_registry.get_directive_suggestions(directive)
    }

    /// Gets role suggestions
    pub fn get_role_suggestions(&self, role: &ParsedRole) -> Vec<String> {
        self.role_registry.get_role_suggestions(role)
    }

    /// Returns current validation statistics
    pub fn statistics(&self) -> &ValidationStatistics {
        &self.statistics
    }

    /// Resets validation statistics
    pub fn reset_statistics(&mut self) {
        self.statistics = ValidationStatistics::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_directive_registry_creation() {
        let registry = DirectiveRegistry::new();
        assert_eq!(registry.get_registered_directives().len(), 0);
    }

    #[test]
    fn test_directive_registry_with_builtin() {
        let registry = DirectiveRegistry::with_builtin_validators();
        assert!(!registry.get_registered_directives().is_empty());
        assert!(registry.is_directive_registered("code-block"));
        assert!(registry.is_directive_registered("note"));
        assert!(registry.is_directive_registered("warning"));
    }

    #[test]
    fn test_role_registry_creation() {
        let registry = RoleRegistry::new();
        assert_eq!(registry.get_registered_roles().len(), 0);
    }

    #[test]
    fn test_role_registry_with_builtin() {
        let registry = RoleRegistry::with_builtin_validators();
        assert!(!registry.get_registered_roles().is_empty());
        assert!(registry.is_role_registered("doc"));
        assert!(registry.is_role_registered("ref"));
        assert!(registry.is_role_registered("download"));
    }

    #[test]
    fn test_validation_statistics() {
        let mut stats = ValidationStatistics::new();

        let directive = ParsedDirective {
            name: "note".to_string(),
            arguments: vec![],
            options: HashMap::new(),
            content: "Test content".to_string(),
            location: SourceLocation {
                file: "test.rst".to_string(),
                line: 1,
                column: 1,
            },
        };

        stats.record_directive(&directive, &DirectiveValidationResult::Valid);
        assert_eq!(stats.total_directives, 1);
        assert_eq!(stats.valid_directives, 1);
        assert_eq!(stats.directive_success_rate(), 1.0);
    }

    #[test]
    fn test_validation_system() {
        let mut system = DirectiveValidationSystem::new();

        let directive = ParsedDirective {
            name: "note".to_string(),
            arguments: vec![],
            options: HashMap::new(),
            content: "Test content".to_string(),
            location: SourceLocation {
                file: "test.rst".to_string(),
                line: 1,
                column: 1,
            },
        };

        let result = system.validate_directive(&directive);
        assert_eq!(result, DirectiveValidationResult::Valid);
        assert_eq!(system.statistics().total_directives, 1);
    }
}
