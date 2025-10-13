use crate::domains::{
    CrossReference, DomainObject, DomainValidator, ReferenceType, ReferenceValidationResult,
};
use crate::error::BuildError;
/// RST Domain Implementation
///
/// Handles RST-specific objects and references like :doc:, :ref:, etc.
use std::collections::HashMap;

/// RST domain validator for document and section references
pub struct RstDomain {
    /// Documents registered in this domain
    documents: HashMap<String, DomainObject>,
    /// Sections registered in this domain
    sections: HashMap<String, DomainObject>,
}

impl Default for RstDomain {
    fn default() -> Self {
        Self::new()
    }
}

impl RstDomain {
    /// Create a new RST domain
    pub fn new() -> Self {
        Self {
            documents: HashMap::new(),
            sections: HashMap::new(),
        }
    }

    /// Register a document
    pub fn register_document(
        &mut self,
        docname: String,
        title: String,
        location: crate::domains::ReferenceLocation,
    ) -> Result<(), BuildError> {
        let object = DomainObject {
            id: format!("doc:{}", docname),
            name: docname.clone(),
            object_type: "document".to_string(),
            domain: "rst".to_string(),
            definition_location: location,
            qualified_name: docname.clone(),
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("title".to_string(), title);
                meta
            },
            signature: None,
            docstring: None,
        };

        self.documents.insert(docname, object);
        Ok(())
    }

    /// Register a section (for :ref: targets)
    pub fn register_section(
        &mut self,
        label: String,
        title: String,
        docname: String,
        location: crate::domains::ReferenceLocation,
    ) -> Result<(), BuildError> {
        let qualified_name = format!("{}#{}", docname, label);
        let object = DomainObject {
            id: format!("ref:{}", label),
            name: label.clone(),
            object_type: "section".to_string(),
            domain: "rst".to_string(),
            definition_location: location,
            qualified_name: qualified_name.clone(),
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("title".to_string(), title);
                meta.insert("docname".to_string(), docname);
                meta
            },
            signature: None,
            docstring: None,
        };

        self.sections.insert(label, object);
        Ok(())
    }

    /// Register a figure or table label
    pub fn register_label(
        &mut self,
        label: String,
        label_type: String, // "figure", "table", "code-block", etc.
        title: Option<String>,
        docname: String,
        location: crate::domains::ReferenceLocation,
    ) -> Result<(), BuildError> {
        let qualified_name = format!("{}#{}", docname, label);
        let object = DomainObject {
            id: format!("{}:{}", label_type, label),
            name: label.clone(),
            object_type: label_type,
            domain: "rst".to_string(),
            definition_location: location,
            qualified_name: qualified_name.clone(),
            metadata: {
                let mut meta = HashMap::new();
                if let Some(title) = title {
                    meta.insert("title".to_string(), title);
                }
                meta.insert("docname".to_string(), docname);
                meta
            },
            signature: None,
            docstring: None,
        };

        self.sections.insert(label, object);
        Ok(())
    }

    /// Find suggestions for a broken reference
    fn find_suggestions(&self, target: &str, ref_type: &ReferenceType) -> Vec<String> {
        let mut suggestions = Vec::new();

        let objects = match ref_type {
            ReferenceType::Document => &self.documents,
            ReferenceType::Section => &self.sections,
            _ => &self.sections, // Default to sections for other types
        };

        // Look for similar names
        for (key, obj) in objects {
            // Exact match on key
            if key.contains(target) || target.contains(key) {
                suggestions.push(key.clone());
            }
            // Title match
            if let Some(title) = obj.metadata.get("title") {
                if title.to_lowercase().contains(&target.to_lowercase()) {
                    suggestions.push(key.clone());
                }
            }
        }

        suggestions.sort();
        suggestions.dedup();
        suggestions.truncate(5); // Limit to 5 suggestions

        suggestions
    }
}

impl DomainValidator for RstDomain {
    fn domain_name(&self) -> &str {
        "rst"
    }

    fn supported_reference_types(&self) -> Vec<ReferenceType> {
        vec![
            ReferenceType::Document,
            ReferenceType::Section,
            ReferenceType::Custom("numref".to_string()),
        ]
    }

    fn register_object(&mut self, object: DomainObject) -> Result<(), BuildError> {
        match object.object_type.as_str() {
            "document" => {
                self.documents.insert(object.name.clone(), object);
            }
            "section" | "figure" | "table" | "code-block" => {
                self.sections.insert(object.name.clone(), object);
            }
            _ => {
                return Err(BuildError::ValidationError(format!(
                    "Unknown RST object type: {}",
                    object.object_type
                )));
            }
        }
        Ok(())
    }

    fn resolve_reference(&self, reference: &CrossReference) -> Option<DomainObject> {
        match reference.ref_type {
            ReferenceType::Document => {
                // For documents, target might include .rst extension or not
                let target = reference.target.trim_end_matches(".rst");
                self.documents
                    .get(target)
                    .cloned()
                    .or_else(|| self.documents.get(&reference.target).cloned())
            }
            ReferenceType::Section => self.sections.get(&reference.target).cloned(),
            _ => {
                // Try sections first, then documents
                self.sections
                    .get(&reference.target)
                    .cloned()
                    .or_else(|| self.documents.get(&reference.target).cloned())
            }
        }
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
            let suggestions = self.find_suggestions(&reference.target, &reference.ref_type);
            let error_message = match reference.ref_type {
                ReferenceType::Document => {
                    if suggestions.is_empty() {
                        format!("Document '{}' not found", reference.target)
                    } else {
                        format!(
                            "Document '{}' not found. Did you mean: {}?",
                            reference.target,
                            suggestions.join(", ")
                        )
                    }
                }
                ReferenceType::Section => {
                    if suggestions.is_empty() {
                        format!("Section label '{}' not found", reference.target)
                    } else {
                        format!(
                            "Section label '{}' not found. Did you mean: {}?",
                            reference.target,
                            suggestions.join(", ")
                        )
                    }
                }
                _ => {
                    if suggestions.is_empty() {
                        format!("Reference target '{}' not found", reference.target)
                    } else {
                        format!(
                            "Reference target '{}' not found. Did you mean: {}?",
                            reference.target,
                            suggestions.join(", ")
                        )
                    }
                }
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
        let mut objects = Vec::new();
        objects.extend(self.documents.values());
        objects.extend(self.sections.values());
        objects
    }

    fn clear_objects(&mut self) {
        self.documents.clear();
        self.sections.clear();
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
    fn test_rst_domain_creation() {
        let domain = RstDomain::new();
        assert_eq!(domain.domain_name(), "rst");
        assert!(domain
            .supported_reference_types()
            .contains(&ReferenceType::Document));
        assert!(domain
            .supported_reference_types()
            .contains(&ReferenceType::Section));
    }

    #[test]
    fn test_register_document() {
        let mut domain = RstDomain::new();

        let result = domain.register_document(
            "index".to_string(),
            "Home Page".to_string(),
            create_test_location(),
        );

        assert!(result.is_ok());
        assert_eq!(domain.documents.len(), 1);

        let doc = domain.documents.get("index").unwrap();
        assert_eq!(doc.name, "index");
        assert_eq!(doc.object_type, "document");
        assert_eq!(doc.metadata.get("title"), Some(&"Home Page".to_string()));
    }

    #[test]
    fn test_register_section() {
        let mut domain = RstDomain::new();

        let result = domain.register_section(
            "introduction".to_string(),
            "Introduction".to_string(),
            "index".to_string(),
            create_test_location(),
        );

        assert!(result.is_ok());
        assert_eq!(domain.sections.len(), 1);

        let section = domain.sections.get("introduction").unwrap();
        assert_eq!(section.name, "introduction");
        assert_eq!(section.object_type, "section");
        assert_eq!(
            section.metadata.get("title"),
            Some(&"Introduction".to_string())
        );
        assert_eq!(section.metadata.get("docname"), Some(&"index".to_string()));
    }

    #[test]
    fn test_document_reference_validation() {
        let mut domain = RstDomain::new();

        domain
            .register_document(
                "getting-started".to_string(),
                "Getting Started".to_string(),
                create_test_location(),
            )
            .unwrap();

        // Valid reference
        let valid_ref = CrossReference {
            ref_type: ReferenceType::Document,
            target: "getting-started".to_string(),
            display_text: None,
            source_location: create_test_location(),
            is_external: false,
        };

        let result = domain.validate_reference(&valid_ref);
        assert!(result.is_valid);
        assert!(result.target_object.is_some());

        // Valid reference with .rst extension
        let valid_ref_ext = CrossReference {
            ref_type: ReferenceType::Document,
            target: "getting-started.rst".to_string(),
            display_text: None,
            source_location: create_test_location(),
            is_external: false,
        };

        let result = domain.validate_reference(&valid_ref_ext);
        assert!(result.is_valid);

        // Invalid reference
        let invalid_ref = CrossReference {
            ref_type: ReferenceType::Document,
            target: "nonexistent".to_string(),
            display_text: None,
            source_location: create_test_location(),
            is_external: false,
        };

        let result = domain.validate_reference(&invalid_ref);
        assert!(!result.is_valid);
        assert!(result.error_message.is_some());
    }

    #[test]
    fn test_section_reference_validation() {
        let mut domain = RstDomain::new();

        domain
            .register_section(
                "api-reference".to_string(),
                "API Reference".to_string(),
                "api".to_string(),
                create_test_location(),
            )
            .unwrap();

        // Valid reference
        let valid_ref = CrossReference {
            ref_type: ReferenceType::Section,
            target: "api-reference".to_string(),
            display_text: None,
            source_location: create_test_location(),
            is_external: false,
        };

        let result = domain.validate_reference(&valid_ref);
        assert!(result.is_valid);
        assert!(result.target_object.is_some());

        // Invalid reference with suggestions
        let invalid_ref = CrossReference {
            ref_type: ReferenceType::Section,
            target: "api".to_string(), // Close but not exact
            display_text: None,
            source_location: create_test_location(),
            is_external: false,
        };

        let result = domain.validate_reference(&invalid_ref);
        assert!(!result.is_valid);
        assert!(!result.suggestions.is_empty());
    }
}
