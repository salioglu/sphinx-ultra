pub mod parser;
/// Domain System & Cross-Reference Validation
///
/// This module implements a domain-based validation system inspired by Sphinx domains.
/// It provides:
/// - Domain registration and management
/// - Cross-reference tracking and validation
/// - Domain object registry (functions, classes, modules, etc.)
/// - Reference resolution and dangling reference detection
pub mod python;
pub mod rst;

use crate::error::BuildError;
use std::collections::HashMap;

/// Represents the type of a cross-reference
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReferenceType {
    /// Document reference (:doc:)
    Document,
    /// Section reference (:ref:)
    Section,
    /// Function reference (:func:)
    Function,
    /// Class reference (:class:)
    Class,
    /// Module reference (:mod:)
    Module,
    /// Method reference (:meth:)
    Method,
    /// Attribute reference (:attr:)
    Attribute,
    /// Data reference (:data:)
    Data,
    /// Exception reference (:exc:)
    Exception,
    /// Custom reference type
    Custom(String),
}

impl std::fmt::Display for ReferenceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReferenceType::Document => write!(f, "Document"),
            ReferenceType::Section => write!(f, "Section"),
            ReferenceType::Function => write!(f, "Function"),
            ReferenceType::Class => write!(f, "Class"),
            ReferenceType::Module => write!(f, "Module"),
            ReferenceType::Method => write!(f, "Method"),
            ReferenceType::Attribute => write!(f, "Attribute"),
            ReferenceType::Data => write!(f, "Data"),
            ReferenceType::Exception => write!(f, "Exception"),
            ReferenceType::Custom(name) => write!(f, "{}", name),
        }
    }
}

/// Represents a cross-reference found in documentation
#[derive(Debug, Clone)]
pub struct CrossReference {
    /// The type of reference
    pub ref_type: ReferenceType,
    /// The target being referenced
    pub target: String,
    /// Optional display text (different from target)
    pub display_text: Option<String>,
    /// Location where the reference was found
    pub source_location: ReferenceLocation,
    /// Whether this reference is external (outside current project)
    pub is_external: bool,
}

/// Location information for a reference
#[derive(Debug, Clone)]
pub struct ReferenceLocation {
    /// Document name where reference appears
    pub docname: String,
    /// Line number in source file
    pub lineno: Option<usize>,
    /// Character position in line
    pub column: Option<usize>,
    /// Source file path
    pub source_path: Option<String>,
}

/// Represents an object within a domain (function, class, etc.)
#[derive(Debug, Clone)]
pub struct DomainObject {
    /// Unique identifier for the object
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Type of object
    pub object_type: String,
    /// Domain this object belongs to
    pub domain: String,
    /// Location where object is defined
    pub definition_location: ReferenceLocation,
    /// Full qualified name (e.g., module.class.method)
    pub qualified_name: String,
    /// Additional metadata specific to object type
    pub metadata: HashMap<String, String>,
    /// Signature for functions/methods
    pub signature: Option<String>,
    /// Documentation string
    pub docstring: Option<String>,
}

/// Result of reference validation
#[derive(Debug, Clone)]
pub struct ReferenceValidationResult {
    /// The original reference
    pub reference: CrossReference,
    /// Whether the reference is valid
    pub is_valid: bool,
    /// Target object if reference resolves
    pub target_object: Option<DomainObject>,
    /// Error message if validation failed
    pub error_message: Option<String>,
    /// Suggestions for fixing broken reference
    pub suggestions: Vec<String>,
}

/// Domain validation statistics
#[derive(Debug, Default)]
pub struct DomainValidationStats {
    /// Total references processed
    pub total_references: usize,
    /// Valid references
    pub valid_references: usize,
    /// Invalid/broken references
    pub broken_references: usize,
    /// External references (not validated)
    pub external_references: usize,
    /// References by type
    pub references_by_type: HashMap<ReferenceType, usize>,
}

/// Core trait for domain validation
pub trait DomainValidator {
    /// Get the name of this domain
    fn domain_name(&self) -> &str;

    /// Get the reference types this domain handles
    fn supported_reference_types(&self) -> Vec<ReferenceType>;

    /// Register a domain object
    fn register_object(&mut self, object: DomainObject) -> Result<(), BuildError>;

    /// Resolve a reference to a domain object
    fn resolve_reference(&self, reference: &CrossReference) -> Option<DomainObject>;

    /// Validate a cross-reference
    fn validate_reference(&self, reference: &CrossReference) -> ReferenceValidationResult;

    /// Get all objects in this domain
    fn get_all_objects(&self) -> Vec<&DomainObject>;

    /// Clear all registered objects (for rebuilds)
    fn clear_objects(&mut self);
}

/// Registry for managing multiple domains
pub struct DomainRegistry {
    /// Map of domain name to validator
    domains: HashMap<String, Box<dyn DomainValidator>>,
    /// All cross-references found during parsing
    cross_references: Vec<CrossReference>,
    /// Global object registry across all domains
    global_objects: HashMap<String, DomainObject>,
}

impl Default for DomainRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DomainRegistry {
    /// Create a new domain registry
    pub fn new() -> Self {
        Self {
            domains: HashMap::new(),
            cross_references: Vec::new(),
            global_objects: HashMap::new(),
        }
    }

    /// Register a domain validator
    pub fn register_domain(
        &mut self,
        validator: Box<dyn DomainValidator>,
    ) -> Result<(), BuildError> {
        let domain_name = validator.domain_name().to_string();

        if self.domains.contains_key(&domain_name) {
            return Err(BuildError::ValidationError(format!(
                "Domain '{}' is already registered",
                domain_name
            )));
        }

        self.domains.insert(domain_name, validator);
        Ok(())
    }

    /// Add a cross-reference for later validation
    pub fn add_cross_reference(&mut self, reference: CrossReference) {
        self.cross_references.push(reference);
    }

    /// Register an object in the global registry and appropriate domain
    pub fn register_object(&mut self, object: DomainObject) -> Result<(), BuildError> {
        // Add to global registry
        let key = format!("{}:{}", object.domain, object.qualified_name);
        self.global_objects.insert(key, object.clone());

        // Add to domain-specific registry
        if let Some(domain) = self.domains.get_mut(&object.domain) {
            domain.register_object(object)?;
        } else {
            return Err(BuildError::ValidationError(format!(
                "Domain '{}' not found for object '{}'",
                object.domain, object.name
            )));
        }

        Ok(())
    }

    /// Validate all cross-references
    pub fn validate_all_references(&self) -> Vec<ReferenceValidationResult> {
        let mut results = Vec::new();

        for reference in &self.cross_references {
            let result = self.validate_reference(reference);
            results.push(result);
        }

        results
    }

    /// Validate a single cross-reference
    pub fn validate_reference(&self, reference: &CrossReference) -> ReferenceValidationResult {
        // Skip external references
        if reference.is_external {
            return ReferenceValidationResult {
                reference: reference.clone(),
                is_valid: true, // Assume external refs are valid
                target_object: None,
                error_message: None,
                suggestions: Vec::new(),
            };
        }

        // Find appropriate domain validator
        for domain in self.domains.values() {
            if domain
                .supported_reference_types()
                .contains(&reference.ref_type)
            {
                return domain.validate_reference(reference);
            }
        }

        // No domain found for this reference type
        ReferenceValidationResult {
            reference: reference.clone(),
            is_valid: false,
            target_object: None,
            error_message: Some(format!(
                "No domain validator found for reference type {:?}",
                reference.ref_type
            )),
            suggestions: Vec::new(),
        }
    }

    /// Get validation statistics
    pub fn get_validation_stats(&self) -> DomainValidationStats {
        let results = self.validate_all_references();
        let mut stats = DomainValidationStats {
            total_references: results.len(),
            ..Default::default()
        };

        for result in results {
            if result.reference.is_external {
                stats.external_references += 1;
            } else if result.is_valid {
                stats.valid_references += 1;
            } else {
                stats.broken_references += 1;
            }

            *stats
                .references_by_type
                .entry(result.reference.ref_type.clone())
                .or_insert(0) += 1;
        }

        stats
    }

    /// Get all broken references
    pub fn get_broken_references(&self) -> Vec<ReferenceValidationResult> {
        self.validate_all_references()
            .into_iter()
            .filter(|r| !r.is_valid && !r.reference.is_external)
            .collect()
    }

    /// Clear all data (for rebuilds)
    pub fn clear(&mut self) {
        self.cross_references.clear();
        self.global_objects.clear();

        for domain in self.domains.values_mut() {
            domain.clear_objects();
        }
    }

    /// Get object by qualified name across all domains
    pub fn get_object(&self, qualified_name: &str) -> Option<&DomainObject> {
        // Try exact match first
        if let Some(obj) = self
            .global_objects
            .values()
            .find(|o| o.qualified_name == qualified_name)
        {
            return Some(obj);
        }

        // Try name match without domain prefix
        self.global_objects
            .values()
            .find(|o| o.name == qualified_name)
    }

    /// Search for objects by name pattern
    pub fn search_objects(&self, pattern: &str) -> Vec<&DomainObject> {
        self.global_objects
            .values()
            .filter(|obj| obj.name.contains(pattern) || obj.qualified_name.contains(pattern))
            .collect()
    }
}
