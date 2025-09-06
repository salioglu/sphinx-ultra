use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Type alias for document relations: (parent, previous, next)
type DocumentRelations = HashMap<String, (Option<String>, Option<String>, Option<String>)>;

/// Build environment that mirrors Sphinx's BuildEnvironment
#[derive(Debug, Clone)]
pub struct BuildEnvironment {
    pub config: crate::config::BuildConfig,
    pub domains: HashMap<String, Domain>,
    pub found_docs: Vec<String>,
    pub all_docs: HashMap<String, f64>, // docname -> mtime
    pub dependencies: HashMap<String, HashSet<PathBuf>>,
    pub included: HashMap<PathBuf, HashSet<String>>,
    pub temp_data: HashMap<String, serde_json::Value>,
    pub ref_context: HashMap<String, serde_json::Value>,
    pub toctree_includes: HashMap<String, Vec<String>>,
    pub files_to_rebuild: HashMap<String, HashSet<String>>,
    pub glob_toctrees: HashSet<String>,
    pub reread_always: HashSet<String>,
    pub metadata: HashMap<String, HashMap<String, String>>,
    pub titles: HashMap<String, String>,
    pub longtitles: HashMap<String, String>,
    pub tocs: HashMap<String, String>,
    pub toc_secnumbers: HashMap<String, HashMap<String, Vec<u32>>>,
    pub toc_fignumbers: HashMap<String, HashMap<String, HashMap<String, Vec<u32>>>>,
    pub toc_num_entries: HashMap<String, usize>,
    pub dlfiles: HashMap<String, (Option<String>, String)>,
    pub images: HashMap<String, String>,
}

use std::collections::HashSet;

impl BuildEnvironment {
    pub fn new(config: crate::config::BuildConfig) -> Self {
        Self {
            config,
            domains: HashMap::new(),
            found_docs: Vec::new(),
            all_docs: HashMap::new(),
            dependencies: HashMap::new(),
            included: HashMap::new(),
            temp_data: HashMap::new(),
            ref_context: HashMap::new(),
            toctree_includes: HashMap::new(),
            files_to_rebuild: HashMap::new(),
            glob_toctrees: HashSet::new(),
            reread_always: HashSet::new(),
            metadata: HashMap::new(),
            titles: HashMap::new(),
            longtitles: HashMap::new(),
            tocs: HashMap::new(),
            toc_secnumbers: HashMap::new(),
            toc_fignumbers: HashMap::new(),
            toc_num_entries: HashMap::new(),
            dlfiles: HashMap::new(),
            images: HashMap::new(),
        }
    }

    /// Add a document to the environment
    pub fn add_document(&mut self, docname: String, mtime: f64) {
        self.found_docs.push(docname.clone());
        self.all_docs.insert(docname, mtime);
    }

    /// Get document path from docname
    pub fn doc2path(&self, docname: &str) -> PathBuf {
        PathBuf::from(format!("{}.rst", docname))
    }

    /// Collect relations between documents
    pub fn collect_relations(
        &self,
    ) -> DocumentRelations {
        // TODO: Implement relation collection from toctree
        HashMap::new()
    }

    /// Check if document needs to be updated
    pub fn doc_needs_update(&self, docname: &str, source_path: &PathBuf) -> bool {
        // Check if document exists in environment
        if !self.all_docs.contains_key(docname) {
            return true;
        }

        // Check modification time
        if let Ok(metadata) = std::fs::metadata(source_path) {
            if let Ok(mtime) = metadata.modified() {
                let file_mtime = mtime
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs_f64();
                if let Some(&env_mtime) = self.all_docs.get(docname) {
                    return file_mtime > env_mtime;
                }
            }
        }

        true
    }

    /// Update domain object
    pub fn update_domain_object(
        &mut self,
        domain_name: &str,
        obj_type: &str,
        object: DomainObject,
    ) {
        let domain = self
            .domains
            .entry(domain_name.to_string())
            .or_insert_with(|| Domain::new(domain_name));
        domain.add_object(obj_type, object);
    }

    /// Get all objects from all domains
    pub fn get_all_objects(&self) -> Vec<&DomainObject> {
        let mut objects = Vec::new();
        for domain in self.domains.values() {
            objects.extend(domain.get_all_objects());
        }
        objects
    }
}

/// Domain represents a Sphinx domain (py, cpp, js, std, etc.)
#[derive(Debug, Clone)]
pub struct Domain {
    pub name: String,
    pub label: String,
    pub object_types: HashMap<String, ObjectType>,
    pub objects: HashMap<String, Vec<DomainObject>>,
    pub indices: Vec<DomainIndex>,
}

impl Domain {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            label: name.to_string(),
            object_types: HashMap::new(),
            objects: HashMap::new(),
            indices: Vec::new(),
        }
    }

    pub fn add_object(&mut self, obj_type: &str, object: DomainObject) {
        self.objects
            .entry(obj_type.to_string())
            .or_default()
            .push(object);
    }

    pub fn get_objects(&self) -> Vec<&DomainObject> {
        let mut all_objects = Vec::new();
        for objects in self.objects.values() {
            all_objects.extend(objects);
        }
        all_objects
    }

    pub fn get_all_objects(&self) -> Vec<&DomainObject> {
        self.get_objects()
    }

    pub fn get_objects_by_type(&self, obj_type: &str) -> Option<&Vec<DomainObject>> {
        self.objects.get(obj_type)
    }
}

/// Object type definition in a domain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectType {
    pub lname: String,
    pub roles: Vec<String>,
    pub attrs: HashMap<String, bool>,
}

/// Domain object (function, class, module, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainObject {
    pub name: String,
    pub display_name: Option<String>,
    pub object_type: String,
    pub docname: String,
    pub anchor: Option<String>,
    pub priority: i32,
    pub description: Option<String>,
    pub signature: Option<String>,
    pub deprecated: bool,
}

impl DomainObject {
    pub fn new(
        name: String,
        object_type: String,
        docname: String,
        anchor: Option<String>,
        priority: i32,
    ) -> Self {
        Self {
            name,
            display_name: None,
            object_type,
            docname,
            anchor,
            priority,
            description: None,
            signature: None,
            deprecated: false,
        }
    }

    pub fn with_display_name(mut self, display_name: String) -> Self {
        self.display_name = Some(display_name);
        self
    }

    pub fn with_description(mut self, description: String) -> Self {
        self.description = Some(description);
        self
    }

    pub fn with_signature(mut self, signature: String) -> Self {
        self.signature = Some(signature);
        self
    }

    pub fn with_deprecated(mut self, deprecated: bool) -> Self {
        self.deprecated = deprecated;
        self
    }
}

/// Domain index for generating index pages
#[derive(Debug, Clone)]
pub struct DomainIndex {
    pub name: String,
    pub localname: String,
    pub shortname: Option<String>,
    pub entries: Vec<IndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    pub subentries: Vec<IndexEntry>,
    pub uri: String,
    pub display_name: String,
    pub extra: Option<String>,
}

/// Create standard domains that mirror Sphinx's built-in domains
pub fn create_standard_domains() -> HashMap<String, Domain> {
    let mut domains = HashMap::new();

    // Python domain
    let mut py_domain = Domain::new("py");
    py_domain.object_types.insert(
        "module".to_string(),
        ObjectType {
            lname: "module".to_string(),
            roles: vec!["mod".to_string(), "obj".to_string()],
            attrs: HashMap::new(),
        },
    );
    py_domain.object_types.insert(
        "function".to_string(),
        ObjectType {
            lname: "function".to_string(),
            roles: vec!["func".to_string(), "obj".to_string()],
            attrs: HashMap::new(),
        },
    );
    py_domain.object_types.insert(
        "class".to_string(),
        ObjectType {
            lname: "class".to_string(),
            roles: vec!["class".to_string(), "obj".to_string()],
            attrs: HashMap::new(),
        },
    );
    py_domain.object_types.insert(
        "method".to_string(),
        ObjectType {
            lname: "method".to_string(),
            roles: vec!["meth".to_string(), "obj".to_string()],
            attrs: HashMap::new(),
        },
    );
    py_domain.object_types.insert(
        "attribute".to_string(),
        ObjectType {
            lname: "attribute".to_string(),
            roles: vec!["attr".to_string(), "obj".to_string()],
            attrs: HashMap::new(),
        },
    );
    py_domain.object_types.insert(
        "exception".to_string(),
        ObjectType {
            lname: "exception".to_string(),
            roles: vec!["exc".to_string(), "obj".to_string()],
            attrs: HashMap::new(),
        },
    );
    py_domain.object_types.insert(
        "data".to_string(),
        ObjectType {
            lname: "data".to_string(),
            roles: vec!["data".to_string(), "obj".to_string()],
            attrs: HashMap::new(),
        },
    );
    domains.insert("py".to_string(), py_domain);

    // C++ domain
    let mut cpp_domain = Domain::new("cpp");
    cpp_domain.object_types.insert(
        "class".to_string(),
        ObjectType {
            lname: "class".to_string(),
            roles: vec!["class".to_string()],
            attrs: HashMap::new(),
        },
    );
    cpp_domain.object_types.insert(
        "function".to_string(),
        ObjectType {
            lname: "function".to_string(),
            roles: vec!["func".to_string()],
            attrs: HashMap::new(),
        },
    );
    cpp_domain.object_types.insert(
        "type".to_string(),
        ObjectType {
            lname: "type".to_string(),
            roles: vec!["type".to_string()],
            attrs: HashMap::new(),
        },
    );
    domains.insert("cpp".to_string(), cpp_domain);

    // JavaScript domain
    let mut js_domain = Domain::new("js");
    js_domain.object_types.insert(
        "module".to_string(),
        ObjectType {
            lname: "module".to_string(),
            roles: vec!["mod".to_string()],
            attrs: HashMap::new(),
        },
    );
    js_domain.object_types.insert(
        "class".to_string(),
        ObjectType {
            lname: "class".to_string(),
            roles: vec!["class".to_string()],
            attrs: HashMap::new(),
        },
    );
    js_domain.object_types.insert(
        "function".to_string(),
        ObjectType {
            lname: "function".to_string(),
            roles: vec!["func".to_string()],
            attrs: HashMap::new(),
        },
    );
    js_domain.object_types.insert(
        "method".to_string(),
        ObjectType {
            lname: "method".to_string(),
            roles: vec!["meth".to_string()],
            attrs: HashMap::new(),
        },
    );
    js_domain.object_types.insert(
        "data".to_string(),
        ObjectType {
            lname: "data".to_string(),
            roles: vec!["data".to_string()],
            attrs: HashMap::new(),
        },
    );
    domains.insert("js".to_string(), js_domain);

    // Standard domain
    let mut std_domain = Domain::new("std");
    std_domain.object_types.insert(
        "doc".to_string(),
        ObjectType {
            lname: "document".to_string(),
            roles: vec!["doc".to_string()],
            attrs: HashMap::new(),
        },
    );
    std_domain.object_types.insert(
        "label".to_string(),
        ObjectType {
            lname: "label".to_string(),
            roles: vec!["ref".to_string()],
            attrs: HashMap::new(),
        },
    );
    std_domain.object_types.insert(
        "term".to_string(),
        ObjectType {
            lname: "term".to_string(),
            roles: vec!["term".to_string()],
            attrs: HashMap::new(),
        },
    );
    std_domain.object_types.insert(
        "cmdoption".to_string(),
        ObjectType {
            lname: "command line option".to_string(),
            roles: vec!["option".to_string()],
            attrs: HashMap::new(),
        },
    );
    std_domain.object_types.insert(
        "envvar".to_string(),
        ObjectType {
            lname: "environment variable".to_string(),
            roles: vec!["envvar".to_string()],
            attrs: HashMap::new(),
        },
    );
    domains.insert("std".to_string(), std_domain);

    domains
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_environment_creation() {
        let config = crate::config::BuildConfig::default();
        let env = BuildEnvironment::new(config);

        assert_eq!(env.found_docs.len(), 0);
        assert_eq!(env.all_docs.len(), 0);
        assert_eq!(env.domains.len(), 0);
    }

    #[test]
    fn test_domain_object_creation() {
        let obj = DomainObject::new(
            "test_function".to_string(),
            "function".to_string(),
            "test_module".to_string(),
            Some("test_function".to_string()),
            1,
        );

        assert_eq!(obj.name, "test_function");
        assert_eq!(obj.object_type, "function");
        assert_eq!(obj.docname, "test_module");
        assert_eq!(obj.anchor, Some("test_function".to_string()));
        assert_eq!(obj.priority, 1);
    }

    #[test]
    fn test_domain_creation() {
        let mut domain = Domain::new("py");

        let obj = DomainObject::new(
            "test_func".to_string(),
            "function".to_string(),
            "test".to_string(),
            None,
            1,
        );

        domain.add_object("function", obj);

        assert_eq!(domain.objects.len(), 1);
        assert_eq!(domain.get_objects().len(), 1);
    }

    #[test]
    fn test_standard_domains() {
        let domains = create_standard_domains();

        assert!(domains.contains_key("py"));
        assert!(domains.contains_key("cpp"));
        assert!(domains.contains_key("js"));
        assert!(domains.contains_key("std"));

        let py_domain = &domains["py"];
        assert!(py_domain.object_types.contains_key("module"));
        assert!(py_domain.object_types.contains_key("function"));
        assert!(py_domain.object_types.contains_key("class"));
    }
}
