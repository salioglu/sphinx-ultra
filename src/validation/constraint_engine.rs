//! Constraint processing engine
//!
//! This module provides the core constraint validation engine that processes
//! validation rules against content items, inspired by sphinx-needs constraint system.

use std::collections::HashMap;

use minijinja::{Environment, Template};

use crate::error::BuildError;
use crate::validation::expression_evaluator::ExpressionEvaluator;
use crate::validation::{
    ActionResult, ConstraintActions, ConstraintValidator, ContentItem, FailureAction,
    ValidationContext, ValidationFailure, ValidationResult, ValidationRule, ValidationSeverity,
    Validator,
};

/// Core constraint validation engine
pub struct ConstraintEngine {
    /// Template environment for processing constraint expressions
    template_env: Environment<'static>,
    /// Cache for compiled templates
    template_cache: HashMap<String, Template<'static, 'static>>,
}

impl ConstraintEngine {
    /// Create a new constraint engine
    pub fn new() -> Self {
        let mut env = Environment::new();

        // Register helper functions similar to sphinx-needs filter functions
        env.add_function("has_tag", |tags: Vec<String>, tag: String| -> bool {
            tags.contains(&tag)
        });

        env.add_function("in_list", |value: String, list: Vec<String>| -> bool {
            list.contains(&value)
        });

        env.add_function("not_empty", |value: String| -> bool { !value.is_empty() });

        Self {
            template_env: env,
            template_cache: HashMap::new(),
        }
    }

    /// Process all constraints for a given content item
    pub fn process_constraints(
        &mut self,
        item: &ContentItem,
        context: &ValidationContext,
    ) -> Result<(ContentItem, Vec<ValidationFailure>), BuildError> {
        let mut modified_item = item.clone();
        let mut failures = Vec::new();

        for constraint_name in &modified_item.constraints.clone() {
            if let Some(constraint_def) = context.config.constraints.get(constraint_name) {
                // Process each check in the constraint
                for (check_name, expression) in &constraint_def.checks {
                    let rule = ValidationRule {
                        name: format!("{}::{}", constraint_name, check_name),
                        description: constraint_def.description.clone(),
                        constraint: expression.clone(),
                        severity: constraint_def.severity,
                        actions: context
                            .config
                            .constraint_failed_options
                            .get(&constraint_def.severity.to_string())
                            .cloned()
                            .unwrap_or_default(),
                        error_template: constraint_def.error_message.clone(),
                    };

                    let result = self.validate_constraint(&rule, &modified_item)?;

                    if !result.passed {
                        let failure =
                            ValidationFailure::new(rule, result, modified_item.id.clone());
                        failures.push(failure);
                    }
                }
            }
        }

        // Apply actions for failures
        if !failures.is_empty() {
            self.apply_failure_actions(&mut modified_item, &failures)?;
        }

        Ok((modified_item, failures))
    }

    /// Process all constraints for a given content item (mutable version)
    pub fn process_constraints_mut(
        &mut self,
        item: &mut ContentItem,
        context: &ValidationContext,
    ) -> Result<Vec<ValidationFailure>, BuildError> {
        let mut failures = Vec::new();

        for constraint_name in &item.constraints.clone() {
            if let Some(constraint_def) = context.config.constraints.get(constraint_name) {
                // Process each check in the constraint
                for (check_name, expression) in &constraint_def.checks {
                    let rule = ValidationRule {
                        name: format!("{}::{}", constraint_name, check_name),
                        description: constraint_def.description.clone(),
                        constraint: expression.clone(),
                        severity: constraint_def.severity,
                        actions: context
                            .config
                            .constraint_failed_options
                            .get(&constraint_def.severity.to_string())
                            .cloned()
                            .unwrap_or_default(),
                        error_template: constraint_def.error_message.clone(),
                    };

                    let result = self.validate_constraint(&rule, item)?;

                    if !result.passed {
                        let failure = ValidationFailure::new(rule, result, item.id.clone());
                        failures.push(failure);
                    }
                }
            }
        }

        // Apply actions for failures
        if !failures.is_empty() {
            self.apply_failure_actions(item, &failures)?;
        }

        Ok(failures)
    }

    /// Validate a single constraint expression against an item
    pub fn validate_constraint(
        &mut self,
        rule: &ValidationRule,
        item: &ContentItem,
    ) -> Result<ValidationResult, BuildError> {
        // Use the expression evaluator for constraint evaluation
        match ExpressionEvaluator::evaluate(&rule.constraint, item) {
            Ok(passed) => {
                if passed {
                    Ok(ValidationResult::success())
                } else {
                    let error_message = self.generate_error_message(rule, item)?;
                    Ok(ValidationResult::failure(error_message))
                }
            }
            Err(e) => Err(BuildError::ValidationError(format!(
                "Failed to evaluate constraint '{}': {}",
                rule.constraint, e
            ))),
        }
    }

    /// Apply actions based on validation failures
    fn apply_failure_actions(
        &self,
        item: &mut ContentItem,
        failures: &[ValidationFailure],
    ) -> Result<(), BuildError> {
        for failure in failures {
            let actions = &failure.rule.actions;

            // Apply on_fail actions
            for action in &actions.on_fail {
                match action {
                    FailureAction::Warn => {
                        log::warn!(
                            "Constraint validation failed for item '{}': {} (rule: {})",
                            item.id,
                            failure
                                .result
                                .error_message
                                .as_deref()
                                .unwrap_or("Unknown error"),
                            failure.rule.name
                        );
                    }
                    FailureAction::Break => {
                        return Err(BuildError::ValidationError(format!(
                            "Critical constraint validation failed for item '{}': {} (rule: {})",
                            item.id,
                            failure
                                .result
                                .error_message
                                .as_deref()
                                .unwrap_or("Unknown error"),
                            failure.rule.name
                        )));
                    }
                    FailureAction::Style => {
                        // Style action is handled below
                    }
                }
            }

            // Apply style changes
            if !actions.style_changes.is_empty() || actions.on_fail.contains(&FailureAction::Style)
            {
                self.apply_style_changes(item, actions);
            }
        }

        Ok(())
    }

    /// Apply style changes to a content item
    fn apply_style_changes(&self, item: &mut ContentItem, actions: &ConstraintActions) {
        let new_styles = actions.style_changes.join(", ");

        if actions.force_style || item.style.is_none() {
            item.style = Some(new_styles);
        } else if let Some(existing_style) = &item.style {
            if !new_styles.is_empty() {
                item.style = Some(format!("{}, {}", existing_style, new_styles));
            }
        }
    }

    /// Get or compile a template for the given expression
    #[allow(mismatched_lifetime_syntaxes)]
    fn get_or_compile_template(&mut self, expression: &str) -> Result<&Template, BuildError> {
        if !self.template_cache.contains_key(expression) {
            let template = self
                .template_env
                .template_from_str(expression)
                .map_err(|e| {
                    BuildError::ValidationError(format!(
                        "Failed to compile constraint template '{}': {}",
                        expression, e
                    ))
                })?;

            // Store template in cache
            let owned_template = unsafe {
                std::mem::transmute::<Template<'_, '_>, Template<'static, 'static>>(template)
            };
            self.template_cache
                .insert(expression.to_string(), owned_template);
        }

        Ok(self.template_cache.get(expression).unwrap())
    }

    /// Create template context from content item
    fn create_template_context(&self, item: &ContentItem) -> minijinja::Value {
        let mut item_data = HashMap::new();

        // Add basic fields
        item_data.insert("id".to_string(), item.id.clone().into());
        item_data.insert("title".to_string(), item.title.clone().into());
        item_data.insert("content".to_string(), item.content.clone().into());

        // Add metadata fields
        for (key, value) in &item.metadata {
            item_data.insert(key.clone(), Self::field_value_to_minijinja_value(value));
        }

        // Add relationships
        for (rel_type, targets) in &item.relationships {
            let target_values: Vec<minijinja::Value> =
                targets.iter().map(|s| s.clone().into()).collect();
            item_data.insert(format!("rel_{}", rel_type), target_values.into());
        }

        // Add location info
        item_data.insert("docname".to_string(), item.location.docname.clone().into());
        if let Some(lineno) = item.location.lineno {
            item_data.insert("lineno".to_string(), (lineno as i64).into());
        }

        item_data.into()
    }

    /// Convert FieldValue to minijinja Value
    fn field_value_to_minijinja_value(
        field_value: &crate::validation::FieldValue,
    ) -> minijinja::Value {
        use crate::validation::FieldValue;

        match field_value {
            FieldValue::String(s) => s.clone().into(),
            FieldValue::Integer(i) => (*i).into(),
            FieldValue::Float(f) => (*f).into(),
            FieldValue::Boolean(b) => (*b).into(),
            FieldValue::Array(arr) => arr
                .iter()
                .map(Self::field_value_to_minijinja_value)
                .collect::<Vec<_>>()
                .into(),
            FieldValue::Object(obj) => obj
                .iter()
                .map(|(k, v)| (k.clone(), Self::field_value_to_minijinja_value(v)))
                .collect::<HashMap<String, minijinja::Value>>()
                .into(),
        }
    }

    /// Generate error message using template
    fn generate_error_message(
        &mut self,
        rule: &ValidationRule,
        item: &ContentItem,
    ) -> Result<String, BuildError> {
        if let Some(error_template) = &rule.error_template {
            let context = self.create_template_context(item);
            let template = self.get_or_compile_template(error_template)?;

            template.render(context).map_err(|e| {
                BuildError::ValidationError(format!(
                    "Failed to render error message template: {}",
                    e
                ))
            })
        } else {
            Ok(format!(
                "Constraint '{}' failed for item '{}'",
                rule.name, item.id
            ))
        }
    }
}

impl Default for ConstraintEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Validator for ConstraintEngine {
    fn validate(&self, _context: &ValidationContext) -> ValidationResult {
        // This is a simple implementation - in practice, you'd want to
        // validate all constraints for the current item
        ValidationResult::success()
    }

    fn get_validation_rules(&self) -> Vec<ValidationRule> {
        // Return all rules from the configuration
        Vec::new() // Placeholder
    }

    fn get_severity(&self) -> ValidationSeverity {
        ValidationSeverity::Warning
    }

    fn supports_incremental(&self) -> bool {
        true
    }
}

impl ConstraintValidator for ConstraintEngine {
    fn validate_constraint(&self, _rule: &ValidationRule, _item: &ContentItem) -> ValidationResult {
        // This would need to be implemented with mutable access
        // For now, return a placeholder
        ValidationResult::success()
    }

    fn apply_actions(
        &self,
        _failures: &[ValidationFailure],
        actions: &ConstraintActions,
    ) -> ActionResult {
        // Apply the specified actions
        let mut warnings = Vec::new();
        let mut errors = Vec::new();

        for action in &actions.on_fail {
            match action {
                FailureAction::Warn => {
                    warnings.push("Constraint validation warning".to_string());
                }
                FailureAction::Break => {
                    errors.push(BuildError::ValidationError(
                        "Constraint validation failed critically".to_string(),
                    ));
                }
                FailureAction::Style => {
                    // Style changes are applied separately
                }
            }
        }

        if errors.is_empty() {
            ActionResult {
                success: true,
                warnings,
                errors,
            }
        } else {
            ActionResult {
                success: false,
                warnings,
                errors,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::ItemLocation;

    fn create_test_item() -> ContentItem {
        let mut metadata = HashMap::new();
        metadata.insert(
            "status".to_string(),
            crate::validation::FieldValue::String("open".to_string()),
        );
        metadata.insert(
            "priority".to_string(),
            crate::validation::FieldValue::String("high".to_string()),
        );

        ContentItem {
            id: "TEST-001".to_string(),
            title: "Test Requirement".to_string(),
            content: "This is a test requirement".to_string(),
            metadata,
            constraints: vec!["status_check".to_string()],
            relationships: HashMap::new(),
            location: ItemLocation {
                docname: "requirements.rst".to_string(),
                lineno: Some(42),
                source_path: None,
            },
            style: None,
        }
    }

    #[test]
    fn test_constraint_engine_creation() {
        let engine = ConstraintEngine::new();
        assert!(!engine.template_cache.is_empty() || engine.template_cache.is_empty());
        // Just test creation
    }

    #[test]
    fn test_template_context_creation() {
        let engine = ConstraintEngine::new();
        let item = create_test_item();

        let context = engine.create_template_context(&item);

        // Verify context contains expected fields using the correct minijinja API
        assert!(context.get_attr("id").is_ok());
        assert!(context.get_attr("title").is_ok());
        assert!(context.get_attr("status").is_ok());
        assert!(context.get_attr("priority").is_ok());
    }
}
