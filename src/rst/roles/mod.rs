//! Role dispatch: Sphinx's `sphinx_domains.role` over docutils'
//! `roles.role` ([RC §2.2, §7.1-7.2]).
//!
//! Sphinx replaces docutils' role lookup for the whole parse
//! (`SP/sphinx/util/docutils.py:348-382`). The name is lowercased; a
//! `domain:` prefix asks that domain, and an unknown domain logs
//! `unknown role name: X` with no location; a bare name asks the
//! document's default domain. The std domain then always gets the name
//! (the part after the colon, so `:foo:ref:` and `:py:ref:` are both
//! `std:ref`), and last comes docutils' own lookup
//! (`SP/docutils/parsers/rst/roles.py:105-156`): the local registry
//! (`_roles`, where Sphinx registers its own roles), then the English
//! alias table, then the canonical registry — with the INFO docutils
//! creates for a name the alias table does not hold. A miss is the
//! caller's `Unknown interpreted text role "X".` ERROR and `problematic`.
//! With `sphinx.ext.intersphinx` loaded, its dispatcher claims every
//! `external:`/`external+` name, as written, before any of this
//! (`SP/sphinx/ext/intersphinx/_resolve.py:350-366`).
//!
//! Docutils caches a canonical hit in `_roles` under the name it was
//! looked up by (`roles.py:152-155`), so a canonical name outside the
//! alias table creates its INFO only on first use in a build; the tables
//! here are immutable, and every use creates it. Only
//! `restructuredtext-unimplemented-role` is such a name, and an INFO
//! message never prints and never survives Sphinx's read
//! (`FilterSystemMessages`).

use std::collections::BTreeMap;
use std::sync::LazyLock;

use crate::doctree::kinds;
use crate::rst::LogLocation;
use crate::transforms::state::DocumentState;

/// A domain of a default Sphinx 9.1.0 html build (`env.domains`, [RC
/// §2.1]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DomainId {
    C,
    Changeset,
    Citation,
    Cpp,
    Index,
    Js,
    Math,
    Py,
    Rst,
    Std,
}

impl DomainId {
    const ALL: [DomainId; 10] = [
        DomainId::C,
        DomainId::Changeset,
        DomainId::Citation,
        DomainId::Cpp,
        DomainId::Index,
        DomainId::Js,
        DomainId::Math,
        DomainId::Py,
        DomainId::Rst,
        DomainId::Std,
    ];

    /// `env.domains.get(name)`: the domain registered under `name`
    /// (lowercase, as every lookup passes it), `None` for any other name.
    pub(crate) fn from_name(name: &str) -> Option<DomainId> {
        DomainId::ALL
            .into_iter()
            .find(|domain| domain.name() == name)
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            DomainId::C => "c",
            DomainId::Changeset => "changeset",
            DomainId::Citation => "citation",
            DomainId::Cpp => "cpp",
            DomainId::Index => "index",
            DomainId::Js => "js",
            DomainId::Math => "math",
            DomainId::Py => "py",
            DomainId::Rst => "rst",
            DomainId::Std => "std",
        }
    }

    /// `Domain.role(name)`: the name in the domain's `roles` table
    /// (probe P01, [RC §2.1]). The c/cpp expression roles and the js
    /// attributes are not modelled ([RC §7.6]): this crate gives every
    /// domain role the `XRefRole` anatomy.
    fn role(self, name: &str) -> Option<&'static str> {
        let roles: &'static [&'static str] = match self {
            DomainId::C => &[
                "data",
                "enum",
                "enumerator",
                "expr",
                "func",
                "macro",
                "member",
                "struct",
                "texpr",
                "type",
                "union",
                "var",
            ],
            DomainId::Cpp => &[
                "any",
                "class",
                "concept",
                "enum",
                "enumerator",
                "expr",
                "func",
                "member",
                "struct",
                "texpr",
                "type",
                "union",
                "var",
            ],
            DomainId::Js => &["attr", "class", "data", "func", "meth", "mod"],
            DomainId::Math => &["numref"],
            DomainId::Py => &[
                "attr", "class", "const", "data", "deco", "exc", "func", "meth", "mod", "obj",
                "type",
            ],
            DomainId::Rst => &["dir", "role"],
            DomainId::Std => &[
                "confval", "doc", "envvar", "keyword", "numref", "option", "ref", "term", "token",
            ],
            DomainId::Changeset | DomainId::Citation | DomainId::Index => &[],
        };
        roles.iter().copied().find(|role| *role == name)
    }
}

/// What a role name resolves to: the role function docutils or Sphinx
/// registered under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RoleFn {
    /// docutils `GenericRole`: the text in a node of this kind.
    Generic(&'static str),
    /// docutils `math_role`.
    Math,
    /// docutils `raw_role`, unusable without a `.. role::` format.
    Raw,
    /// docutils `pep_reference_role` and `rfc_reference_role`: a plain
    /// `reference` to the PEP or RFC.
    PepReference,
    RfcReference,
    /// docutils `code_role`, the canonical `code` outside Sphinx.
    DocutilsCode,
    /// docutils `unimplemented_role`.
    Unimplemented,
    /// Sphinx's `code_role`, which replaces docutils' canonical `code`
    /// (`SP/sphinx/roles.py:577-605`, `:643`).
    SphinxCode,
    /// Sphinx's `IndexRole` (`SP/sphinx/domains/index.py:97-117`).
    Index,
    /// Sphinx's `PEP`, `RFC`, `CVE` and `CWE` roles (`SP/sphinx/roles.py`).
    Pep,
    Rfc,
    Cve,
    Cwe,
    /// `AnyXRefRole`, registered without a domain.
    Any,
    /// The math domain's `eq`, registered without a domain
    /// (`SP/sphinx/domains/math.py:163`).
    Eq,
    /// A domain's role. Sphinx calls it with its full name,
    /// `domain:name`, whatever the document wrote
    /// (`SP/sphinx/domains/__init__.py:157-181`).
    Domain(DomainId, &'static str),
    /// A Sphinx role whose function T17a ports (`kbd`, `guilabel`, the
    /// generic docroles, `download`, …), by its registered name. Until
    /// then the parser gives it the std `pending_xref` it has always
    /// given it.
    Unported(&'static str),
    /// `IntersphinxRole`, for the `external:`/`external+` names.
    External,
}

/// docutils' role registries as a build sees them: the local registry
/// `_roles` (checked first, by lowercased name) and the canonical
/// registry, in which a Sphinx build replaces docutils' `code`.
#[derive(Clone, Debug)]
pub(crate) struct RoleTable {
    local: BTreeMap<String, RoleFn>,
    code: RoleFn,
}

impl RoleTable {
    /// The registries of a default Sphinx 9.1.0 build: the 23 local
    /// roles of `sphinx.roles`, `sphinx.domains.index` and
    /// `sphinx.domains.math` (probe P01, [RC §2.1]) and Sphinx's `code`.
    pub(crate) fn sphinx() -> RoleTable {
        let local = [
            ("abbr", RoleFn::Unported("abbr")),
            ("any", RoleFn::Any),
            ("command", RoleFn::Unported("command")),
            ("cve", RoleFn::Cve),
            ("cwe", RoleFn::Cwe),
            ("dfn", RoleFn::Unported("dfn")),
            ("download", RoleFn::Unported("download")),
            ("eq", RoleFn::Eq),
            ("file", RoleFn::Unported("file")),
            ("guilabel", RoleFn::Unported("guilabel")),
            ("index", RoleFn::Index),
            ("kbd", RoleFn::Unported("kbd")),
            ("mailheader", RoleFn::Unported("mailheader")),
            ("makevar", RoleFn::Unported("makevar")),
            ("manpage", RoleFn::Unported("manpage")),
            ("menuselection", RoleFn::Unported("menuselection")),
            ("mimetype", RoleFn::Unported("mimetype")),
            ("newsgroup", RoleFn::Unported("newsgroup")),
            ("pep", RoleFn::Pep),
            ("program", RoleFn::Unported("program")),
            ("regexp", RoleFn::Unported("regexp")),
            ("rfc", RoleFn::Rfc),
            ("samp", RoleFn::Unported("samp")),
        ]
        .into_iter()
        .map(|(name, role)| (name.to_string(), role))
        .collect();
        RoleTable {
            local,
            code: RoleFn::SphinxCode,
        }
    }

    /// docutils' own registries: no local role, docutils' `code`.
    pub(crate) fn docutils() -> RoleTable {
        RoleTable {
            local: BTreeMap::new(),
            code: RoleFn::DocutilsCode,
        }
    }

    /// The registries every parse in this mode starts from and keeps: no
    /// document adds a role yet (`.. role::` is T17a's).
    pub(crate) fn builtin(sphinx: bool) -> &'static RoleTable {
        static SPHINX: LazyLock<RoleTable> = LazyLock::new(RoleTable::sphinx);
        static DOCUTILS: LazyLock<RoleTable> = LazyLock::new(RoleTable::docutils);
        if sphinx {
            &SPHINX
        } else {
            &DOCUTILS
        }
    }

    /// docutils `_role_registry[canonical]`.
    fn canonical(&self, canonical: &str) -> Option<RoleFn> {
        Some(match canonical {
            "abbreviation" => RoleFn::Generic(kinds::ABBREVIATION),
            "acronym" => RoleFn::Generic(kinds::ACRONYM),
            "emphasis" => RoleFn::Generic(kinds::EMPHASIS),
            "literal" => RoleFn::Generic(kinds::LITERAL),
            "strong" => RoleFn::Generic(kinds::STRONG),
            "subscript" => RoleFn::Generic(kinds::SUBSCRIPT),
            "superscript" => RoleFn::Generic(kinds::SUPERSCRIPT),
            "title-reference" => RoleFn::Generic(kinds::TITLE_REFERENCE),
            "code" => self.code,
            "math" => RoleFn::Math,
            "pep-reference" => RoleFn::PepReference,
            "rfc-reference" => RoleFn::RfcReference,
            "raw" => RoleFn::Raw,
            "index"
            | "named-reference"
            | "anonymous-reference"
            | "uri-reference"
            | "footnote-reference"
            | "citation-reference"
            | "substitution-reference"
            | "target"
            | "restructuredtext-unimplemented-role" => RoleFn::Unimplemented,
            _ => return None,
        })
    }
}

/// docutils' English role aliases (`languages/en.py`, `roles`): the
/// canonical name of a language-dependent one.
fn english_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "abbreviation" | "ab" => "abbreviation",
        "acronym" | "ac" => "acronym",
        "code" => "code",
        "emphasis" => "emphasis",
        "literal" => "literal",
        "math" => "math",
        "pep-reference" | "pep" => "pep-reference",
        "rfc-reference" | "rfc" => "rfc-reference",
        "strong" => "strong",
        "subscript" | "sub" => "subscript",
        "superscript" | "sup" => "superscript",
        "title-reference" | "title" | "t" => "title-reference",
        "raw" => "raw",
        "index" | "i" => "index",
        "anonymous-reference" => "anonymous-reference",
        "citation-reference" => "citation-reference",
        "footnote-reference" => "footnote-reference",
        "named-reference" => "named-reference",
        "substitution-reference" => "substitution-reference",
        "uri-reference" | "uri" | "url" => "uri-reference",
        "target" => "target",
        _ => return None,
    })
}

/// What decides a lookup besides the name: the parser's mode and the
/// current document's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DispatchCtx {
    /// Sphinx mode: Sphinx's domain dispatch runs before docutils' lookup.
    pub(crate) sphinx: bool,
    /// `env.current_document.default_domain`: `primary_domain` until a
    /// `.. default-domain::` changes it.
    pub(crate) default_domain: Option<DomainId>,
    /// Whether `sphinx.ext.intersphinx` is loaded, which installs the
    /// dispatcher that claims the `external:` roles.
    pub(crate) intersphinx: bool,
}

/// The outcome of a lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Resolution {
    /// The role, the name it is called with and the INFO docutils' lookup
    /// created on the way, if any.
    Found {
        role: RoleFn,
        call_name: String,
        info: Option<String>,
    },
    /// No role: the INFO docutils' lookup created, then the caller's
    /// `Unknown interpreted text role` ERROR.
    NotFound { info: Option<String> },
}

/// Looks up the role a document wrote as `written` (the name between the
/// colons, as written), the way a Sphinx parse or a docutils parse does.
/// The `unknown role name: X` warning of an unknown domain prefix is
/// logged into `log` as a parse-time logger record with no location,
/// before the caller reports anything else.
pub(crate) fn resolve_role(
    table: &RoleTable,
    ctx: &DispatchCtx,
    written: &str,
    log: &mut DocumentState,
) -> Resolution {
    if !ctx.sphinx {
        return docutils_role(table, written, written);
    }
    if ctx.intersphinx && crate::intersphinx::is_external_role(written) {
        return Resolution::Found {
            role: RoleFn::External,
            call_name: written.to_string(),
            info: None,
        };
    }
    let lower = written.to_lowercase();
    let name = match lower.split_once(':') {
        Some((domain_name, name)) => {
            match DomainId::from_name(domain_name) {
                Some(domain) => {
                    if let Some(found) = domain_role(domain, name) {
                        return found;
                    }
                }
                None => log.log_warning(format!("unknown role name: {lower}"), LogLocation::None),
            }
            name
        }
        None => {
            if let Some(found) = ctx
                .default_domain
                .and_then(|domain| domain_role(domain, &lower))
            {
                return found;
            }
            lower.as_str()
        }
    };
    if let Some(found) = domain_role(DomainId::Std, name) {
        return found;
    }
    docutils_role(table, &lower, written)
}

/// `Domain.role(name)`, called with the domain's full name.
fn domain_role(domain: DomainId, name: &str) -> Option<Resolution> {
    let role = domain.role(name)?;
    Some(Resolution::Found {
        role: RoleFn::Domain(domain, role),
        call_name: format!("{}:{role}", domain.name()),
        info: None,
    })
}

/// docutils `roles.role(role_name, ...)` (`roles.py:105-156`), whose INFO
/// names `role_name` as it was given — lowercased by
/// `sphinx_domains.role` in a Sphinx parse. The role it finds is called
/// with the name the document wrote (`Inliner.interpreted`).
fn docutils_role(table: &RoleTable, role_name: &str, written: &str) -> Resolution {
    let normname = role_name.to_lowercase();
    if let Some(role) = table.local.get(&normname) {
        return Resolution::Found {
            role: *role,
            call_name: written.to_string(),
            info: None,
        };
    }
    // `if role_name: ... else: canonicalname = DEFAULT_INTERPRETED_ROLE`.
    let (canonical, info) = if role_name.is_empty() {
        ("title-reference", None)
    } else {
        match english_alias(&normname) {
            Some(canonical) => (canonical, None),
            None => (
                normname.as_str(),
                Some(format!(
                    "No role entry for \"{role_name}\" in module \
                     \"docutils.parsers.rst.languages.en\".\n\
                     Trying \"{role_name}\" as canonical role name."
                )),
            ),
        }
    };
    match table.canonical(canonical) {
        Some(role) => Resolution::Found {
            role,
            call_name: written.to_string(),
            info,
        },
        None => Resolution::NotFound { info },
    }
}

#[cfg(test)]
pub(crate) mod tests;
