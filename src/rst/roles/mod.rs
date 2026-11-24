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
//! The local registry is build-scoped ([`RoleTable`], decision 14):
//! docutils keeps `_roles` for the whole process, so `.. role::` and a
//! `.. default-role::` while `default_role` is unset reach every later
//! document of the build. What else docutils caches there is not kept:
//! a canonical hit is cached under the name it was looked up by
//! (`roles.py:152-155`), so a canonical name outside the alias table
//! creates its INFO only on first use in a build, where here every use
//! creates it. Only `restructuredtext-unimplemented-role` is such a name,
//! and an INFO message never prints and never survives Sphinx's read
//! (`FilterSystemMessages`); keeping the cache would only make every
//! document that uses a docutils role change the table.
//!
//! The role functions live beside the dispatch: docutils' in
//! [`docutils`], Sphinx's in [`sphinx`], `ReferenceRole`'s title split in
//! [`reference`] and the `role` directive's custom roles in [`custom`].

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

use crate::doctree::kinds;
use crate::rst::LogLocation;
use crate::transforms::state::DocumentState;

pub(crate) mod custom;
pub(crate) mod docutils;
pub(crate) mod reference;
pub(crate) mod sphinx;

pub(crate) use custom::{CustomRole, RoleOptions};

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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// One of Sphinx's `generic_docroles`, by its registered name:
    /// `CustomRole(name, GenericRole(name, nodeclass), {'classes':
    /// [name]})` (`SP/sphinx/roles.py:28-37`, `:633-636`).
    SphinxGeneric(&'static str),
    /// Sphinx's `Keyboard` (`SP/sphinx/roles.py:478-528`).
    Kbd,
    /// Sphinx's `GUILabel` and `MenuSelection` (`:387-410`).
    GuiLabel,
    MenuSelection,
    /// Sphinx's `EmphasizedLiteral`, registered as `file` and `samp`
    /// (`:413-460`).
    EmphasizedLiteral,
    /// Sphinx's `Abbreviation` (`:463-475`).
    Abbr,
    /// Sphinx's `Manpage` (`:531-550`).
    Manpage,
    /// `XRefRole(nodeclass=addnodes.download_reference)`, registered
    /// without a domain (`:610`).
    Download,
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
    /// `IntersphinxRole(orig_name)`, for the `external:`/`external+`
    /// names: `orig_name` is the name the dispatcher was given, as the
    /// document wrote it, and the role reads its inventory, domain and
    /// role names from it, case kept, not from the name it is called by
    /// (`SP/sphinx/ext/intersphinx/_resolve.py:357-380`).
    External(String),
    /// docutils `generic_custom_role`: what `.. role:: name` with no base
    /// registers (`SP/docutils/parsers/rst/roles.py:246-253`).
    GenericCustom,
    /// docutils `CustomRole`: what `.. role:: name(base)` registers
    /// ([`CustomRole`]).
    Custom(Arc<CustomRole>),
}

impl RoleFn {
    /// Whether the role is a `SphinxRole`, whose `__call__` takes its name
    /// from the document's default role when it is called as the default
    /// role, and raises `SphinxError('cannot determine default role!')`
    /// when the document has none (`SP/sphinx/util/docutils.py:650-659`).
    /// A domain role is one too, but its adapter always calls it with its
    /// full name.
    pub(crate) fn is_sphinx_role(&self) -> bool {
        matches!(
            self,
            RoleFn::Kbd
                | RoleFn::GuiLabel
                | RoleFn::MenuSelection
                | RoleFn::EmphasizedLiteral
                | RoleFn::Abbr
                | RoleFn::Manpage
                | RoleFn::Download
                | RoleFn::Index
                | RoleFn::Pep
                | RoleFn::Rfc
                | RoleFn::Cve
                | RoleFn::Cwe
                | RoleFn::Any
                | RoleFn::Eq
                | RoleFn::External(_)
        )
    }
}

/// One call of a role function, `role_fn(name, rawtext, text, lineno,
/// inliner, options)` (`SP/docutils/parsers/rst/states.py:996-1007`).
pub(crate) struct RoleCall<'a> {
    /// The name the role receives: for a Sphinx role, the one
    /// `SphinxRole.__call__` settles on (the name written, lowercased, or
    /// the document's default role); for any other, the name written
    /// (`domain:name` for a domain's role, empty for the default role).
    pub(crate) name: &'a str,
    /// `rawtext`: the whole interpreted text construct, role included,
    /// with its backslashes restored (`unescape(rawsource, True)`).
    pub(crate) rawtext: &'a str,
    /// The interpreted text, with docutils' null escapes.
    pub(crate) escaped: &'a str,
    /// The options a custom role passes ([`RoleOptions`]).
    pub(crate) options: &'a RoleOptions,
    pub(crate) span: crate::doctree::Span,
}

/// docutils' role registries as a build sees them: the local registry
/// `_roles` (checked first, by lowercased name), which the `role` and
/// `default-role` directives add to and a document's `default_role`
/// fills for the document's parse, and the canonical registry, in which a
/// Sphinx build replaces docutils' `code`.
///
/// Docutils keeps `_roles` for the whole process, and Sphinx reads the
/// documents of a build in sorted docname order, so what a document adds
/// stays for every later one (decision 14): a parse starts from the
/// table the build hands it ([`crate::rst::ParseOptions::roles`]) and
/// leaves the one the next document starts from
/// ([`crate::rst::ParseOutput::roles_after`]). Two tables are equal when
/// they register the same roles under the same names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleTable {
    local: BTreeMap<String, RoleFn>,
    code: RoleFn,
}

impl Default for RoleTable {
    /// docutils' own registries ([`Self::docutils`]).
    fn default() -> Self {
        RoleTable::docutils()
    }
}

impl RoleTable {
    /// The registries of a default Sphinx 9.1.0 build: the 23 local
    /// roles of `sphinx.roles`, `sphinx.domains.index` and
    /// `sphinx.domains.math` (probe P01, [RC §2.1]) and Sphinx's `code`.
    pub(crate) fn sphinx() -> RoleTable {
        let generic = |name: &'static str| (name, RoleFn::SphinxGeneric(name));
        let local = [
            ("abbr", RoleFn::Abbr),
            ("any", RoleFn::Any),
            generic("command"),
            ("cve", RoleFn::Cve),
            ("cwe", RoleFn::Cwe),
            generic("dfn"),
            ("download", RoleFn::Download),
            ("eq", RoleFn::Eq),
            ("file", RoleFn::EmphasizedLiteral),
            ("guilabel", RoleFn::GuiLabel),
            ("index", RoleFn::Index),
            ("kbd", RoleFn::Kbd),
            generic("mailheader"),
            generic("makevar"),
            ("manpage", RoleFn::Manpage),
            ("menuselection", RoleFn::MenuSelection),
            generic("mimetype"),
            generic("newsgroup"),
            ("pep", RoleFn::Pep),
            generic("program"),
            generic("regexp"),
            ("rfc", RoleFn::Rfc),
            ("samp", RoleFn::EmphasizedLiteral),
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

    /// The registries a build in this mode starts from: Sphinx's or
    /// docutils' ([`Self::sphinx`], [`Self::docutils`]), shared.
    pub(crate) fn builtin(sphinx: bool) -> Arc<RoleTable> {
        static SPHINX: LazyLock<Arc<RoleTable>> = LazyLock::new(|| Arc::new(RoleTable::sphinx()));
        static DOCUTILS: LazyLock<Arc<RoleTable>> =
            LazyLock::new(|| Arc::new(RoleTable::docutils()));
        Arc::clone(if sphinx { &SPHINX } else { &DOCUTILS })
    }

    /// docutils `register_local_role(name, role)`: `_roles[name.lower()]`.
    pub(crate) fn register_local(&mut self, name: &str, role: RoleFn) {
        self.local.insert(name.to_lowercase(), role);
    }

    /// The default role, `_roles['']`: registered (`Some`) or removed
    /// (`None`, docutils' `title-reference` fallback again).
    pub(crate) fn set_default(&mut self, role: Option<RoleFn>) {
        match role {
            Some(role) => {
                self.local.insert(String::new(), role);
            }
            None => {
                self.local.remove("");
            }
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
            "code" => self.code.clone(),
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

/// An input Sphinx 9.1.0 crashes on (decision 13): the build fails, with
/// exit status 2 and a report naming the exception Sphinx raises, where
/// Sphinx raises it: while it reads the configuration, while it reads a
/// document (the parse, the read transforms), or while it resolves one.
/// Raised in a document's read, it is kept as the document's first crash
/// ([`crate::transforms::state::DocumentState::crash`]), with its place
/// among the document's records ([`Self::at`]), and the build reports the
/// crash of the first document in Sphinx's read order, after the records
/// Sphinx prints before it.
///
/// One outcome for every crash kind: the exception's class as Sphinx's
/// traceback names it and its message, never Sphinx's whole report (its
/// version table and traceback cannot be reproduced).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SphinxCrash {
    /// The exception's class as the last line of Sphinx's traceback names
    /// it: `IndexError`, `ValueError`, `sphinx.errors.SphinxError`.
    pub exception: &'static str,
    /// `SphinxError.category` for one of Sphinx's own errors (`Sphinx
    /// error`, `Extension error (sphinx.config)`), which Sphinx's report
    /// opens with, followed by `!`; `None` for a Python exception, whose
    /// report opens with nothing.
    pub category: Option<&'static str>,
    /// The exception's message.
    pub message: String,
    /// Where the build was when it crashed: the document's source, and
    /// the line of the input that crashes it. Rendered by the build,
    /// which knows the source directory ([`Self::located`]).
    pub location: LogLocation,
    /// The rendered location, once the build has filled it in or the
    /// crash was raised where the location is rendered
    /// ([`Self::rendered_at`]).
    pub rendered_location: Option<String>,
    /// Where a crash in a document's read stands among the document's
    /// records ([`crate::diagnostics::OrderKey`]): the parse's `(0, 0)`
    /// or the raising transform's row, and the seq the next record would
    /// have drawn. Sphinx logged the records keyed before it, and none
    /// after.
    pub at: crate::diagnostics::OrderKey,
}

impl SphinxCrash {
    /// A crash raised as a Python exception (no category line).
    pub(crate) fn python(exception: &'static str, message: String, location: LogLocation) -> Self {
        SphinxCrash {
            exception,
            category: None,
            message,
            location,
            rendered_location: None,
            at: crate::diagnostics::OrderKey::default(),
        }
    }

    /// `SphinxError(message)`, whose category is `Sphinx error`.
    pub(crate) fn sphinx_error(message: &str, location: LogLocation) -> Self {
        SphinxCrash {
            category: Some("Sphinx error"),
            ..SphinxCrash::python("sphinx.errors.SphinxError", message.to_string(), location)
        }
    }

    /// An `ExtensionError` Sphinx's event manager raises for a handler of
    /// one of its own modules that raised `exception` (`EventManager.emit`,
    /// `SP/sphinx/events.py:430-452`): its category names the module,
    /// `Extension error (sphinx.config)`, and its message the handler, the
    /// event and the exception. Python names the handler with its address
    /// in memory, which no build can repeat; the address is elided as
    /// `0x...`.
    pub(crate) fn extension_error(
        category: &'static str,
        handler: &str,
        event: &str,
        exception: &str,
    ) -> Self {
        SphinxCrash {
            category: Some(category),
            ..SphinxCrash::python(
                "sphinx.errors.ExtensionError",
                format!(
                    "Handler <function {handler} at 0x...> for event '{event}' threw an \
                     exception (exception: {exception})"
                ),
                LogLocation::None,
            )
        }
    }

    /// The crash, located at `location`, rendered the way a warning's is:
    /// for a crash raised where its location is already rendered (a read
    /// transform's node, a reference the resolve pass reaches).
    pub(crate) fn rendered_at(mut self, location: crate::diagnostics::Location) -> SphinxCrash {
        use crate::diagnostics::Location;
        self.rendered_location = match location {
            Location::None => None,
            Location::Verbatim(location) => Some(location.trim_end_matches(':').to_string()),
            Location::Path { path, line: None } => Some(path),
            Location::Path {
                path,
                line: Some(line),
            } => Some(format!("{path}:{line}")),
        };
        self
    }

    /// The crash, its location rendered the way a warning's is, against
    /// the document's source table and the project's source directory
    /// ([`LogLocation::render`]), unless it was rendered where the crash
    /// was raised.
    pub fn located(
        self,
        sources: &[String],
        srcdir: &std::path::Path,
        docname: &str,
    ) -> SphinxCrash {
        if self.rendered_location.is_some() {
            return self;
        }
        let location = self.location.render(sources, srcdir, docname);
        self.rendered_at(location)
    }

    /// The report the build prints, to stderr and the `-w` file, before
    /// it exits 2: what Sphinx's report (`handle_exception`,
    /// `SP/sphinx/_cli/util/errors.py:162-245`) opens with, the category
    /// line of a `SphinxError`, then the last line of its traceback, the
    /// exception and its message (the bare exception for one without a
    /// message), indented as Sphinx indents it; then where the input is.
    pub fn report(&self) -> String {
        let category = self
            .category
            .map(|category| format!("{category}!\n"))
            .unwrap_or_default();
        let location = self
            .rendered_location
            .as_deref()
            .map(|location| format!("{location}: "))
            .unwrap_or_default();
        format!(
            "\n{category}\n    {self}\n\n{location}Sphinx 9.1.0 crashes on this input; \
             sphinx-ultra stops the build as it does.\n"
        )
    }
}

impl std::fmt::Display for SphinxCrash {
    /// The last line of Python's traceback: `Exception: message`, or the
    /// bare `Exception` when the message is empty (an `assert` with none).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.message.is_empty() {
            write!(f, "{}", self.exception)
        } else {
            write!(f, "{}: {}", self.exception, self.message)
        }
    }
}

impl std::error::Error for SphinxCrash {}

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
            role: RoleFn::External(written.to_string()),
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
            role: role.clone(),
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
