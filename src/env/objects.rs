//! The domains' `get_objects()` (`domains/__init__.py:285`): every object a
//! domain exposes to the object inventory (`objects.inv`) and the search
//! index (`searchindex.js`), as the `(name, dispname, type, docname, anchor,
//! priority)` rows Sphinx yields — plus the two per-type lookups both
//! consumers share, the search priority (`ObjType.attrs['searchprio']`) and
//! the localized type name (`Domain.get_type_name`).
//!
//! One producer for both artifacts is the point: the inventory and the
//! search index can only ever disagree about an object if Sphinx's do.

use crate::env::numbers::clean_astext;
use crate::env::BuildEnvironment;
use crate::inventory::InvObject;

/// `ObjType(..., searchprio=...)` for the std domain's types
/// (`domains/std/__init__.py:725-734`). `ObjType` defaults `searchprio` to
/// 1 (`domains/__init__.py:53`), so every type the table does not mark
/// unsearchable — `confval`, `envvar`, `cmdoption`, and whatever
/// `add_object_type` registers — is 1.
pub fn std_search_priority(objtype: &str) -> i32 {
    match objtype {
        "term" | "token" | "label" | "doc" => -1,
        _ => 1,
    }
}

/// `Domain.get_type_name(otype)` (`domains/__init__.py:318-322`), as
/// `IndexBuilder.get_objects` calls it: the std domain overrides it to the
/// bare `ObjType.lname` (`domains/std/__init__.py:1359`); every other domain
/// prefixes its label, so a Python class is a `Python class`. A type the
/// domain has no `ObjType` for is named by its type string, which is what
/// `get_objects` falls back to.
pub fn object_type_name(domain: &str, objtype: &str) -> String {
    match domain {
        "std" => match objtype {
            "cmdoption" => "program option",
            "confval" => "configuration value",
            "envvar" => "environment variable",
            "term" => "glossary term",
            "token" => "grammar token",
            "label" => "reference label",
            "doc" => "document",
            other => other,
        }
        .to_string(),
        "py" => {
            let lname = match objtype {
                "function" | "data" | "class" | "exception" | "method" | "attribute"
                | "property" | "module" => objtype,
                "classmethod" => "class method",
                "staticmethod" => "static method",
                "type" => "type alias",
                _ => return objtype.to_string(),
            };
            format!("Python {lname}")
        }
        _ => objtype.to_string(),
    }
}

/// Both domains this build populates, in `env.domains.sorted()` order, each
/// sorted the way `sorted(domain.get_objects())` sorts its tuples.
pub fn domain_objects(env: &BuildEnvironment) -> Vec<(&'static str, Vec<InvObject>)> {
    vec![
        ("py", py_domain_objects(env)),
        ("std", std_domain_objects(env)),
    ]
}

/// `StandardDomain.get_objects()` (`domains/std/__init__.py:1283-1308`).
pub fn std_domain_objects(env: &BuildEnvironment) -> Vec<InvObject> {
    let mut objects = Vec::new();

    // "handle the special 'doc' reference here"
    for docname in env.all_docs.keys() {
        objects.push(InvObject {
            name: docname.clone(),
            objtype: "doc".to_string(),
            priority: -1,
            docname: docname.clone(),
            anchor: String::new(),
            dispname: env
                .titles
                .get(docname)
                .map(clean_astext)
                .unwrap_or_default(),
        });
    }

    for ((program, option), (docname, anchor)) in &env.std.progoptions {
        let fullname = match program {
            Some(program) => format!("{program}.{option}"),
            None => option.clone(),
        };
        objects.push(InvObject {
            name: fullname.clone(),
            objtype: "cmdoption".to_string(),
            priority: 1,
            docname: docname.clone(),
            anchor: anchor.clone(),
            dispname: fullname,
        });
    }

    for ((objtype, name), (docname, anchor)) in &env.std.objects {
        objects.push(InvObject {
            name: name.clone(),
            objtype: objtype.clone(),
            priority: std_search_priority(objtype),
            docname: docname.clone(),
            anchor: anchor.clone(),
            dispname: name.clone(),
        });
    }

    for (name, (docname, labelid, sectionname)) in &env.std.labels {
        objects.push(InvObject {
            name: name.clone(),
            objtype: "label".to_string(),
            priority: -1,
            docname: docname.clone(),
            anchor: labelid.clone(),
            dispname: sectionname.clone(),
        });
    }

    // "add anonymous-only labels as well"
    for (name, (docname, labelid)) in &env.std.anonlabels {
        if !env.std.labels.contains_key(name) {
            objects.push(InvObject {
                name: name.clone(),
                objtype: "label".to_string(),
                priority: -1,
                docname: docname.clone(),
                anchor: labelid.clone(),
                dispname: name.clone(),
            });
        }
    }

    sort_like_sphinx(&mut objects);
    objects
}

/// `PythonDomain.get_objects()` (`domains/python/__init__.py:1056-1065`):
/// modules come from the module registry at priority 0 and are skipped in
/// the object registry; aliased objects are not full-text searchable.
pub fn py_domain_objects(env: &BuildEnvironment) -> Vec<InvObject> {
    let mut objects = Vec::new();
    for (name, module) in &env.py.modules {
        objects.push(InvObject {
            name: name.clone(),
            objtype: "module".to_string(),
            priority: 0,
            docname: module.docname.clone(),
            anchor: module.node_id.clone(),
            dispname: name.clone(),
        });
    }
    for (name, object) in &env.py.objects {
        if object.objtype == "module" {
            continue;
        }
        objects.push(InvObject {
            name: name.clone(),
            objtype: object.objtype.clone(),
            priority: if object.aliased { -1 } else { 1 },
            docname: object.docname.clone(),
            anchor: object.node_id.clone(),
            dispname: name.clone(),
        });
    }
    sort_like_sphinx(&mut objects);
    objects
}

/// `sorted(domain.get_objects())`: tuple order over
/// `(name, dispname, type, docname, anchor, priority)`.
fn sort_like_sphinx(objects: &mut [InvObject]) {
    objects.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.dispname.cmp(&b.dispname))
            .then_with(|| a.objtype.cmp(&b.objtype))
            .then_with(|| a.docname.cmp(&b.docname))
            .then_with(|| a.anchor.cmp(&b.anchor))
            .then_with(|| a.priority.cmp(&b.priority))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn std_types_the_table_does_not_name_are_searchable() {
        assert_eq!(std_search_priority("term"), -1);
        assert_eq!(std_search_priority("doc"), -1);
        assert_eq!(std_search_priority("confval"), 1);
        assert_eq!(std_search_priority("envvar"), 1);
        assert_eq!(std_search_priority("cmdoption"), 1);
        assert_eq!(std_search_priority("custom-type"), 1);
    }

    #[test]
    fn type_names_follow_get_type_name() {
        assert_eq!(object_type_name("std", "envvar"), "environment variable");
        assert_eq!(object_type_name("std", "custom"), "custom");
        assert_eq!(object_type_name("py", "class"), "Python class");
        assert_eq!(object_type_name("py", "classmethod"), "Python class method");
        assert_eq!(object_type_name("py", "type"), "Python type alias");
        assert_eq!(object_type_name("py", "unknown"), "unknown");
    }

    #[test]
    fn modules_are_listed_once_from_the_module_registry() {
        use crate::env::py_domain::{PyModuleEntry, PyObjectEntry};

        let mut env = BuildEnvironment::default();
        env.py.note_module(
            "pkg",
            PyModuleEntry {
                docname: "api".to_string(),
                node_id: "module-pkg".to_string(),
                synopsis: String::new(),
                platform: String::new(),
                deprecated: false,
            },
        );
        env.py.note_object(
            "pkg",
            PyObjectEntry {
                docname: "api".to_string(),
                node_id: "module-pkg".to_string(),
                objtype: "module".to_string(),
                aliased: false,
            },
        );
        env.py.note_object(
            "pkg.run",
            PyObjectEntry {
                docname: "api".to_string(),
                node_id: "pkg.run".to_string(),
                objtype: "function".to_string(),
                aliased: false,
            },
        );
        let rows: Vec<(String, String, i32)> = py_domain_objects(&env)
            .into_iter()
            .map(|o| (o.name, o.objtype, o.priority))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("pkg".to_string(), "module".to_string(), 0),
                ("pkg.run".to_string(), "function".to_string(), 1),
            ]
        );
    }
}
