use crate::domains::{
    CrossReference, DomainObject, DomainValidator, ReferenceType, ReferenceValidationResult,
};
use crate::error::BuildError;
/// Python Domain Implementation
///
/// Handles Python-specific objects and references like :func:, :class:, :mod:, etc.
use std::collections::HashMap;

/// Python domain validator for Python-specific references
pub struct PythonDomain {
    /// Objects registered in this domain
    objects: HashMap<String, DomainObject>,
}

impl Default for PythonDomain {
    fn default() -> Self {
        Self::new()
    }
}

impl PythonDomain {
    /// Create a new Python domain
    pub fn new() -> Self {
        Self {
            objects: HashMap::new(),
        }
    }

    /// Register a Python function
    pub fn register_function(
        &mut self,
        name: String,
        qualified_name: String,
        signature: Option<String>,
        docstring: Option<String>,
        location: crate::domains::ReferenceLocation,
    ) -> Result<(), BuildError> {
        let object = DomainObject {
            id: format!("py:func:{}", qualified_name),
            name: name.clone(),
            object_type: "function".to_string(),
            domain: "python".to_string(),
            definition_location: location,
            qualified_name: qualified_name.clone(),
            metadata: HashMap::new(),
            signature,
            docstring,
        };

        self.register_object(object)
    }

    /// Register a Python class
    pub fn register_class(
        &mut self,
        name: String,
        qualified_name: String,
        docstring: Option<String>,
        location: crate::domains::ReferenceLocation,
    ) -> Result<(), BuildError> {
        let object = DomainObject {
            id: format!("py:class:{}", qualified_name),
            name: name.clone(),
            object_type: "class".to_string(),
            domain: "python".to_string(),
            definition_location: location,
            qualified_name: qualified_name.clone(),
            metadata: HashMap::new(),
            signature: None,
            docstring,
        };

        self.register_object(object)
    }

    /// Register a Python module
    pub fn register_module(
        &mut self,
        name: String,
        qualified_name: String,
        docstring: Option<String>,
        location: crate::domains::ReferenceLocation,
    ) -> Result<(), BuildError> {
        let object = DomainObject {
            id: format!("py:mod:{}", qualified_name),
            name: name.clone(),
            object_type: "module".to_string(),
            domain: "python".to_string(),
            definition_location: location,
            qualified_name: qualified_name.clone(),
            metadata: HashMap::new(),
            signature: None,
            docstring,
        };

        self.register_object(object)
    }

    /// Find suggestions for a broken reference
    fn find_suggestions(&self, target: &str) -> Vec<String> {
        let mut suggestions = Vec::new();

        // Look for similar names
        for obj in self.objects.values() {
            // Exact match on simple name or fuzzy match or qualified name match
            if obj.name == target
                || obj.name.contains(target)
                || target.contains(&obj.name)
                || obj.qualified_name.contains(target)
            {
                suggestions.push(obj.qualified_name.clone());
            }
        }

        suggestions.sort();
        suggestions.dedup();
        suggestions.truncate(5); // Limit to 5 suggestions

        suggestions
    }
}

impl DomainValidator for PythonDomain {
    fn domain_name(&self) -> &str {
        "python"
    }

    fn supported_reference_types(&self) -> Vec<ReferenceType> {
        vec![
            ReferenceType::Function,
            ReferenceType::Class,
            ReferenceType::Module,
            ReferenceType::Method,
            ReferenceType::Attribute,
            ReferenceType::Data,
            ReferenceType::Exception,
        ]
    }

    fn register_object(&mut self, object: DomainObject) -> Result<(), BuildError> {
        let key = object.qualified_name.clone();
        self.objects.insert(key, object);
        Ok(())
    }

    fn resolve_reference(&self, reference: &CrossReference) -> Option<DomainObject> {
        // Try exact qualified name match
        if let Some(obj) = self.objects.get(&reference.target) {
            return Some(obj.clone());
        }

        // Try simple name match
        for obj in self.objects.values() {
            if obj.name == reference.target {
                return Some(obj.clone());
            }
        }

        None
    }

    fn validate_reference(&self, reference: &CrossReference) -> ReferenceValidationResult {
        if let Some(target_object) = self.resolve_reference(reference) {
            ReferenceValidationResult {
                reference: reference.clone(),
                is_valid: true,
                target_object: Some(target_object),
                error_message: None,
                suggestions: Vec::new(),
            }
        } else {
            let suggestions = self.find_suggestions(&reference.target);
            let error_message = if suggestions.is_empty() {
                format!("Python object '{}' not found", reference.target)
            } else {
                format!(
                    "Python object '{}' not found. Did you mean: {}?",
                    reference.target,
                    suggestions.join(", ")
                )
            };

            ReferenceValidationResult {
                reference: reference.clone(),
                is_valid: false,
                target_object: None,
                error_message: Some(error_message),
                suggestions,
            }
        }
    }

    fn get_all_objects(&self) -> Vec<&DomainObject> {
        self.objects.values().collect()
    }

    fn clear_objects(&mut self) {
        self.objects.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::ReferenceLocation;

    fn create_test_location() -> ReferenceLocation {
        ReferenceLocation {
            docname: "test.rst".to_string(),
            lineno: Some(10),
            column: Some(5),
            source_path: Some("test.rst".to_string()),
        }
    }

    #[test]
    fn test_python_domain_creation() {
        let domain = PythonDomain::new();
        assert_eq!(domain.domain_name(), "python");
        assert!(domain
            .supported_reference_types()
            .contains(&ReferenceType::Function));
        assert!(domain
            .supported_reference_types()
            .contains(&ReferenceType::Class));
    }

    #[test]
    fn test_register_function() {
        let mut domain = PythonDomain::new();

        let result = domain.register_function(
            "test_func".to_string(),
            "module.test_func".to_string(),
            Some("test_func(x, y)".to_string()),
            Some("Test function".to_string()),
            create_test_location(),
        );

        assert!(result.is_ok());
        assert_eq!(domain.objects.len(), 1);

        let obj = domain.objects.get("module.test_func").unwrap();
        assert_eq!(obj.name, "test_func");
        assert_eq!(obj.object_type, "function");
        assert_eq!(obj.domain, "python");
    }

    #[test]
    fn test_reference_resolution() {
        let mut domain = PythonDomain::new();

        domain
            .register_function(
                "example".to_string(),
                "mymodule.example".to_string(),
                None,
                None,
                create_test_location(),
            )
            .unwrap();

        let reference = CrossReference {
            ref_type: ReferenceType::Function,
            target: "mymodule.example".to_string(),
            display_text: None,
            source_location: create_test_location(),
            is_external: false,
        };

        let resolved = domain.resolve_reference(&reference);
        assert!(resolved.is_some());

        let obj = resolved.unwrap();
        assert_eq!(obj.qualified_name, "mymodule.example");
        assert_eq!(obj.object_type, "function");
    }

    #[test]
    fn test_reference_validation() {
        let mut domain = PythonDomain::new();

        domain
            .register_class(
                "TestClass".to_string(),
                "module.TestClass".to_string(),
                None,
                create_test_location(),
            )
            .unwrap();

        // Valid reference
        let valid_ref = CrossReference {
            ref_type: ReferenceType::Class,
            target: "module.TestClass".to_string(),
            display_text: None,
            source_location: create_test_location(),
            is_external: false,
        };

        let result = domain.validate_reference(&valid_ref);
        assert!(result.is_valid);
        assert!(result.target_object.is_some());
        assert!(result.error_message.is_none());

        // Invalid reference
        let invalid_ref = CrossReference {
            ref_type: ReferenceType::Class,
            target: "nonexistent.Class".to_string(),
            display_text: None,
            source_location: create_test_location(),
            is_external: false,
        };

        let result = domain.validate_reference(&invalid_ref);
        assert!(!result.is_valid);
        assert!(result.target_object.is_none());
        assert!(result.error_message.is_some());
    }

    #[test]
    fn test_suggestions() {
        let mut domain = PythonDomain::new();

        domain
            .register_function(
                "similar_function".to_string(),
                "module.similar_function".to_string(),
                None,
                None,
                create_test_location(),
            )
            .unwrap();

        let reference = CrossReference {
            ref_type: ReferenceType::Function,
            target: "similar".to_string(),
            display_text: None,
            source_location: create_test_location(),
            is_external: false,
        };

        let result = domain.validate_reference(&reference);
        assert!(!result.is_valid);
        assert!(!result.suggestions.is_empty());
        assert!(result
            .suggestions
            .contains(&"module.similar_function".to_string()));
    }
}
