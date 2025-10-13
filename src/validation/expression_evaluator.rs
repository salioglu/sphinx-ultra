//! Simple expression evaluator for constraint validation
//!
//! This module provides a basic expression evaluator that can handle
//! constraint expressions similar to those used in sphinx-needs.

use std::collections::HashMap;

use crate::error::BuildError;
use crate::validation::{ContentItem, FieldValue};

/// Simple expression evaluator for constraint validation
pub struct ExpressionEvaluator;

impl ExpressionEvaluator {
    /// Evaluate a constraint expression against a content item
    pub fn evaluate(expression: &str, item: &ContentItem) -> Result<bool, BuildError> {
        // Create context from the item
        let mut context = HashMap::new();

        // Add basic fields
        context.insert("id".to_string(), FieldValue::String(item.id.clone()));
        context.insert("title".to_string(), FieldValue::String(item.title.clone()));
        context.insert(
            "content".to_string(),
            FieldValue::String(item.content.clone()),
        );

        // Add metadata fields
        for (key, value) in &item.metadata {
            context.insert(key.clone(), value.clone());
        }

        // Simple expression parser - handles basic comparisons and logic
        Self::evaluate_expression(expression, &context)
    }

    /// Parse and evaluate a simple boolean expression
    fn evaluate_expression(
        expr: &str,
        context: &HashMap<String, FieldValue>,
    ) -> Result<bool, BuildError> {
        let expr = expr.trim();

        // Handle OR operations
        if expr.contains(" or ") {
            let parts: Vec<&str> = expr.split(" or ").collect();
            for part in parts {
                if Self::evaluate_expression(part.trim(), context)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }

        // Handle AND operations
        if expr.contains(" and ") {
            let parts: Vec<&str> = expr.split(" and ").collect();
            for part in parts {
                if !Self::evaluate_expression(part.trim(), context)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }

        // Handle NOT operations
        if let Some(inner_expr) = expr.strip_prefix("not ") {
            return Ok(!Self::evaluate_expression(inner_expr, context)?);
        }

        // Handle comparisons
        if expr.contains(" == ") {
            let parts: Vec<&str> = expr.split(" == ").collect();
            if parts.len() != 2 {
                return Err(BuildError::ValidationError(format!(
                    "Invalid comparison: {}",
                    expr
                )));
            }
            let left = Self::get_value(parts[0].trim(), context)?;
            let right = Self::parse_literal(parts[1].trim())?;
            return Ok(Self::values_equal(&left, &right));
        }

        if expr.contains(" != ") {
            let parts: Vec<&str> = expr.split(" != ").collect();
            if parts.len() != 2 {
                return Err(BuildError::ValidationError(format!(
                    "Invalid comparison: {}",
                    expr
                )));
            }
            let left = Self::get_value(parts[0].trim(), context)?;
            let right = Self::parse_literal(parts[1].trim())?;
            return Ok(!Self::values_equal(&left, &right));
        }

        // Handle 'in' operations
        if expr.contains(" in ") {
            let parts: Vec<&str> = expr.split(" in ").collect();
            if parts.len() != 2 {
                return Err(BuildError::ValidationError(format!(
                    "Invalid 'in' expression: {}",
                    expr
                )));
            }
            let left = Self::get_value(parts[0].trim(), context)?;
            let right_expr = parts[1].trim();

            // Parse list syntax [item1, item2, ...]
            if right_expr.starts_with('[') && right_expr.ends_with(']') {
                let list_content = &right_expr[1..right_expr.len() - 1];
                let items: Vec<&str> = list_content.split(',').map(|s| s.trim()).collect();

                for item in items {
                    let item_value = Self::parse_literal(item)?;
                    if Self::values_equal(&left, &item_value) {
                        return Ok(true);
                    }
                }
                return Ok(false);
            }
        }

        // Handle simple variable access (return truthy value)
        if let Ok(value) = Self::get_value(expr, context) {
            return Ok(Self::is_truthy(&value));
        }

        Err(BuildError::ValidationError(format!(
            "Could not evaluate expression: {}",
            expr
        )))
    }

    /// Get a value from the context
    fn get_value(
        name: &str,
        context: &HashMap<String, FieldValue>,
    ) -> Result<FieldValue, BuildError> {
        context
            .get(name)
            .cloned()
            .ok_or_else(|| BuildError::ValidationError(format!("Unknown variable: {}", name)))
    }

    /// Parse a literal value (string, number, boolean)
    fn parse_literal(literal: &str) -> Result<FieldValue, BuildError> {
        let literal = literal.trim();

        // String literal
        if (literal.starts_with('\'') && literal.ends_with('\''))
            || (literal.starts_with('"') && literal.ends_with('"'))
        {
            let content = &literal[1..literal.len() - 1];
            return Ok(FieldValue::String(content.to_string()));
        }

        // Boolean literal
        if literal == "true" {
            return Ok(FieldValue::Boolean(true));
        }
        if literal == "false" {
            return Ok(FieldValue::Boolean(false));
        }

        // Number literal
        if let Ok(int_val) = literal.parse::<i64>() {
            return Ok(FieldValue::Integer(int_val));
        }
        if let Ok(float_val) = literal.parse::<f64>() {
            return Ok(FieldValue::Float(float_val));
        }

        // Default to string
        Ok(FieldValue::String(literal.to_string()))
    }

    /// Check if two field values are equal
    fn values_equal(left: &FieldValue, right: &FieldValue) -> bool {
        match (left, right) {
            (FieldValue::String(a), FieldValue::String(b)) => a == b,
            (FieldValue::Integer(a), FieldValue::Integer(b)) => a == b,
            (FieldValue::Float(a), FieldValue::Float(b)) => (a - b).abs() < f64::EPSILON,
            (FieldValue::Boolean(a), FieldValue::Boolean(b)) => a == b,
            (FieldValue::Integer(a), FieldValue::Float(b)) => (*a as f64 - b).abs() < f64::EPSILON,
            (FieldValue::Float(a), FieldValue::Integer(b)) => (a - *b as f64).abs() < f64::EPSILON,
            _ => false,
        }
    }

    /// Check if a value is truthy
    fn is_truthy(value: &FieldValue) -> bool {
        match value {
            FieldValue::String(s) => !s.is_empty(),
            FieldValue::Integer(i) => *i != 0,
            FieldValue::Float(f) => *f != 0.0,
            FieldValue::Boolean(b) => *b,
            FieldValue::Array(arr) => !arr.is_empty(),
            FieldValue::Object(obj) => !obj.is_empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::ItemLocation;

    fn create_test_item() -> ContentItem {
        let mut metadata = HashMap::new();
        metadata.insert("status".to_string(), FieldValue::String("open".to_string()));
        metadata.insert(
            "priority".to_string(),
            FieldValue::String("high".to_string()),
        );

        ContentItem {
            id: "TEST-001".to_string(),
            title: "Test Item".to_string(),
            content: "Test content".to_string(),
            metadata,
            constraints: vec![],
            relationships: HashMap::new(),
            location: ItemLocation {
                docname: "test.rst".to_string(),
                lineno: Some(1),
                source_path: None,
            },
            style: None,
        }
    }

    #[test]
    fn test_simple_equality() {
        let item = create_test_item();

        assert!(ExpressionEvaluator::evaluate("status == 'open'", &item).unwrap());
        assert!(!ExpressionEvaluator::evaluate("status == 'closed'", &item).unwrap());
        assert!(ExpressionEvaluator::evaluate("priority == 'high'", &item).unwrap());
    }

    #[test]
    fn test_inequality() {
        let item = create_test_item();

        assert!(!ExpressionEvaluator::evaluate("status != 'open'", &item).unwrap());
        assert!(ExpressionEvaluator::evaluate("status != 'closed'", &item).unwrap());
    }

    #[test]
    fn test_or_logic() {
        let item = create_test_item();

        assert!(
            ExpressionEvaluator::evaluate("status == 'open' or status == 'closed'", &item).unwrap()
        );
        assert!(
            ExpressionEvaluator::evaluate("status == 'closed' or priority == 'high'", &item)
                .unwrap()
        );
        assert!(
            !ExpressionEvaluator::evaluate("status == 'closed' or priority == 'low'", &item)
                .unwrap()
        );
    }

    #[test]
    fn test_and_logic() {
        let item = create_test_item();

        assert!(
            ExpressionEvaluator::evaluate("status == 'open' and priority == 'high'", &item)
                .unwrap()
        );
        assert!(
            !ExpressionEvaluator::evaluate("status == 'open' and priority == 'low'", &item)
                .unwrap()
        );
    }

    #[test]
    fn test_in_list() {
        let item = create_test_item();

        assert!(
            ExpressionEvaluator::evaluate("priority in ['low', 'medium', 'high']", &item).unwrap()
        );
        assert!(!ExpressionEvaluator::evaluate("priority in ['low', 'medium']", &item).unwrap());
        assert!(ExpressionEvaluator::evaluate("status in ['open', 'closed']", &item).unwrap());
    }

    #[test]
    fn test_complex_expression() {
        let item = create_test_item();

        // This should fail because priority is 'high' but status is not 'complete' or 'verified'
        assert!(!ExpressionEvaluator::evaluate(
            "priority != 'high' or status == 'complete' or status == 'verified'",
            &item
        )
        .unwrap());

        // This should pass because priority is not 'critical'
        assert!(ExpressionEvaluator::evaluate(
            "priority != 'critical' or status == 'complete'",
            &item
        )
        .unwrap());
    }
}
