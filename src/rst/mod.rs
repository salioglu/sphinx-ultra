//! Recursive-descent RST parser with docutils-0.22.4 fidelity (M2 wave 1:
//! block grammar only — the inline parser arrives in wave 2).
//!
//! Fidelity contract: output `pformat()` is byte-identical to
//! `docutils.parsers.rst.Parser` parse-layer output for the construct set in
//! `tests/fixtures/doctree_differential.json`. Transforms (doctitle
//! promotion, target propagation, transition hoisting, message filtering)
//! are explicitly NOT applied here; they arrive as separate components in
//! later waves. Behavior sources: the committed differential fixture and the
//! probe notes in docs/plans/2026-08-07-m2-wave1-probes.md.

pub(crate) mod block;
pub(crate) mod digits;
pub mod inline;
pub mod lines;
mod punctuation;
pub(crate) mod pygments_aliases;
pub(crate) mod roles;

use std::collections::{HashMap, HashSet};

use crate::doctree::ids::DupnameFixup;
use crate::doctree::Doctree;
use crate::transforms::state::DocumentState;

#[derive(Debug, Clone)]
pub struct ParseOptions {
    /// What `<document source="...">` prints (docutils `new_document` name).
    pub source_path: String,
    /// Sphinx mode: the Sphinx directive/role registries extend the
    /// docutils-native ones (toctree, xref roles, ...). The binary build
    /// path runs with this on; the docutils differential fixture off.
    pub sphinx: bool,
    /// The docname recorded on pending_xref nodes (sphinx `refdoc`).
    pub docname: String,
    /// Every docname the project discovered (sphinx `env.found_docs`).
    /// The `toctree` directive resolves its entries against this set at
    /// parse time, exactly as Sphinx's `TocTree.parse_content` does.
    ///
    /// `None` means "parsed without an environment" — a standalone parse
    /// (the differential harnesses, `parse_rst` callers) where no document
    /// exists, so every toctree entry resolves to nothing and `entries`/
    /// `includefiles` stay empty. Shared by `Arc` because the build clones
    /// these options once per source file.
    pub found_docs: Option<std::sync::Arc<std::collections::BTreeSet<String>>>,
    /// `exclude_patterns`, which `TocTree.parse_content` consults to tell an
    /// *excluded* toctree target from a *nonexisting* one. Empty for a parse
    /// without an environment, where no entry resolves anyway.
    pub exclude_patterns: Vec<String>,
    /// The object-signature / py-domain configuration the read phase
    /// consumes ([`crate::py::PySigConfig`]): today the `fix_parens` roles'
    /// `add_function_parentheses`, and from the py directives onward the
    /// signature-wrapping and TOC-entry keys too. Defaults to sphinx's own
    /// defaults, so a parse without a project behaves like a default one.
    pub py: crate::py::PySigConfig,
    /// The project source directory (sphinx `env.srcdir`), which the
    /// `include` directive's sphinx-mode path rewrite resolves against
    /// (`sphinx/directives/other.py:413-416` runs `env.relfn2path` on every
    /// include argument before docutils sees it) and which the parse-time
    /// `included`/`dependencies` records are spelled relative to.
    ///
    /// `None` — the default — means "parsed without a project": include
    /// arguments then resolve the docutils way, relative to the directory
    /// of the *containing file*, and no records are made. Standalone
    /// parses (the differential harnesses, `parse_rst` callers) keep their
    /// current behavior.
    pub srcdir: Option<std::path::PathBuf>,
    /// Sphinx's `source_encoding` config value (`config.py:244`, default
    /// `'utf-8-sig'`), which the environment copies onto
    /// `settings.input_encoding` (`environment/__init__.py:375`). In sphinx
    /// mode both file-inserting directives read it as their default:
    /// `include` through `settings.input_encoding` (`misc.py:116`) and
    /// `literalinclude` through `config.source_encoding` (`code.py:210`);
    /// an explicit `:encoding:` option still wins. Ignored outside sphinx
    /// mode, where bare docutils' `'utf-8'` default applies.
    pub source_encoding: String,
    /// Record one [`IdEvent`](crate::transforms::state::IdEvent) per id
    /// the parse allocates, in `ParseOutput.state.ids` — the read-state
    /// differential's `id_trace`. Tests only; off by default.
    pub trace_ids: bool,
    /// The configuration the Sphinx-mode read consumes ([`ReadConfig`]).
    pub read: ReadConfig,
}

/// The build configuration a Sphinx-mode parse reads, each field holding
/// Sphinx 9.1.0's default unless the configuration says otherwise
/// ([`ReadConfig::from`] a [`crate::config::BuildConfig`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadConfig {
    /// `primary_domain` (`SP/sphinx/config.py:268`, default `'py'`): the
    /// domain each document starts with as its default
    /// (`env.current_document.default_domain`), which unprefixed role and
    /// directive names are looked up in before the std domain. A name no
    /// domain is registered under, or `None`, leaves a document without
    /// one (`env.domains.get(name)`).
    pub primary_domain: Option<String>,
    /// `highlight_language` (`config.py:257`, default `'default'`): the
    /// language a `code-block` or Sphinx `code` directive with no argument
    /// takes when no `.. highlight::` set one.
    pub highlight_language: String,
    /// Whether `sphinx.ext.intersphinx` is in `extensions`: only then is
    /// `:external:` a role ([RC §2.2], [TW F12]).
    pub intersphinx: bool,
}

impl Default for ReadConfig {
    fn default() -> Self {
        ReadConfig {
            primary_domain: Some("py".to_string()),
            highlight_language: "default".to_string(),
            intersphinx: false,
        }
    }
}

impl From<&crate::config::BuildConfig> for ReadConfig {
    fn from(config: &crate::config::BuildConfig) -> Self {
        ReadConfig {
            primary_domain: config.primary_domain.clone(),
            highlight_language: config.highlight_language.clone(),
            intersphinx: config.intersphinx_enabled(),
        }
    }
}

/// Sphinx's default `source_encoding` (`config.py:244`).
pub const DEFAULT_SOURCE_ENCODING: &str = "utf-8-sig";

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions {
            source_path: "<string>".to_string(),
            sphinx: false,
            docname: "index".to_string(),
            found_docs: None,
            exclude_patterns: Vec::new(),
            py: crate::py::PySigConfig::default(),
            srcdir: None,
            source_encoding: DEFAULT_SOURCE_ENCODING.to_string(),
            trace_ids: false,
            read: ReadConfig::default(),
        }
    }
}

/// Pre-conversion directive tuple mirroring the M1 validation scanner's
/// semantics (whitespace-split args, inline-admonition content routing,
/// raw string options) — the feed for `DirectiveValidationSystem`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DirectiveRecord {
    /// Source-table index of the marker line (`Doctree::sources`): a
    /// directive inside an included file must be reported against THAT
    /// file, since `line` is numbered within it. Deliberately not
    /// `#[serde(default)]` (cache-shape rule, see
    /// [`RegistryExport::program_options`]): a pre-provenance document
    /// cache entry decoding with source 0 would pair every included
    /// directive's line with the includer's path again.
    pub source: u16,
    pub name: String,
    pub arguments: Vec<String>,
    pub options: Vec<(String, String)>,
    pub content: String,
    /// 1-based marker line, within `source`.
    pub line: u32,
}

/// A role occurrence (sphinx mode): validation + nitpicky feed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoleRecord {
    /// Source-table index of the enclosing text block's first line — see
    /// [`DirectiveRecord::source`], same cache-shape rule.
    pub source: u16,
    /// Final role-name segment, lowercased (`:py:func:` records `func`),
    /// with the full as-written name kept alongside.
    pub name: String,
    pub full_name: String,
    pub target: String,
    pub display: Option<String>,
    /// 1-based line of the enclosing text block's first line, within
    /// `source`.
    pub line: u32,
}

/// A toctree directive occurrence (sphinx mode).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToctreeRecord {
    pub glob: bool,
    pub entries: Vec<ToctreeEntryRecord>,
    /// The optional caption is a title in Sphinx's search index; entry
    /// labels and targets are not searchable body text.
    pub caption: Option<String>,
    /// Source-table index of the `.. toctree::` line — the section-
    /// numbering warning (`location=toctreenode`) names this source's
    /// path. Not `#[serde(default)]` (see [`DirectiveRecord::source`]).
    pub source: u16,
    /// 1-based line of the directive, within `source`.
    pub line: u32,
    /// Diagnostics `TocTree.parse_content` produced while resolving this
    /// directive's entries. They ride the record because the parser has
    /// no warning sink: the read logs them with the parse's other records
    /// ([`crate::read::read_document`]), and the merge reads their kinds.
    pub warnings: Vec<crate::env::toctree::ToctreeWarning>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToctreeEntryRecord {
    pub title: Option<String>,
    pub target: String,
    /// 1-based line of the entry itself.
    pub line: u32,
}

/// One `Cmdoption.add_target_and_index` call the parse layer made
/// (`sphinx/domains/std/__init__.py:308-315`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProgramOptionRecord {
    /// Source-table index of the registering signature (an option inside
    /// an included file must attribute to that file). Deliberately not
    /// `#[serde(default)]` — the cache-shape rule
    /// [`RegistryExport::program_options`] explains.
    pub source: u16,
    /// The `.. program::` in scope, `None` outside one.
    pub program: Option<String>,
    /// One `desc_signature['allnames']` spelling (`--file`, `-f`, ...).
    pub name: String,
    /// `signode['ids'][0]` — the *first* id of the signature, which is what
    /// Sphinx registers for every spelling in it.
    pub node_id: String,
    /// The parse's record sequence number, drawn at the registration
    /// ([`crate::transforms::state::DocumentState::draw_seq`]): a warning
    /// the replay raises is ordered by it among the parse's records. Not
    /// `#[serde(default)]` (cache-shape rule).
    pub seq: u32,
}

/// One `PythonDomain.note_object` call the parse layer made
/// (`PyObject.add_target_and_index`, `domains/python/_object.py:415-437`,
/// or `PyModule.run`, `__init__.py:522`) — the fullname → `ObjectEntry`
/// registration the env layer replays, plus the provenance the
/// duplicate-description warning needs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PyObjectRecord {
    /// Module-qualified full object name (`mymod.C.meth`), or the canonical
    /// name for an `aliased` record.
    pub fullname: String,
    /// The desc's objtype AFTER directive-name aliasing (`py:classmethod`
    /// registers `method`, `py:decorator` registers `function`).
    pub objtype: String,
    pub node_id: String,
    /// `:canonical:` alias registrations carry `true` (`_object.py:427-437`)
    /// — resolve-time disambiguation prefers non-aliased entries.
    pub aliased: bool,
    /// Source-table index of the registering signature. Deliberately not
    /// `#[serde(default)]` (cache-shape rule, see
    /// [`RegistryExport::program_options`]).
    pub source: u16,
    /// 1-based line of the signature node (`location=signode`).
    pub lineno: u32,
    /// The parse's record sequence number, drawn at the registration (see
    /// [`ProgramOptionRecord::seq`]).
    pub seq: u32,
}

/// One `PythonDomain.note_module` call (`PyModule.run`,
/// `domains/python/__init__.py:515-521`) — the modname → `ModuleEntry`
/// registration feeding the env layer and, later, the py-modindex.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PyModuleRecord {
    pub name: String,
    /// `module-<name>` (or its `module-<n>` collision serial).
    pub node_id: String,
    /// `:synopsis:` option, `''` when absent. (A bare `:synopsis:` with no
    /// value is Python `None` in sphinx's identity-lambda option spec; it is
    /// recorded as `''` here — probe `module_synopsis_bare`.)
    pub synopsis: String,
    /// `:platform:` option, `''` when absent.
    pub platform: String,
    /// `:deprecated:` flag.
    pub deprecated: bool,
    /// Source-table index of the directive. Deliberately not
    /// `#[serde(default)]` (cache-shape rule, see
    /// [`RegistryExport::program_options`]).
    pub source: u16,
    /// 1-based line of the directive marker.
    pub lineno: u32,
    /// The parse's record sequence number, drawn at the registration (see
    /// [`ProgramOptionRecord::seq`]).
    pub seq: u32,
}

/// One `StandardDomain.note_object` call the parse layer made
/// (`GenericObject`/`ConfigurationValue.add_target_and_index`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ObjectRegistration {
    /// Source-table index of the registering signature; the duplicate
    /// warning names this source's path. Deliberately not
    /// `#[serde(default)]` (cache-shape rule, see
    /// [`RegistryExport::program_options`]).
    pub source: u16,
    /// `self.objtype` — `envvar`, `confval`, ... `describe`/`object` never
    /// reach here: the base `add_target_and_index` is a no-op.
    pub objtype: String,
    /// The name `handle_signature` returned, which is what the matching
    /// `:envvar:`/`:confval:` role resolves against.
    pub name: String,
    pub node_id: String,
    /// 1-based line of the signature node (`location=signode`), for the
    /// duplicate-description warning.
    pub line: u32,
    /// The parse's record sequence number, drawn at the registration (see
    /// [`ProgramOptionRecord::seq`]).
    pub seq: u32,
}

/// One glossary term `make_glossary_term` registered
/// (`SP/sphinx/domains/std/__init__.py:375-407`: `std._note_term(termtext,
/// node_id, location=term)`), for the environment to replay in parse order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GlossaryTermRecord {
    /// `termtext`: the parsed term's `astext()`.
    pub term: String,
    /// The term node's id (`make_id(env, document, 'term', termtext)`).
    pub id: String,
    /// The term's `location=term`, as `make_glossary_term` stamps it
    /// (`term.source, term.line = source, lineno`): the source-table index
    /// of the term's line, and [`Self::line`]. Not `#[serde(default)]`
    /// (cache-shape rule, see [`RegistryExport::program_options`]).
    pub source: u16,
    /// The term's `term.line`, which is one *less* than the line it is on:
    /// `make_glossary_term` is handed the linenos of `self.content.items`,
    /// and a directive's content items carry docutils' **0-based** line
    /// offsets (`content_offset` comes from `abs_line_offset()`), while
    /// everything else in a warning location is 1-based. Verified against
    /// Sphinx 9.1.0: a term on source line 8 reports `b.rst:7`, one on line
    /// 11 reports `b.rst:10`.
    pub line: u32,
    /// The parse's record sequence number, drawn at the registration (see
    /// [`ProgramOptionRecord::seq`]).
    pub seq: u32,
}

/// One equation label the math directive registered with the math domain
/// (`MathDirective.add_target` → `domain.note_equation(docname, label,
/// location=node)`, `SP/sphinx/directives/patches.py:177-194`), for stage M
/// to replay in parse order ([`crate::env::math::process_doc`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EquationRecord {
    /// The equation's label (`node['label']`): as the `:label:` or `:name:`
    /// option wrote it, or `<docname>:<n>` for an empty `:label:`.
    pub label: String,
    /// The number the domain gives the equation, `env.new_serialno('eqno')
    /// + 1`: the document's count of labelled equations so far.
    pub number: u32,
    /// `location=node`: the source-table index and line `set_source_info`
    /// stamped on the `math_block`, the directive's. Not
    /// `#[serde(default)]` (cache-shape rule, see
    /// [`RegistryExport::program_options`]).
    pub source: u16,
    pub line: u32,
    /// The parse's record sequence number, drawn at the registration (see
    /// [`ProgramOptionRecord::seq`]).
    pub seq: u32,
}

/// What the parse layer hands the environment besides the doctree itself:
/// what Sphinx registers while the parse runs, against state that dies
/// with it (`env.ref_context`, the serial counters), and that the env
/// collectors need. It rides the document cache
/// ([`crate::document::Document::registry`]), so it stays
/// serde-serializable and cheap to clone.
///
/// The name table, which the std domain's label harvest reads, is not
/// here: it is [`DocumentState::names`], which the read keeps. A cache
/// entry that still carries the `nameids` snapshot this export once held
/// decodes, the key ignored.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RegistryExport {
    /// sphinx `env.new_serialno('index')` counter value at the end of the
    /// parse (shared by the index directive and index-entry-emitting roles).
    pub index_serial: u32,
    /// The std-domain registrations the object-description directives made
    /// while running, in document order.
    ///
    /// Sphinx performs these from inside `add_target_and_index`, against
    /// state the finished doctree does not carry: the program an option
    /// belongs to comes from `env.ref_context['std:program']` and is
    /// stamped on no node, and a `:no-typesetting:` description registers
    /// itself and then **replaces its whole `desc` node with a bare
    /// target** — so a doctree walk can neither recover the program nor see
    /// that the object existed. Recording the calls keeps the env layer
    /// exact for both.
    ///
    /// Deliberately *not* `#[serde(default)]`, for the reason
    /// [`crate::document::Document::registry`] gives: a cache entry written
    /// before this field existed must FAIL to decode so the document is
    /// re-parsed. Defaulting it to an empty vector would let a pre-desc
    /// cache decode cleanly, and every `:option:`/`:envvar:`/`:confval:`
    /// in the project would then dangle against an empty registry.
    pub program_options: Vec<ProgramOptionRecord>,
    /// See [`Self::program_options`] — including why this is not
    /// `#[serde(default)]` either.
    pub std_objects: Vec<ObjectRegistration>,
    /// The py-domain object registrations (`PythonDomain.note_object`), in
    /// document order. Same rationale and cache-shape rule as
    /// [`Self::program_options`]: the program state analog here is the
    /// parser's `py:module`/`py:class` ref_context, which no doctree node
    /// carries, and a `:no-typesetting:` py object registers and then
    /// vanishes from the tree.
    pub py_objects: Vec<PyObjectRecord>,
    /// The py-domain module registrations (`PythonDomain.note_module`), in
    /// document order. Not `#[serde(default)]` — see
    /// [`Self::program_options`].
    pub py_modules: Vec<PyModuleRecord>,
    /// The glossary terms the parse registered (`StandardDomain._note_term`),
    /// in parse order. Not `#[serde(default)]` — see
    /// [`Self::program_options`].
    pub glossary_terms: Vec<GlossaryTermRecord>,
    /// The equation labels the parse registered with the math domain
    /// (`MathDomain.note_equation`), in parse order. Not
    /// `#[serde(default)]` — see [`Self::program_options`]: a cache entry
    /// that decoded with none would leave every `:eq:` to its equations
    /// dangling.
    pub equations: Vec<EquationRecord>,
    /// Diagnostics the parse raised through Sphinx's *logger* rather than
    /// into the tree, which have nowhere else to go: docutils turns a
    /// directive error into a `system_message` node, but a Sphinx directive
    /// calling `logger.warning` produces no node at all. The read moves
    /// them into the document's log, rendered, beside the reporter's
    /// records ([`crate::read::read_document`]), so a read's export holds
    /// none; a bare parse keeps them here.
    pub log_warnings: Vec<ParseLogWarning>,
    /// Files the document pulls in at parse time (docutils
    /// `settings.record_dependencies`, harvested by sphinx's
    /// `DependenciesCollector`): one srcdir-relative normalized path per
    /// successfully opened `include` target — non-doc files included,
    /// standard includes excluded (§Scope-2b). The env layer replays these
    /// into `env.dependencies`, which drives `get_outdated_files`. Not
    /// `#[serde(default)]` — see [`Self::program_options`]: a pre-include
    /// cache decoding with an empty list would never re-read the document
    /// when an included file changes.
    pub dependencies: Vec<String>,
    /// The docnames this document textually includes (sphinx
    /// `env.note_included`, recorded for every include argument that maps
    /// to a docname — before the file is even opened, like sphinx). The
    /// env layer replays these into `env.included`, whose only consumer is
    /// the orphan check. Not `#[serde(default)]` — see
    /// [`Self::program_options`].
    pub included: Vec<String>,
}

/// Where a Sphinx `logger.warning(..., location=...)` points, in the forms
/// the parse produces, each rendered the way `SphinxLogRecordTranslator`
/// renders it (`SP/sphinx/util/logging.py:500-525`; [`Self::render`]).
/// Source ids index the document's source table (`Doctree::sources`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LogLocation {
    /// No `location`: the record prints bare.
    None,
    /// `location=docname`: the document's path, with no line.
    Doc,
    /// `location=(docname, lineno)`.
    DocLine(u32),
    /// `location=node`: the node's source, made absolute, and its line
    /// (`get_node_location`).
    Node { source: u16, line: u32 },
    /// `location=(source, line)` whose first element is a source path,
    /// which the translator takes for a docname.
    Tuple { source: u16, line: u32 },
}

impl LogLocation {
    /// The printed location, rendered against the document's source table,
    /// the project's source directory and the document's name:
    ///
    /// - `Doc` and `DocLine` are `doc2path(docname)`, Sphinx's
    ///   `srcdir / (docname + source_suffix[0])` (`.rst` until T17c),
    ///   with `:lineno` when the line is non-zero;
    /// - `Tuple` passes the source path as the docname: `doc2path` finds
    ///   no such document and appends the suffix again
    ///   (`SP/sphinx/project.py:114-128`), so the document itself, whose
    ///   table path is absolute, prints `<srcdir>/index.rst.rst:N`, and an
    ///   included file, whose table path is srcdir-relative, prints
    ///   `<srcdir>/part.rst.rst:N` ([INC §3.4]);
    /// - `Node` is `get_node_location`'s `abspath(source)`
    ///   ([`node_source_path`]), so `<srcdir>/part.rst:N` for an included
    ///   file (`SP/sphinx/util/logging.py:554-562`); a node without a line
    ///   prints `path:`;
    /// - `None` prints nothing.
    ///
    /// A path already under `srcdir` stands alone, as a rooted one does:
    /// the document's own table path is absolute in a build, and a read
    /// with no project spells it under a stand-in source directory
    /// (`<SRCDIR>/index.rst`, [`crate::read::read_document`]).
    pub fn render(
        self,
        sources: &[String],
        srcdir: &std::path::Path,
        docname: &str,
    ) -> crate::diagnostics::Location {
        use crate::diagnostics::Location;
        let table_path = |source: u16| {
            sources
                .get(usize::from(source))
                .or_else(|| sources.first())
                .map(String::as_str)
                .unwrap_or_default()
        };
        let under_srcdir = |path: &str| {
            if std::path::Path::new(path).starts_with(srcdir) {
                path.to_string()
            } else {
                srcdir.join(path).display().to_string()
            }
        };
        let doc2path = |name: &str| under_srcdir(&format!("{name}.rst"));
        let path_line = |path: String, line: u32| Location::Path {
            path,
            line: (line != 0).then_some(line),
        };
        match self {
            LogLocation::None => Location::None,
            LogLocation::Doc => path_line(doc2path(docname), 0),
            LogLocation::DocLine(line) => path_line(doc2path(docname), line),
            LogLocation::Tuple { source, line } => path_line(doc2path(table_path(source)), line),
            LogLocation::Node { source, line } => {
                let path = node_source_path(table_path(source), srcdir);
                match line {
                    0 => Location::Verbatim(format!("{path}:")),
                    line => path_line(path, line),
                }
            }
        }
    }

    /// The `WARNING` record of `logger.warning(message, location=…)` with
    /// this location, rendered by [`Self::render`], and `category`, the
    /// record's `type` and `subtype`.
    pub fn warning(
        self,
        sources: &[String],
        srcdir: &std::path::Path,
        docname: &str,
        message: String,
        category: crate::diagnostics::Category,
    ) -> crate::diagnostics::Diagnostic {
        crate::diagnostics::Diagnostic {
            location: self.render(sources, srcdir, docname),
            severity: crate::diagnostics::Severity::Warning,
            message,
            category,
        }
    }
}

/// `os.path.abspath(source)`: the path Sphinx's `get_node_location`
/// prints for the source of a node a logger warning is about
/// (`SP/sphinx/util/logging.py:554-562`), whichever phase warns. An
/// included file's table path is relative to the source directory, as
/// docutils' is to the cwd, which the oracle runs in: `abspath` gives the
/// file's absolute path either way, so a warning about a node of
/// `part.rst` prints `<srcdir>/part.rst` whatever the cwd. `abspath`
/// also collapses `.` and `..` (`normpath`), so a file included from
/// outside the source directory prints as `<root>/outside.inc`, not
/// `<root>/src/../outside.inc` (probed on Sphinx 9.1.0).
///
/// A path already under `srcdir` stands alone, as a rooted one does
/// ([`LogLocation::render`]).
pub(crate) fn node_source_path(table_path: &str, srcdir: &std::path::Path) -> String {
    use std::path::{Component, Path, PathBuf};
    if Path::new(table_path).starts_with(srcdir) {
        return table_path.to_string();
    }
    let joined = srcdir.join(table_path);
    let dotted = table_path
        .split(['/', '\\'])
        .any(|segment| matches!(segment, "." | ".."));
    if !dotted {
        return joined.display().to_string();
    }
    let mut normal = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normal.components().next_back() {
                Some(Component::Normal(_)) => {
                    normal.pop();
                }
                // `normpath('/..')` is `/`.
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => normal.push(".."),
            },
            other => normal.push(other.as_os_str()),
        }
    }
    normal.display().to_string()
}

/// One `logger.warning` a directive raised during the parse.
///
/// Producers: `Cmdoption.handle_signature`'s malformed option description
/// (`domains/std/__init__.py:237-245`) and — since wave 4.5 — the three
/// `literalinclude` reader warnings ([INC §3.4]). All are logged with no
/// `type`/`subtype` and so render with no `[category]` suffix.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ParseLogWarning {
    /// The warning text, already formatted exactly as Sphinx renders it.
    pub message: String,
    /// The `location=` Sphinx passes ([`LogLocation::render`]). Not
    /// `#[serde(default)]` (cache-shape rule, see
    /// [`RegistryExport::program_options`]): a cache entry written before
    /// it must MISS, not decode with no location.
    pub location: LogLocation,
    /// The parse's record sequence number, drawn when the warning was
    /// logged ([`crate::transforms::state::DocumentState::draw_seq`]). Not
    /// `#[serde(default)]` (cache-shape rule).
    pub seq: u32,
}

impl ParseLogWarning {
    /// The record Sphinx logs: a `WARNING` with no category, located by
    /// [`LogLocation::render`].
    pub fn diagnostic(
        &self,
        sources: &[String],
        srcdir: &std::path::Path,
        docname: &str,
    ) -> crate::diagnostics::Diagnostic {
        self.location.warning(
            sources,
            srcdir,
            docname,
            self.message.clone(),
            crate::diagnostics::Category::None,
        )
    }
}

/// What the parser threads through every nested and detached sub-parse
/// beside the docutils object state ([`DocumentState`], whose `parse`
/// field holds it), so that a sub-parse continues the document rather than
/// starting a new one: the source table's paths, the record streams
/// [`ParseOutput`] carries, the dupnames still owed to nodes the parser has
/// placed, Sphinx's per-document serial counters, docutils' `include_log`
/// and the `note_pending` counter. The parse drains it when it ends.
#[derive(Debug, Default, Clone)]
pub(crate) struct ParseOnlyState {
    /// The paths of the parser's source table, by source id (what
    /// `Doctree::sources` publishes): the reporter stamps them on the
    /// messages it builds ([`DocumentState::report`]).
    pub(crate) sources: Vec<std::sync::Arc<str>>,
    /// docutils `dupname(old_node, name)` on nodes already placed in the
    /// tree, applied when the tree is complete
    /// ([`crate::doctree::ids::apply_dupname_fixups`]).
    pub(crate) dupname_fixups: Vec<DupnameFixup>,
    /// sphinx `env.new_serialno('index')`, shared by the index directive
    /// and the roles that emit index entries.
    pub(crate) index_serial: u32,
    /// The other `env.new_serialno(category)` counters, keyed by the
    /// category: the `sphinx.util.nodes.make_id` prefix that asked for one
    /// (`cmdoption-myprog`, `confval`, …), or the math directive's `eqno`
    /// and `sphinx.ext.math#equations`; per document and category, from 0,
    /// like [`Self::index_serial`] ([`DocumentState::new_serialno`]).
    pub(crate) serialnos: HashMap<String, u32>,
    /// docutils `document.include_log` (`nodes.py:1802-1803`): the open
    /// inclusions as `(source display path, clip options)` pairs. Seeded
    /// with the root document (and an empty clip) on first use
    /// (`misc.py:253-256`); an entry is pushed before each splice and
    /// popped when the comment path reaches the matching
    /// `.. end of inclusion from "..."` marker.
    pub(crate) include_log: Vec<(String, block::IncludeClip)>,
    /// The next `document.note_pending` order
    /// ([`crate::doctree::pending::Pending::order`]).
    pub(crate) next_pending_order: u32,
    /// Validation-feed records ([`DirectiveRecord`], [`RoleRecord`]) and
    /// the toctree records.
    pub(crate) directive_records: Vec<DirectiveRecord>,
    pub(crate) role_records: Vec<RoleRecord>,
    pub(crate) toctree_records: Vec<ToctreeRecord>,
    /// The std- and py-domain registrations the parse made, in document
    /// order (see [`RegistryExport::program_options`] for why the finished
    /// doctree cannot carry them).
    pub(crate) program_option_records: Vec<ProgramOptionRecord>,
    pub(crate) std_object_records: Vec<ObjectRegistration>,
    pub(crate) py_object_records: Vec<PyObjectRecord>,
    pub(crate) py_module_records: Vec<PyModuleRecord>,
    pub(crate) glossary_term_records: Vec<GlossaryTermRecord>,
    /// The math domain's equation registrations ([`EquationRecord`]).
    pub(crate) equation_records: Vec<EquationRecord>,
    /// The ids `document.ids` holds for an equation's target: the ones the
    /// math directive registered first, whose `Duplicate ID` message it
    /// can spell.
    pub(crate) equation_ids: HashSet<String>,
    /// The `Duplicate ID` records whose text still lacks the start tag of
    /// the node that holds the id, which the parse spells once its tree
    /// is complete ([`DuplicateId`]).
    pub(crate) duplicate_ids: Vec<DuplicateId>,
    /// By id, the start tag of the first node holding it in a subtree the
    /// parse dropped ([`DocumentState::detach`]), as the node was then:
    /// docutils' `document.ids` keeps such a node, and a later `Duplicate
    /// ID` names it ([`DocumentState::spell_duplicate_ids`]).
    pub(crate) dropped_id_holders: HashMap<String, String>,
    /// The nodes docutils stamps with `abs_line_number()`, whose line the
    /// parse knows once the top-level input is complete ([`AbsoluteLine`]).
    pub(crate) absolute_lines: Vec<AbsoluteLine>,
    /// `logger.warning` diagnostics raised while running directives.
    pub(crate) log_warnings: Vec<ParseLogWarning>,
    /// Parse-recorded dependency paths and included docnames (see
    /// [`RegistryExport::dependencies`]); sphinx mode only.
    pub(crate) dependency_records: Vec<String>,
    pub(crate) included_records: Vec<String>,
}

/// docutils' `Duplicate ID: "<id>" used by <holder's start tag> and <the
/// node's start tag>` (`SP/docutils/nodes.py:1832-1843`), logged when the
/// node's preset id turned out taken ([`DocumentState::set_id_at`]). The
/// node that holds the id can be anywhere in the tree the parser is still
/// building, so its start tag is read off the finished tree
/// ([`DocumentState::spell_duplicate_ids`]).
#[derive(Debug, Clone)]
pub(crate) struct DuplicateId {
    /// The index of the record in [`DocumentState::log`].
    pub(crate) record: usize,
    pub(crate) id: String,
    /// The node the id was preset on, which is not the holder.
    pub(crate) node: crate::doctree::Uid,
    /// That node's start tag when the message was made.
    pub(crate) starttag: String,
}

/// A node docutils stamps with the state machine's `abs_line_number()`, the
/// 1-based index of its line in the top-level input with every inclusion's
/// lines and padding lines spliced in, offset by a table cell's count
/// (`SP/docutils/parsers/rst/states.py:2057`, `:2123` for a hyperlink
/// target): `(file, absolute line)` once `setup_child` gives it the
/// current source in a container that has `document` set, the absolute
/// line alone in one that has not, which the reporter reads back against
/// the input ([`DocumentState::source_and_line`]). The parse records the
/// node's own line, and resolves it when the input is complete
/// (`BlockParser::resolve_absolute_lines`).
#[derive(Debug, Clone)]
pub(crate) struct AbsoluteLine {
    pub(crate) node: crate::doctree::Uid,
    /// The node's line as this parser numbers it: its source-table entry
    /// and its 1-based line there.
    pub(crate) source: u16,
    pub(crate) lineno: u32,
    /// What the machine's count adds there (`BlockParser::line_bias`).
    pub(crate) bias: u32,
    /// Whether the machine counts the line by its line in its own file, as
    /// a glossary definition's does (`BlockParser::definition_bias_base`),
    /// not by its place in the top-level input; `bias` is then what accrued
    /// inside the definition.
    pub(crate) own_line: bool,
    /// Whether the node's container has `document` set.
    pub(crate) attached: bool,
}

impl ParseOnlyState {
    /// The path of source-table entry `source`; the document's own path
    /// for an id the table does not hold, and docutils' `'<string>'` for a
    /// state with no table (a standalone inline parse).
    pub(crate) fn source_path(&self, source: u16) -> &str {
        self.sources
            .get(usize::from(source))
            .or_else(|| self.sources.first())
            .map_or("<string>", |path| path)
    }
}

/// Everything a parse produces: the doctree plus the flat records the
/// build pipeline consumes without re-walking raw source.
pub struct ParseOutput {
    pub doctree: Doctree,
    pub directive_records: Vec<DirectiveRecord>,
    pub role_records: Vec<RoleRecord>,
    pub toctrees: Vec<ToctreeRecord>,
    pub registry: RegistryExport,
    /// The docutils document state the parse leaves behind, which the read
    /// transforms read beside the tree ([`DocumentState`]).
    pub state: DocumentState,
}

/// Parse RST source into a doctree. Never errors — problems become
/// `system_message` nodes, exactly like docutils — and never panics in
/// release builds; debug builds assert the registration invariant at the
/// end of the parse ([`DocumentState::misplaced_registrations`]).
pub fn parse_rst(source: &str, opts: &ParseOptions) -> Doctree {
    parse_rst_full(source, opts).doctree
}

pub fn parse_rst_full(source: &str, opts: &ParseOptions) -> ParseOutput {
    let mut parser = block::BlockParser::new(source, &opts.source_path);
    parser.sphinx = opts.sphinx;
    parser.docname = opts.docname.clone();
    parser.found_docs = opts.found_docs.clone();
    parser.exclude_patterns = opts.exclude_patterns.clone();
    parser.py = opts.py.clone();
    parser.srcdir = opts.srcdir.clone();
    parser.source_encoding = opts.source_encoding.clone();
    // `env.current_document.default_domain =
    // env.domains.get(config.primary_domain)` (`prepare_settings`).
    parser.default_domain = opts
        .read
        .primary_domain
        .as_deref()
        .and_then(roles::DomainId::from_name);
    parser.read = opts.read.clone();
    if opts.trace_ids {
        parser.state.ids.enable_trace();
    }
    parser.parse_document_full()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doctree::{kinds, Node, Uid};
    use serde_json::{json, Value};

    /// The complete current [`RegistryExport`] shape, with one record of
    /// every kind, so that a guard below can remove exactly ONE field and
    /// know the decode failed for no other reason.
    const COMPLETE_REGISTRY: &str = r#"{"index_serial":0,
        "program_options":[{"source":0,"program":null,"name":"-f","node_id":"a","seq":0}],
        "std_objects":[{"source":0,"objtype":"envvar","name":"P","node_id":"b","line":1,
            "seq":1}],
        "py_objects":[{"fullname":"m.f","objtype":"function","node_id":"m.f",
            "aliased":false,"source":0,"lineno":1,"seq":2}],
        "py_modules":[{"name":"m","node_id":"module-m","synopsis":"","platform":"",
            "deprecated":false,"source":0,"lineno":1,"seq":3}],
        "glossary_terms":[{"term":"T","id":"term-T","source":0,"line":1,"seq":4}],
        "equations":[{"label":"e","number":1,"source":0,"line":3,"seq":6}],
        "log_warnings":[{"message":"m","location":{"Node":{"source":0,"line":2}},"seq":5}],
        "dependencies":["part.rst"],"included":["part"]}"#;

    /// Decode [`COMPLETE_REGISTRY`] with the field `name` removed at
    /// `path` (object keys and array indices), and require the failure
    /// to be about THAT field. A blob that also omitted some other
    /// required field would fail whether or not the field under test is
    /// defaulting — which is how the earlier hand-written blobs stopped
    /// discriminating (panel fix round B, [3]).
    fn must_miss(path: &[&str], name: &str) {
        let mut value: serde_json::Value = serde_json::from_str(COMPLETE_REGISTRY).unwrap();
        serde_json::from_value::<RegistryExport>(value.clone())
            .expect("the complete current shape decodes");
        let mut slot = &mut value;
        for part in path {
            slot = match part.parse::<usize>() {
                Ok(index) => &mut slot[index],
                Err(_) => &mut slot[*part],
            };
        }
        slot.as_object_mut()
            .unwrap()
            .remove(name)
            .unwrap_or_else(|| panic!("{path:?}/{name} is not in the complete shape"));
        let error = serde_json::from_value::<RegistryExport>(value)
            .err()
            .unwrap_or_else(|| panic!("a registry missing {path:?}/{name} decoded"))
            .to_string();
        assert!(
            error.contains(&format!("missing field `{name}`")),
            "the decode must fail on the missing {path:?}/{name}, not elsewhere: {error}"
        );
    }

    /// [`RegistryExport`]'s newer fields carry state that cannot be recovered
    /// from a cached doctree, so a cache entry written before they existed
    /// must MISS rather than decode with empty vectors — decoding it would
    /// reuse a doctree still full of unknown-directive errors and leave every
    /// `:option:`/`:envvar:`/`:confval:` in the project dangling. Guards the
    /// `#[serde(default)]` off these fields, which nothing else would catch:
    /// the warm-rebuild tests round-trip the current shape only. Each case
    /// removes exactly one top-level field from the complete shape.
    #[test]
    fn a_registry_written_before_the_std_records_existed_fails_to_decode() {
        for field in [
            // A wave-4 registry (no py record streams at all) must MISS: a
            // defaulted empty vector would leave every py xref in the
            // project dangling on a warm rebuild.
            "program_options",
            "std_objects",
            "py_objects",
            "py_modules",
            // A pre-wave-5a registry (no glossary-term stream) must MISS.
            "glossary_terms",
            // So must one written before the math domain's equation
            // stream (wave 5a, T13b): a defaulted empty list would leave
            // every `:eq:` to its equations dangling on a warm rebuild.
            "equations",
            "log_warnings",
            // A wave-4.5 pre-include registry (no dependencies/included
            // stream) must MISS: a defaulted empty list would never
            // re-read the document when an included file changes, and
            // would silently un-suppress the orphan warning.
            "dependencies",
            "included",
        ] {
            must_miss(&[], field);
        }
    }

    /// The `nameids` snapshot the export once carried is gone (nothing read
    /// it once the std domain read the name table), and a cache entry that
    /// still holds it decodes, the key ignored: its other fields are the
    /// current shape, so retiring the snapshot costs no re-read.
    #[test]
    fn a_registry_with_the_retired_nameids_snapshot_still_decodes() {
        let mut value: Value = serde_json::from_str(COMPLETE_REGISTRY).unwrap();
        value["nameids"] = json!([["tgt", "tgt", true]]);
        let registry =
            serde_json::from_value::<RegistryExport>(value).expect("the retired key is ignored");
        assert_eq!(registry.program_options.len(), 1);
    }

    /// The per-record `source` fields added by the provenance wave follow
    /// the same rule: a cache entry whose records predate them must FAIL to
    /// decode (a defaulted 0 would silently mis-attribute nothing today,
    /// but would decode a stale record stream as current). Each case
    /// removes exactly one field from one record of the complete shape.
    #[test]
    fn records_written_before_the_source_field_existed_fail_to_decode() {
        must_miss(&["program_options", "0"], "source");
        must_miss(&["std_objects", "0"], "source");
        must_miss(&["py_objects", "0"], "source");
        must_miss(&["py_modules", "0"], "source");
        // A glossary term recorded before its location (wave 5a, T13a)
        // must MISS: the duplicate-term warning reads it.
        must_miss(&["glossary_terms", "0"], "source");
        must_miss(&["glossary_terms", "0"], "line");
        // So must an equation recorded without its location (T13b): the
        // duplicate-label warning reads it.
        must_miss(&["equations", "0"], "source");
        must_miss(&["equations", "0"], "line");
        // A pre-wave-5a log record (its `source`/`line`/`doc2path_location`
        // triple, no `location`) must MISS: decoding it with a default
        // location would render the warning at the wrong path.
        must_miss(&["log_warnings", "0"], "location");
    }

    /// The parse's record sequence numbers (wave 5a, [DS §6.4]) order the
    /// replayed registrations among the parse's records; a cache entry
    /// written before them must MISS rather than decode every record at
    /// seq 0. Each case removes exactly one `seq` from the complete shape.
    #[test]
    fn records_written_before_their_seq_existed_fail_to_decode() {
        for registry in [
            "program_options",
            "std_objects",
            "py_objects",
            "py_modules",
            "glossary_terms",
            "equations",
            "log_warnings",
        ] {
            must_miss(&[registry, "0"], "seq");
        }
        // A toctree warning rides its `ToctreeRecord` in the document
        // cache, under the same rule.
        let complete = r#"{"source":1,"line":3,"message":"m","category":null,
            "kind":"MissingDocument","seq":0}"#;
        serde_json::from_str::<crate::env::toctree::ToctreeWarning>(complete)
            .expect("the current shape decodes");
        let error = serde_json::from_str::<crate::env::toctree::ToctreeWarning>(
            r#"{"source":1,"line":3,"message":"m","category":null,"kind":"MissingDocument"}"#,
        )
        .expect_err("a warning without its seq must not decode")
        .to_string();
        assert!(error.contains("missing field `seq`"), "{error}");
    }

    /// The provenance fields panel fix round B added to the DOCUMENT-side
    /// records (`DirectiveRecord`, `RoleRecord`, `ToctreeRecord` and the
    /// `ToctreeWarning` it carries) follow the same rule. These ride the
    /// document cache (`src/cache.rs`, serde_json): a pre-field entry
    /// decoding with `source: 0` would silently report every directive,
    /// role and toctree inside an included file against the includer's
    /// path again — the exact regression the fields exist to close.
    ///
    /// Each stale blob is the CURRENT complete shape minus `source` and
    /// nothing else, so the decode can fail for no other reason; the error
    /// text is asserted to name that field.
    #[test]
    fn document_records_written_before_their_source_field_existed_fail_to_decode() {
        fn must_miss<T: serde::de::DeserializeOwned>(complete: &str, stale: &str) {
            serde_json::from_str::<T>(complete).expect("the current shape decodes");
            let error = serde_json::from_str::<T>(stale)
                .err()
                .unwrap_or_else(|| panic!("a stale record decoded: {stale}"))
                .to_string();
            assert!(
                error.contains("missing field `source`"),
                "the decode must fail on the missing source field, not elsewhere: \
                 {error} ({stale})"
            );
        }

        must_miss::<DirectiveRecord>(
            r#"{"source":1,"name":"note","arguments":[],"options":[],"content":"x","line":3}"#,
            r#"{"name":"note","arguments":[],"options":[],"content":"x","line":3}"#,
        );
        must_miss::<RoleRecord>(
            r#"{"source":1,"name":"ref","full_name":"ref","target":"t","display":null,
                "line":3}"#,
            r#"{"name":"ref","full_name":"ref","target":"t","display":null,"line":3}"#,
        );
        must_miss::<crate::env::toctree::ToctreeWarning>(
            r#"{"source":1,"line":3,"message":"m","category":null,"kind":"MissingDocument",
                "seq":0}"#,
            r#"{"line":3,"message":"m","category":null,"kind":"MissingDocument","seq":0}"#,
        );
        must_miss::<ToctreeRecord>(
            r#"{"glob":false,"entries":[],"source":1,"line":3,"warnings":[]}"#,
            r#"{"glob":false,"entries":[],"line":3,"warnings":[]}"#,
        );
    }

    // ------------------------------------------------------------------
    // docutils registrations by node identity ([DS §4, §7.3, §8.2])
    // ------------------------------------------------------------------

    fn sphinx_options() -> ParseOptions {
        ParseOptions {
            source_path: "<snippet>".into(),
            sphinx: true,
            ..Default::default()
        }
    }

    fn sphinx_parse(source: &str) -> ParseOutput {
        parse_rst_full(source, &sphinx_options())
    }

    /// `registries_post_parse` of a sphinx-mode parse.
    fn dump(source: &str) -> Value {
        let out = sphinx_parse(source);
        out.state.registries_dump(&out.doctree.root)
    }

    /// The ids of every `kind` node in document order.
    fn tree_ids(node: &Node, kind: &str, out: &mut Vec<String>) {
        if node.kind == kind {
            out.extend(node.attrs.ids.iter().cloned());
        }
        for child in &node.children {
            tree_ids(child, kind, out);
        }
    }

    fn footnote_ref_tree_order(out: &ParseOutput) -> Vec<String> {
        let mut ids = Vec::new();
        tree_ids(&out.doctree.root, kinds::FOOTNOTE_REFERENCE, &mut ids);
        ids
    }

    /// `[kind, ids, attached]` entries of `autofootnote_refs`.
    fn auto_ref(id: &str, attached: bool) -> Value {
        json!(["footnote_reference", [id], attached])
    }

    /// Only the facts [DS §4.2] (the corpus rows where registration order
    /// or membership differs from the tree) and [DS §4.3 (a)-(h)] state,
    /// checked on the post-parse dump. Every value below is also what a
    /// parse-only Sphinx 9.1.0 run records for the input
    /// (`apply_transforms` intercepted, registries walked by identity).
    /// Every other `state.*` case is compared in T6 against the generated
    /// fixture.
    #[test]
    fn registries_post_parse_literal_facts() {
        // docutils-0575: the `[#]_` registers during the inline parse, then
        // the substitution definition is rejected — the reference stays
        // registered, detached.
        let d = dump(".. |f| replace:: note [#]_\n");
        assert_eq!(d["autofootnote_refs"], json!([auto_ref("id1", false)]));

        // docutils-0224 / -0226: the embedded alias's target registers in
        // `indirect_targets` and `refnames` BEFORE its reference
        // (`states.py:977-978`), so registration order is not tree order.
        for (source, alias) in [
            ("See `text <alias_>`_ here.\n", "alias"),
            ("See `text <two words_>`_ here.\n", "two words"),
        ] {
            let d = dump(source);
            assert_eq!(d["indirect_targets"], json!([["target", ["text"], true]]));
            assert_eq!(
                d["refnames"],
                json!([[alias, [["target", ["text"], true], ["reference", [], true]]]]),
                "{source}"
            );
            assert_eq!(d["names"], json!([["text", "text", false]]));
        }

        // docutils-0240 / -0241: a citation reference lands in
        // `citation_refs` and `refnames` (619 later detaches it).
        for (source, label) in [
            ("Cite [cite2020]_ end.\n", "cite2020"),
            ("Cite [Cite.2020-X]_ end.\n", "cite.2020-x"),
        ] {
            let d = dump(source);
            let registered = json!([[label, [["citation_reference", ["id1"], true]]]]);
            assert_eq!(d["citation_refs"], registered, "{source}");
            assert_eq!(d["refnames"], registered, "{source}");
        }

        // (a) DocFieldTransformer reorders the registered references: the
        // registry is id1 id2 id3, the tree id1 id3 id2.
        let source = ".. py:function:: f(a, b)\n\n   :param a: first [#]_\n   \
                      :returns: result [#]_\n   :param b: second [#]_\n\n\
                      .. [#] one\n.. [#] two\n.. [#] three\n";
        let out = sphinx_parse(source);
        let d = out.state.registries_dump(&out.doctree.root);
        assert_eq!(
            d["autofootnote_refs"],
            json!([
                auto_ref("id1", true),
                auto_ref("id2", true),
                auto_ref("id3", true)
            ])
        );
        assert_eq!(footnote_ref_tree_order(&out), ["id1", "id3", "id2"]);

        // (b) a `:sorted:` glossary: zeta's reference registers first
        // (id1), alpha's second (id2). The parse keeps the source order;
        // the read puts alpha first (`GlossarySorter`, 500), so the
        // post-read tree has them the other way round.
        let source = ".. glossary::\n   :sorted:\n\n   zeta\n      Last letter [#]_.\n\n   \
                      alpha\n      First letter [#]_.\n\n.. [#] z\n.. [#] a\n";
        let out = sphinx_parse(source);
        let d = out.state.registries_dump(&out.doctree.root);
        assert_eq!(
            d["autofootnote_refs"],
            json!([auto_ref("id1", true), auto_ref("id2", true)])
        );
        assert_eq!(footnote_ref_tree_order(&out), ["id1", "id2"]);

        // (c) the abstract's reference registers before the author's.
        let d = dump(
            ":Abstract: abstract text [#]_\n:Author: someone [#]_\n\n.. [#] one\n.. [#] two\n",
        );
        assert_eq!(
            d["autofootnote_refs"],
            json!([auto_ref("id1", true), auto_ref("id2", true)])
        );

        // (d) rejected `table` / `list-table` content keeps its
        // registrations: the references in the title and the content are
        // registered and detached (the footnotes' later backrefs).
        let d = dump(
            ".. table:: Cap [#]_\n\n   not a table [#]_\n\nText [#]_.\n\n.. [#] a\n.. [#] b\n",
        );
        assert_eq!(
            d["autofootnote_refs"],
            json!([
                auto_ref("id1", false),
                auto_ref("id2", false),
                auto_ref("id3", true)
            ])
        );
        assert_eq!(
            d["autofootnotes"],
            json!([["footnote", ["id4"], true], ["footnote", ["id5"], true]])
        );
        let d = dump(".. list-table::\n\n   not a list [#]_\n\nText [#]_.\n\n.. [#] a\n");
        assert_eq!(
            d["autofootnote_refs"],
            json!([auto_ref("id1", false), auto_ref("id2", true)])
        );
        assert_eq!(d["autofootnotes"], json!([["footnote", ["id3"], true]]));

        // (e) a rejected substitution definition's embedded alias: the
        // target stays an indirect target and keeps its name, detached.
        let d = dump(".. |f| replace:: `x <a_>`_\n\nText.\n");
        assert_eq!(d["indirect_targets"], json!([["target", ["x"], false]]));
        assert_eq!(
            d["refnames"],
            json!([["a", [["target", ["x"], false], ["reference", [], false]]]])
        );
        assert_eq!(d["names"], json!([["x", "x", false]]));
        // ... and a plain inline target keeps its name (no registry holds
        // the target itself).
        let d = dump(".. |f| replace:: _`x`\n\nText x_.\n");
        assert_eq!(d["names"], json!([["x", "x", true]]));
        assert_eq!(d["refnames"], json!([["x", [["reference", [], true]]]]));

        // (f) the reference inside the definition is the first `refnames`
        // entry for its name (the 220 copies append to it later).
        let d = dump("One |s| two |s|.\n\n.. _indirect: missing_\n\n.. |s| replace:: indirect_\n");
        assert_eq!(
            d["refnames"],
            json!([
                ["missing", [["target", ["indirect"], true]]],
                ["indirect", [["reference", [], true]]]
            ])
        );
        assert_eq!(
            d["substitution_defs"],
            json!([["s", ["substitution_definition", [], true]]])
        );
        assert_eq!(d["substitution_names"], json!([["s", "s"]]));

        // (g) both citation references sit in `citation_refs` (which later
        // decide the citation's backrefs).
        let d = dump("Cite [CIT]_ and [CIT]_.\n\n.. [CIT] The citation.\n");
        let refs = json!([[
            "cit",
            [
                ["citation_reference", ["id1"], true],
                ["citation_reference", ["id2"], true]
            ]
        ]]);
        assert_eq!(d["citation_refs"], refs);
        assert_eq!(d["refnames"], refs);
        assert_eq!(d["citations"], json!([["citation", ["cit"], true]]));

        // (h) one list per footnote kind, each in registration order.
        let d = dump(".. [#] auto one\n.. [*] sym\n.. [1] manual\n.. [#b] auto labeled\n");
        assert_eq!(
            d["autofootnotes"],
            json!([["footnote", ["id1"], true], ["footnote", ["b"], true]])
        );
        assert_eq!(d["symbol_footnotes"], json!([["footnote", ["id2"], true]]));
        assert_eq!(d["footnotes"], json!([["footnote", ["id3"], true]]));
        assert_eq!(d["names"], json!([["1", "id3", true], ["b", "b", true]]));
    }

    /// `Body.substitution_def` keeps every child of the definition that is
    /// `nodes.Inline` or Text and hoists the rest to its parent
    /// (`SP/docutils/parsers/rst/states.py:2184-2191`). Sphinx's
    /// `pending_xref` is `Inline` (`SP/sphinx/addnodes.py:493`), so the
    /// `:ref:` of a `replace::` stays inside its definition, the content
    /// Substitutions (220) copies ([SQ §4.3]).
    //
    // oracle (Sphinx 9.1.0): the input read as `index.rst` through
    //   tools/gen_read_state_fixture.py's harness; no transform touches a
    //   definition without substitution references, so its post-read
    //   definitions are the parse's.
    #[test]
    fn substitution_definition_keeps_its_pending_xref() {
        let out = parse_rst_full(
            ".. |x| replace:: \"plain\" -- x\n\
             .. |y| replace:: :ref:`\"T\" -- t <nolabel>` and \"plain\"\n",
            &ParseOptions {
                source_path: "<SRCDIR>/index.rst".into(),
                sphinx: true,
                docname: "index".into(),
                ..Default::default()
            },
        );
        assert_eq!(
            out.doctree.root.pformat(),
            "<document source=\"<SRCDIR>/index.rst\">\n    \
             <substitution_definition names=\"x\">\n        \"plain\" -- x\n    \
             <substitution_definition names=\"y\">\n        <pending_xref refdoc=\"index\" \
             refdomain=\"std\" refexplicit=\"1\" reftarget=\"nolabel\" reftype=\"ref\" \
             refwarn=\"1\">\n            <inline classes=\"xref std std-ref\">\n                \
             \"T\" -- t\n         and \"plain\"\n"
        );
    }

    /// Every `Uid` stamped on a node in the subtree.
    fn uids_in(node: &Node, out: &mut Vec<Uid>) {
        out.extend(node.uid());
        for child in &node.children {
            uids_in(child, out);
        }
    }

    /// A footnote reference inside a rejected substitution definition is
    /// registered and lives on in `detached` (Review Focus 5;
    /// `state.rejected_subst_autofootnote` and `docutils-0575`): the
    /// definition the parser discarded is a detached root holding the
    /// registered node, and the root never reaches it.
    #[test]
    fn detached_registrations_survive_the_parse() {
        for (source, refs) in [
            (
                ".. |f| replace:: note [#]_\n\nText [#]_.\n\n.. [#] the footnote\n",
                json!([auto_ref("id1", false), auto_ref("id2", true)]),
            ),
            (
                ".. |f| replace:: note [#]_\n",
                json!([auto_ref("id1", false)]),
            ),
        ] {
            let out = sphinx_parse(source);
            let state = &out.state;
            assert_eq!(
                state.registries_dump(&out.doctree.root)["autofootnote_refs"],
                refs,
                "{source}"
            );
            let detached_ref = state.registries.autofootnote_refs[0];
            let [definition] = state.detached.as_slice() else {
                panic!("one detached root, got {:?}", state.detached);
            };
            assert_eq!(definition.kind, "substitution_definition");
            assert_eq!(definition.attrs.names, ["f"]);
            assert_eq!(definition.rawsource(), Some(".. |f| replace:: note [#]_"));
            let mut in_detached = Vec::new();
            uids_in(definition, &mut in_detached);
            assert_eq!(in_detached, [detached_ref], "{source}");
            let mut in_tree = Vec::new();
            uids_in(&out.doctree.root, &mut in_tree);
            assert!(!in_tree.contains(&detached_ref), "{source}");
            assert!(state.misplaced_registrations(&out.doctree.root).is_empty());
        }
    }

    /// Every way a directive drops content it already parsed keeps that
    /// content's registrations in `detached`: `replace` with more than a
    /// paragraph, a `table` whose widths do not fit, a `csv-table` failing
    /// after its title, `list-table`'s structure checks, a figure caption
    /// that is no paragraph (with the legend after it), a non-list
    /// `hlist`, a `:no-typesetting:` description, and the doc-field
    /// transformer (an unclaimed `:type:`, a `:param:` whose content has
    /// no text, a `:meta:` field, an unknown field name's first node).
    /// Each registry below is what a parse-only Sphinx 9.1.0 run records.
    #[test]
    fn dropped_content_keeps_its_registrations() {
        let detached_refs = |n: usize| {
            Value::Array(
                (1..=n)
                    .map(|i| auto_ref(&format!("id{i}"), false))
                    .collect(),
            )
        };
        for (source, refs) in [
            (".. |x| replace:: a [#]_\n\n   b\n", 1),
            (
                ".. table:: T [#]_\n   :widths: 1 2 3\n\n   ===  ===\n   a    b\n   ===  ===\n",
                1,
            ),
            (".. csv-table:: T [#]_\n   :header-rows: 5\n\n   a, b\n", 1),
            (".. list-table:: T [#]_\n\n   - a [#]_\n", 2),
            (".. list-table::\n\n   - - a [#]_\n   - - b\n     - c\n", 1),
            (".. figure:: a.png\n\n   - x [#]_\n\n   legend [#]_\n", 2),
            (".. hlist::\n\n   para [#]_\n", 1),
            (
                ".. py:function:: f()\n   :no-typesetting:\n\n   Body [#]_.\n",
                1,
            ),
            (
                ".. py:function:: f(x)\n\n   :type y: [#]_\n   :param x: [#]_\n   \
                 :meta m: [#]_\n   :notafield_: body\n",
                3,
            ),
        ] {
            let out = sphinx_parse(source);
            let d = out.state.registries_dump(&out.doctree.root);
            assert_eq!(d["autofootnote_refs"], detached_refs(refs), "{source}");
            assert!(
                out.state
                    .misplaced_registrations(&out.doctree.root)
                    .is_empty(),
                "{source}"
            );
        }
        let d = dump(
            ".. py:function:: f(x)\n\n   :type y: [#]_\n   :param x: [#]_\n   \
             :meta m: [#]_\n   :notafield_: body\n",
        );
        assert_eq!(
            d["refnames"],
            json!([["notafield", [["reference", [], false]]]])
        );
    }

    /// Content a directive drops that holds no registered or pending node
    /// is gone, as docutils garbage-collects it, even when a node in it
    /// has an identity for its location override alone (an image's
    /// `:target:` reference to a URI): its override goes with it. Each
    /// discarded subtree is judged on its own, so an image dropped beside
    /// a paragraph with a `[#]_` goes while the paragraph stays; a
    /// `:target:` naming a target is a registered reference (`refnames`)
    /// and keeps its subtree and its override.
    #[test]
    fn discarded_subtrees_without_registrations_are_dropped() {
        let image = |target: &str| format!("   .. image:: pic.png\n      :target: {target}\n");
        let uri = image("https://x/");
        let both_modes = [
            format!(".. table::\n\n{uri}"),
            format!(".. list-table::\n\n{uri}"),
            // The caption is no paragraph: the legend is dropped.
            format!(".. figure:: a.png\n\n   - x\n\n{uri}"),
        ];
        let sphinx_only = [
            format!(".. hlist::\n\n{uri}"),
            format!(".. py:function:: f()\n   :no-typesetting:\n\n{uri}"),
        ];
        let docutils = ParseOptions {
            source_path: "<snippet>".into(),
            ..Default::default()
        };
        let mut parses = Vec::new();
        for source in both_modes.iter().chain(&sphinx_only) {
            parses.push((sphinx_parse(source), source));
        }
        for source in &both_modes {
            parses.push((parse_rst_full(source, &docutils), source));
        }
        for (out, source) in parses {
            assert_eq!(
                out.state.next_uid, 1,
                "{source:?}: the reference has an identity"
            );
            assert!(
                out.state.detached.is_empty(),
                "{source:?}: {:?}",
                out.state.detached
            );
            assert!(out.state.loc_overrides.is_empty(), "{source:?}");
        }

        let out = sphinx_parse(&format!(".. hlist::\n\n{uri}\n   Para [#]_.\n"));
        let detached: Vec<&str> = out.state.detached.iter().map(|node| node.kind).collect();
        assert_eq!(detached, [kinds::PARAGRAPH]);
        assert!(out.state.loc_overrides.is_empty());
        assert!(out
            .state
            .misplaced_registrations(&out.doctree.root)
            .is_empty());

        let out = sphinx_parse(&format!(".. hlist::\n\n{}", image("bad_")));
        let [reference] = out.state.detached.as_slice() else {
            panic!("one detached root, got {:?}", out.state.detached);
        };
        assert_eq!(reference.kind, kinds::REFERENCE);
        let uid = reference.uid().expect("registered");
        let overridden: Vec<&Uid> = out.state.loc_overrides.keys().collect();
        assert_eq!(overridden, [&uid]);
    }

    /// docutils' `CSVTable.run` makes the title before it reads the data
    /// (`SP/docutils/parsers/rst/directives/tables.py:286-287`), so every
    /// data error drops a title whose references are already registered:
    /// the title's `[#]_` takes `id1`, detached, and the paragraph's
    /// reference the next id. Probed on Sphinx 9.1.0 (parse-only) for each
    /// early return: no content, content beside `:file:`, `:file:` beside
    /// `:url:`, and an unreadable `:file:`.
    #[test]
    fn csv_table_makes_its_title_before_reading_its_data() {
        for source in [
            ".. csv-table:: T [#]_\n\nText [#]_.\n",
            ".. csv-table:: T [#]_\n   :file: data.csv\n\n   a, b\n\nText [#]_.\n",
            ".. csv-table:: T [#]_\n   :file: missing.csv\n   :url: http://x/\n\nText [#]_.\n",
            ".. csv-table:: T [#]_\n   :file: missing.csv\n\nText [#]_.\n",
        ] {
            let out = sphinx_parse(source);
            assert_eq!(footnote_ref_tree_order(&out), ["id2"], "{source}");
            assert_eq!(
                out.state.registries_dump(&out.doctree.root)["autofootnote_refs"],
                json!([auto_ref("id1", false), auto_ref("id2", true)]),
                "{source}"
            );
            assert!(
                out.state
                    .misplaced_registrations(&out.doctree.root)
                    .is_empty(),
                "{source}"
            );
        }
    }

    // ------------------------------------------------------------------
    // the reporter log, locations and the final position ([DS §6, §8.2])
    // ------------------------------------------------------------------

    use crate::transforms::reporter::{render_records, LogRecord};
    use crate::transforms::state::{DocLoc, SourceRef};

    /// The reporter records of a parse: `(seq, level, loc, text)`.
    fn reporter_records(out: &ParseOutput) -> Vec<(u32, u8, DocLoc, String)> {
        out.state
            .log
            .iter()
            .filter_map(|record| match record {
                LogRecord::Reporter {
                    key,
                    level,
                    loc,
                    text,
                } => Some((key.seq, *level, *loc, text.clone())),
                LogRecord::Logger { .. } => None,
            })
            .collect()
    }

    /// Every `system_message` of the tree, in document order.
    fn tree_messages(out: &ParseOutput) -> Vec<&Node> {
        let mut found = Vec::new();
        all(&out.doctree.root, kinds::SYSTEM_MESSAGE, &mut found);
        found
    }

    /// The eleven corpus cases whose `Anonymous hyperlink mismatch` (a
    /// transform message with neither a node nor a line) prints the final
    /// state-machine position ([DS §6.2], [TW §5.3.4]), by their html
    /// oracle id: `docutils-NNNN` is the NNNNth case of
    /// `doctree_differential.json`. `None` prints `index.rst::`.
    const FINAL_POSITION_CASES: [(usize, &str, Option<u32>); 11] = [
        (
            120,
            ".. __: https://example.com/1\n\n__ https://example.com/2\n",
            None,
        ),
        (167, "__ https://x/\npara after\n", Some(3)),
        (218, "See word__ here.\n", Some(2)),
        (219, "See `some phrase`__ here.\n", Some(2)),
        (220, "Anon word__.\n", Some(2)),
        (242, "A |sub| B |sub2|_ C |sub3|__ D.\n", Some(2)),
        (
            603,
            ".. |s2| replace:: text\n\n|s2|_ and |s2|__ refs.\n",
            Some(4),
        ),
        (638, "__\n\npara\n", Some(4)),
        (639, "__ https://example.com/\n   path\n", None),
        (683, "__ http://x\u{1f}y\n", None),
        (700, ".. __ :\n", None),
    ];

    /// The position as a `DocLoc` of the document itself.
    fn in_document(line: Option<u32>) -> DocLoc {
        match line {
            Some(line) => DocLoc::at(0, line),
            None => DocLoc::default(),
        }
    }

    /// [DS §6.2]'s rows whose inputs are on record: the parser's final
    /// position for the eleven corpus cases, as docutils 0.22.4 reports it
    /// (`probe_final_line.py`, [TW §5.3.4]; re-probed in this task).
    #[test]
    fn final_position_matches_docutils_probe_table() {
        let corpus = fixture("doctree_differential.json");
        for (case, source, line) in FINAL_POSITION_CASES {
            assert_eq!(
                corpus["cases"][case - 1]["rst"].as_str(),
                Some(source),
                "docutils-{case:04} is the corpus input"
            );
            assert_eq!(
                sphinx_parse(source).state.final_position,
                in_document(line),
                "docutils-{case:04}: {source:?}"
            );
        }
    }

    /// CRLF line endings are line boundaries like LF (`string2lines`'
    /// `splitlines`), so the final position is the same.
    #[test]
    fn final_position_ignores_line_endings() {
        for (case, source, line) in FINAL_POSITION_CASES {
            let crlf = source.replace('\n', "\r\n");
            assert_eq!(
                sphinx_parse(&crlf).state.final_position,
                in_document(line),
                "docutils-{case:04} with CRLF: {crlf:?}"
            );
        }
    }

    /// The rule behind [DS §6.2]'s table, construct by construct: where
    /// docutils' top-level state machine is once the last construct is
    /// parsed. A construct it reads itself ends past the end (`(source, N +
    /// 1)`); a list, or an explicit construct whose block took the trailing
    /// blank lines, ends off it (`(None, None)`); a target, an anonymous
    /// target, an empty comment, a line block of two or more lines and a
    /// quoted literal block end at a blank line, so trailing blank lines
    /// put them past the end; a `::` paragraph with no literal block on the
    /// last line stays on it; a document ending inside an include ends on
    /// the include's padding line, `n + 4` for `n` included lines, whatever
    /// blank lines follow the include. Every row probed on docutils 0.22.4
    /// (`reporter.get_source_and_line(None)` after the parse).
    #[test]
    fn final_position_follows_the_state_machine() {
        let rows: [(&str, Option<u32>); 36] = [
            ("", None),
            ("\n\n", Some(3)),
            ("Para\n\n\n", Some(4)),
            ("Para::\n", Some(1)),
            ("Para::\n\n", Some(3)),
            ("Para::\n\n  code\n", Some(4)),
            ("Para::\n\n> a\n", None),
            ("Para::\n\n> a\n\n", Some(5)),
            ("Title\n=====\n", Some(3)),
            ("=====\nTitle\n=====\n", Some(4)),
            ("Para.\n\n----\n", Some(4)),
            ("  quote\n", Some(2)),
            (">>> x\n", Some(2)),
            ("+---+\n| a |\n+---+\n", Some(4)),
            ("=  =\na  b\n=  =\n", Some(4)),
            ("| a\n", Some(2)),
            ("| a\n| b\n", None),
            ("| a\n| b\n\n", Some(4)),
            ("- a\n\n\n", None),
            ("1. a\n", None),
            ("term\n  def\n\n", None),
            (":f: v\n\n", None),
            ("-a  opt\n", None),
            ("Title\n=====\n\n- a\n- b\n", None),
            (".. note:: x\n\n\n", None),
            (".. comment\n\n", None),
            (".. [1] fn\n\n", None),
            (".. |s| replace:: x\n\n", None),
            ("..\n", None),
            ("..\n\n", Some(3)),
            ("Para.\n\n.. _x:\n", None),
            ("Para.\n\n.. _x:\n\n\n", Some(6)),
            (".. _x: http://x\n   cont\n\n", Some(4)),
            ("__ http://x\n\n", Some(3)),
            (".. note:: x\n.. _a:\n\n", Some(4)),
            ("Para.\n\n.. _x:\n\nPara2\n", Some(6)),
        ];
        for (source, line) in rows {
            assert_eq!(
                sphinx_parse(source).state.final_position,
                in_document(line),
                "{source:?}"
            );
        }

        // A document ending inside an include ends on the padding docutils
        // inserts after the included lines, whatever they end with; one
        // ending after it ends in itself.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        for (name, text) in [
            ("para.rst", "Inc para.\n"),
            ("list.rst", "- a\n- b\n"),
            ("target.rst", ".. _t:\n"),
            ("inc.rst", "A.\n\nB.\n"),
        ] {
            std::fs::write(tmp.path().join(name), text).expect("write");
        }
        let parse_in = |source: &str| {
            parse_rst_full(
                source,
                &ParseOptions {
                    source_path: tmp.path().join("index.rst").display().to_string(),
                    sphinx: true,
                    srcdir: Some(tmp.path().to_path_buf()),
                    ..Default::default()
                },
            )
        };
        // docutils inserts an include's lines after the blank lines that
        // follow the directive, so blank lines after an include that ends
        // the document do not end it; nor does an include in a block quote.
        for (source, padding, line) in [
            ("Para.\n\n.. include:: para.rst\n", "para.rst", 5),
            ("Para.\n\n.. include:: list.rst\n", "list.rst", 6),
            ("Para.\n\n.. include:: target.rst\n", "target.rst", 5),
            ("Para.\n\n.. include:: para.rst\n\n", "para.rst", 5),
            ("Para.\n\n.. include:: para.rst\n\n\n", "para.rst", 5),
            ("Para.\n\n.. include:: list.rst\n\n", "list.rst", 6),
            ("Para.\n\n.. include:: target.rst\n\n", "target.rst", 5),
            ("`a`__\n\n.. include:: inc.rst\n", "inc.rst", 7),
            ("`a`__\n\n.. include:: inc.rst\n\n", "inc.rst", 7),
            ("`a`__\n\n   .. include:: inc.rst\n", "inc.rst", 7),
            ("`a`__\n\n   .. include:: inc.rst\n\n", "inc.rst", 7),
        ] {
            let out = parse_in(source);
            let position = out.state.final_position;
            let Some(SourceRef::Table(id)) = position.source else {
                panic!("{source:?}: no source in {position:?}");
            };
            assert_eq!(
                out.doctree.sources[usize::from(id)],
                format!("internal padding after {padding}"),
                "{source:?}"
            );
            assert_eq!(position.line, Some(line), "{source:?}");
        }
        let out = parse_in("Para.\n\n.. include:: para.rst\n\nAfter.\n");
        assert_eq!(out.state.final_position, DocLoc::at(0, 6));
    }

    /// The reporter logs a message's text as docutils writes it, at
    /// creation: `run_directive` appends a `DirectiveError`'s block to the
    /// message after the reporter created it (`states.py:2287-2291`), so
    /// the record holds the message alone while the node in the tree also
    /// holds the block (`docutils-0392`).
    #[test]
    fn reporter_records_creation_time_text() {
        let out = sphinx_parse(".. note::\n");
        let text = "Content block expected for the \"note\" directive; none found.";
        assert_eq!(
            reporter_records(&out),
            [(0, 3, DocLoc::at(0, 1), text.to_string())]
        );
        let [message] = tree_messages(&out)[..] else {
            panic!("one message in the tree");
        };
        assert_eq!(message.element_astext(), format!("{text}\n\n.. note::"));
    }

    /// An option error is a `MarkupError`, whose message docutils builds
    /// with the block as its child from the start (`states.py:2274-2281`),
    /// so the record holds the block too (`docutils-0391`; [TW §5.3.2]).
    #[test]
    fn option_errors_record_their_block() {
        let out = sphinx_parse(".. note::\n   :bogus: x\n\n   Body.\n");
        let text = "Error in \"note\" directive:\nunknown option: \"bogus\".\n\n\
                    .. note::\n   :bogus: x\n\n   Body.";
        assert_eq!(
            reporter_records(&out),
            [(0, 3, DocLoc::at(0, 1), text.to_string())]
        );
        let [message] = tree_messages(&out)[..] else {
            panic!("one message in the tree");
        };
        assert_eq!(message.element_astext(), text);
    }

    /// docutils checks a simple table row by row (its margins, then its
    /// column spans: `check_columns`, `init_row`) before `build_table`
    /// parses a cell (`tableparser.py:393-490`, `states.py:1799-1815`), so
    /// a malformed table parses none of its cells. The log holds the
    /// Malformed-table record alone, at `startline + offset` (the failing
    /// row's start plus one); a footnote reference in an earlier row's cell
    /// is never created, let alone registered; and a column span problem
    /// in one row is found before text in a later row's margin. Probed on
    /// docutils 0.22.4: each input prints its Malformed-table message at
    /// line 5 (`table line` 5, 5 and 3) and nothing else.
    #[test]
    fn malformed_simple_table_parses_no_cell() {
        let emphasis = "=====  =====\n*a     b\n=====  =====\nc      d\n--  --------\n=====  =====";
        let footnote = "=====  =====\n[#]_   b\n=====  =====\nc      d\n--  --------\n=====  =====";
        let margin_after = "=====  =====\n*a     b\n--  --------\nc     xd\n=====  =====";
        for sphinx in [false, true] {
            let options = ParseOptions {
                source_path: "<snippet>".into(),
                sphinx,
                ..Default::default()
            };
            for (before, block, after, table_line) in [
                ("", emphasis, "", 5),
                ("", footnote, "\n.. [#] Note.\n", 5),
                ("Para.\n\n", margin_after, "", 3),
            ] {
                // In a debug build, the end-of-parse assertion fails the
                // parse when a parsed cell's registration is dropped.
                let out = parse_rst_full(&format!("{before}{block}\n{after}"), &options);
                let text = format!(
                    "Malformed table.\nColumn span alignment problem in table line \
                     {table_line}.\n\n{block}"
                );
                assert_eq!(
                    reporter_records(&out),
                    [(0, 3, DocLoc::at(0, 5), text)],
                    "{block:?} (sphinx: {sphinx})"
                );
                assert_eq!(tree_messages(&out).len(), 1);
                assert!(out.state.registries.autofootnote_refs.is_empty());
                assert!(out
                    .state
                    .misplaced_registrations(&out.doctree.root)
                    .is_empty());
            }
        }
    }

    /// One counter numbers the parse's records in creation order, whatever
    /// their channel: a reporter message, a toctree warning, a logger
    /// warning. Swapping the inputs swaps the numbers.
    #[test]
    fn parse_records_share_the_seq_counter() {
        let found_docs = Some(std::sync::Arc::new(
            ["index".to_string()]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
        ));
        let parse_with_docs = |source: &str| {
            parse_rst_full(
                source,
                &ParseOptions {
                    found_docs: found_docs.clone(),
                    ..sphinx_options()
                },
            )
        };
        let toctree_seqs = |out: &ParseOutput| -> Vec<u32> {
            out.toctrees
                .iter()
                .flat_map(|toctree| toctree.warnings.iter().map(|warning| warning.seq))
                .collect()
        };
        let reporter_seqs = |out: &ParseOutput| -> Vec<u32> {
            reporter_records(out)
                .into_iter()
                .map(|(seq, ..)| seq)
                .collect()
        };

        let out = parse_with_docs("Title text\n====\n\n.. toctree::\n\n   missing\n");
        assert_eq!(
            reporter_records(&out)[0].3.lines().next(),
            Some("Title underline too short.")
        );
        assert_eq!(
            (reporter_seqs(&out), toctree_seqs(&out)),
            (vec![0], vec![1])
        );
        let out = parse_with_docs(".. toctree::\n\n   missing\n\nTitle text\n====\n");
        assert_eq!(
            (reporter_seqs(&out), toctree_seqs(&out)),
            (vec![1], vec![0])
        );

        // `Para` then `   indented.` is a definition list; a two-line
        // paragraph before the indented line is what raises `Unexpected
        // indentation.`.
        let out = sphinx_parse(".. option:: =bad\n\nPara\nmore\n   indented.\n");
        let logged: Vec<(u32, &str)> = out
            .registry
            .log_warnings
            .iter()
            .map(|warning| (warning.seq, warning.message.as_str()))
            .collect();
        assert_eq!(logged.len(), 1);
        assert!(logged[0]
            .1
            .starts_with("Malformed option description '=bad'"));
        assert_eq!(
            reporter_records(&out),
            [(
                1,
                3,
                DocLoc::at(0, 5),
                "Unexpected indentation.".to_string()
            )]
        );
        assert_eq!(logged[0].0, 0);
    }

    /// A registration record draws its seq where Sphinx registers, before
    /// the description's content is parsed: a `py:function` between two
    /// reporter messages numbers between them, and so does a glossary
    /// term (`GlossaryTermRecord`).
    #[test]
    fn registration_records_carry_parse_seq() {
        let out = sphinx_parse("*a\n\n.. py:function:: f()\n\n   *in content\n\n*b\n");
        let seqs: Vec<u32> = reporter_records(&out)
            .into_iter()
            .map(|(seq, ..)| seq)
            .collect();
        assert_eq!(seqs, [0, 2, 3]);
        let [record] = out.registry.py_objects.as_slice() else {
            panic!("one py object: {:?}", out.registry.py_objects);
        };
        assert_eq!((record.fullname.as_str(), record.seq), ("f", 1));

        let out = sphinx_parse("*a\n\n.. glossary::\n\n   term\n      def\n\n*b\n");
        assert_eq!(
            out.registry.glossary_terms,
            [GlossaryTermRecord {
                term: "term".to_string(),
                id: "term-term".to_string(),
                source: 0,
                line: 4,
                seq: 1,
            }]
        );
        let seqs: Vec<u32> = reporter_records(&out)
            .into_iter()
            .map(|(seq, ..)| seq)
            .collect();
        assert_eq!(seqs, [0, 2]);
    }

    /// The `reference` an image's `:target:` wraps is appended to its
    /// container right after the option block's nested state machine
    /// cleared the line, so docutils' `setup_child` gives it the source and
    /// no line when that container has `document` set, and neither when it
    /// has not ([DS §6.1]; `docutils-0439`). The attached containers are
    /// the document, a section, and those Sphinx attaches before it fills
    /// them: object description content, `py:module` content, `only`
    /// (inside a `note` too) and glossary definitions. A `note` (inside
    /// `only` too), a list item and a substitution definition are not, nor
    /// is a figure, which takes the reference before anything appends the
    /// figure. The default rule gives a node outside a TextElement its
    /// span's location ([DS §8.2] rule 4), which is neither, so both are
    /// recorded. Probed after the parse on docutils 0.22.4 (the rows of
    /// both modes) and by a priority-1 transform on Sphinx 9.1.0 (every
    /// row).
    #[test]
    fn loc_overrides_for_image_target_refs() {
        let source_only = DocLoc {
            source: Some(SourceRef::Table(0)),
            line: None,
        };
        let neither = DocLoc::default();
        // `directive` with a `:target:` after `prefix`, indented by `indent`.
        let target = |prefix: &str, indent: &str, directive: &str| {
            format!("{prefix}{indent}.. {directive} pic.png\n{indent}   :target: sometarget_\n")
        };
        let image = |prefix: &str, indent: &str| target(prefix, indent, "image::");
        let both_modes = [
            (image("", ""), source_only),
            (image("Title\n=====\n\n", ""), source_only),
            (image(".. note::\n\n", "   "), neither),
            (image("- item\n\n", "  "), neither),
            (target("", "", "figure::"), neither),
            (target("", "", "|x| image::"), neither),
        ];
        let sphinx_only = [
            (
                ".. image:: pic.png\n   :target: https://x/\n".to_string(),
                source_only,
            ),
            (image(".. py:function:: f()\n\n", "   "), source_only),
            (image(".. describe:: thing\n\n", "   "), source_only),
            (image(".. py:module:: m\n\n", "   "), source_only),
            (image(".. only:: html\n\n", "   "), source_only),
            (
                image(".. note::\n\n   .. only:: html\n\n", "      "),
                source_only,
            ),
            (image(".. glossary::\n\n   term\n", "      "), source_only),
            (
                image(".. only:: html\n\n   .. note::\n\n", "      "),
                neither,
            ),
            (
                target(".. py:function:: f()\n\n", "   ", "figure::"),
                neither,
            ),
        ];
        let docutils = ParseOptions {
            source_path: "<snippet>".into(),
            ..Default::default()
        };
        let mut parses = Vec::new();
        for (source, expected) in both_modes.iter().chain(&sphinx_only) {
            parses.push((sphinx_parse(source), source, expected));
        }
        for (source, expected) in &both_modes {
            parses.push((parse_rst_full(source, &docutils), source, expected));
        }
        // A glossary term and its index node, and a node a directive
        // returns without a location (an object description's `desc`),
        // have overrides of their own.
        fn glossary_uids(node: &Node, out: &mut Vec<crate::doctree::Uid>) {
            if matches!(node.kind, kinds::TERM | "index" | "desc") {
                out.extend(node.uid());
            }
            node.children
                .iter()
                .for_each(|child| glossary_uids(child, out));
        }
        for (out, source, expected) in parses {
            let reference = first(&out.doctree.root, kinds::REFERENCE).expect("a reference");
            let uid = reference
                .uid()
                .expect("an override gives the node an identity");
            let mut terms = Vec::new();
            glossary_uids(&out.doctree.root, &mut terms);
            let mut overrides = out.state.loc_overrides.clone();
            overrides.retain(|held, _| !terms.contains(held));
            assert_eq!(
                overrides,
                std::collections::HashMap::from([(uid, *expected)]),
                "{source:?}"
            );
        }
    }

    /// The locations a `parsed-literal`'s literal block and a `rubric` have
    /// once Sphinx's ApplySourceWorkaround (010) has run, which the default
    /// rule ([DS §8.2] rule 4: the directive's line) does not give: what
    /// the parse records for them (`DocumentState::loc_overrides`) as the
    /// location model completes it. A parsed-literal block keeps
    /// `node.line = self.content_offset + 1` (`body.py:144`), its first
    /// content line as the machine counts it, one high in a table cell,
    /// and takes its source from its container or from 010. docutils makes
    /// a rubric without a location, so it takes its container's
    /// `setup_child` location in a container that has `document` set (the
    /// document, a section, `only`, object description content, a glossary
    /// definition): where the machine reading its block stopped, its
    /// trailing blank lines included, or the source and no line after an
    /// option block. In any other container (a list item, a `note`, also
    /// inside `only`, a block quote, a definition) it has none until 010
    /// gives it its nearest located ancestor's source and line 0. A rubric
    /// in a substitution definition keeps the default rule, the directive's
    /// line. Probed on Sphinx 9.1.0: each node's `get_source_line` after
    /// the read.
    #[test]
    fn locations_of_parsed_literals_and_rubrics() {
        let source_only = DocLoc {
            source: Some(SourceRef::Table(0)),
            line: None,
        };
        let at = |line| DocLoc::at(0, line);
        let literal = kinds::LITERAL_BLOCK;
        let rubric = "rubric";
        let cell = "+------------------------+\n| .. parsed-literal::    |\n\
                    |                        |\n|    code bad_           |\n\
                    +------------------------+\n";
        let rows = [
            (
                literal,
                "Para.\n\n.. parsed-literal::\n\n   code bad_\n",
                at(5),
            ),
            (
                literal,
                "- item\n\n  .. parsed-literal::\n\n     code bad_\n",
                at(5),
            ),
            (
                literal,
                "Para.\n\n.. parsed-literal::\n   :class: x\n\n   code bad_\n",
                at(6),
            ),
            (
                literal,
                ".. note::\n\n   .. parsed-literal::\n\n      code bad_\n",
                at(5),
            ),
            (
                literal,
                "Para.\n\n.. parsed-literal::\n   code bad_\n",
                at(4),
            ),
            (literal, ".. parsed-literal:: code bad_\n", at(1)),
            (literal, cell, at(5)),
            (rubric, ".. rubric:: Rubric bad_\n", at(1)),
            (rubric, ".. rubric:: Rubric bad_\n\nPara.\n", at(2)),
            (rubric, ".. rubric:: Rubric bad_\n\n\n\nPara.\n", at(4)),
            (rubric, ".. rubric:: R\n   bad_ long\n\nPara.\n", at(3)),
            (
                rubric,
                "Title\n=====\n\nSub\n---\n\n.. rubric:: Rubric bad_\n\n\nPara.\n",
                at(9),
            ),
            (
                rubric,
                ".. rubric:: Rubric bad_\n   :class: x\n\nPara.\n",
                source_only,
            ),
            (
                rubric,
                ".. only:: html\n\n   .. rubric:: Rubric bad_\n\n   Para.\n",
                at(4),
            ),
            (
                rubric,
                ".. only:: html\n\n   .. rubric:: Rubric bad_\n\n\nAfter.\n",
                at(3),
            ),
            (
                rubric,
                ".. py:function:: f()\n\n   .. rubric:: Rubric bad_\n\n   Text.\n",
                at(4),
            ),
            (
                rubric,
                ".. py:function:: f()\n\n   .. rubric:: Rubric bad_\n      :class: x\n\n   \
                 Text.\n",
                source_only,
            ),
            (
                rubric,
                ".. glossary::\n\n   term\n      .. rubric:: R bad_\n\n      Text.\n",
                at(5),
            ),
            (
                rubric,
                ".. note::\n\n   .. only:: html\n\n      .. rubric:: R bad_\n\n      Text.\n",
                at(6),
            ),
            (
                rubric,
                "- item\n\n  .. rubric:: Rubric bad_\n\n  Para.\n",
                at(0),
            ),
            (
                rubric,
                ".. note::\n\n   .. rubric:: Rubric bad_\n\n   Para.\n",
                at(0),
            ),
            (
                rubric,
                "Para.\n\n   .. rubric:: Rubric bad_\n\n   Text.\n",
                at(0),
            ),
            (rubric, "term\n   .. rubric:: R bad_\n\n   Text.\n", at(0)),
            (
                rubric,
                ".. only:: html\n\n   .. note::\n\n      .. rubric:: R bad_\n\n      Text.\n",
                at(0),
            ),
            (rubric, ".. |s| rubric:: R bad_\n\nPara |s|.\n", at(1)),
        ];
        for (kind, source, expected) in rows {
            let ParseOutput {
                doctree, mut state, ..
            } = sphinx_parse(source);
            let tree = crate::transforms::tree::TxTree::from_parse(doctree, &mut state);
            let node = tree.findall(tree.root(), |node| node.kind == kind)[0];
            let found = crate::transforms::location::get_source_line(&tree, node, &state);
            assert_eq!(found, expected, "{source:?}");
        }
    }

    /// Every duplicate name `NameTable::bind` reports is logged at
    /// creation: two explicit targets give one WARNING, two sections an
    /// INFO that never prints. The index directive notes its target with
    /// no `msgnode`, so its message is in no tree, and is still printed,
    /// at the state machine's line. All three probed on Sphinx 9.1.0: the
    /// WARNING at line 5, the INFO at line 7, and `index.rst:6: WARNING:
    /// Duplicate explicit target name: "a". [docutils]` with both targets
    /// `dupnames="a"` and the index target `ids="id1"`.
    #[test]
    fn duplicate_name_messages_are_logged() {
        let out = sphinx_parse(".. _a:\n\nPara.\n\n.. _a:\n\nPara2.\n");
        assert_eq!(
            reporter_records(&out),
            [(
                0,
                2,
                DocLoc::at(0, 5),
                "Duplicate explicit target name: \"a\".".to_string()
            )]
        );

        let out = sphinx_parse("Title\n=====\n\nText.\n\nTitle\n=====\n\nMore.\n");
        let records = reporter_records(&out);
        assert_eq!(
            records,
            [(
                0,
                1,
                DocLoc::at(0, 7),
                "Duplicate implicit target name: \"title\".".to_string()
            )]
        );
        assert!(render_records(out.state.log, &out.doctree.sources, "<snippet>").is_empty());

        let out = parse_rst_full(
            ".. _a:\n\nText.\n\n.. index:: x\n   :name: a\n",
            &ParseOptions {
                source_path: "/src/index.rst".into(),
                ..sphinx_options()
            },
        );
        let printed: Vec<String> = render_records(
            out.state.log.clone(),
            &out.doctree.sources,
            "/src/index.rst",
        )
        .into_iter()
        .map(|(_, diagnostic)| diagnostic.render(true))
        .collect();
        assert_eq!(
            printed,
            ["/src/index.rst:6: WARNING: Duplicate explicit target name: \"a\". [docutils]"]
        );
        assert!(tree_messages(&out).is_empty());
        let mut targets = Vec::new();
        all(&out.doctree.root, kinds::TARGET, &mut targets);
        let shapes: Vec<(&[String], &[String])> = targets
            .iter()
            .map(|target| {
                (
                    target.attrs.ids.as_slice(),
                    target.attrs.dupnames.as_slice(),
                )
            })
            .collect();
        let (a, id1, dup) = ("a".to_string(), "id1".to_string(), "a".to_string());
        assert_eq!(
            shapes,
            [
                (std::slice::from_ref(&a), std::slice::from_ref(&dup)),
                (std::slice::from_ref(&id1), std::slice::from_ref(&dup)),
            ]
        );
    }

    /// docutils appends a duplicate name's message to the named node and
    /// detaches it again when the node's content model rejects it; Sphinx
    /// never places it again, so it is printed and absent from the tree
    /// ([TW A10]). An image rejects it; a `note` keeps it (probed on
    /// Sphinx 9.1.0: the note's first child), so it stays in the tree.
    #[test]
    fn detached_duplicate_name_message_is_logged_not_inserted() {
        let text = "Duplicate explicit target name: \"x\".".to_string();
        let out = sphinx_parse(".. note:: b\n   :name: x\n\n.. image:: a.png\n   :name: x\n");
        assert_eq!(
            reporter_records(&out),
            [(0, 2, DocLoc::at(0, 4), text.clone())]
        );
        assert!(
            tree_messages(&out).is_empty(),
            "{}",
            out.doctree.root.pformat()
        );
        let image = first(&out.doctree.root, kinds::IMAGE).expect("the image");
        assert_eq!(image.attrs.dupnames, ["x"]);

        let out = sphinx_parse(".. image:: a.png\n   :name: x\n\n.. note:: b\n   :name: x\n");
        assert_eq!(reporter_records(&out).len(), 1);
        assert_eq!(tree_messages(&out).len(), 1);
    }

    /// docutils `dupname`s the earlier node object wherever it lives
    /// (T5b review): a target or a substitution definition in a subtree
    /// the parse discarded loses the name a later node duplicates. Probed
    /// on Sphinx 9.1.0 (parse-only, the noted nodes' attributes): the
    /// rejected definition's embedded-alias target `x` (an indirect
    /// target, so its subtree is kept) gives its name up to the later
    /// explicit `.. _x:`, which keeps it as `id1`, and the rejected table
    /// content's `|s|` gives it up to the later definition.
    #[test]
    fn detached_nodes_lose_duplicated_names() {
        let out = sphinx_parse(".. |f| replace:: `x <a_>`_\n\n.. _x:\n\nText.\n");
        let mut detached_targets = Vec::new();
        for node in &out.state.detached {
            all(node, kinds::TARGET, &mut detached_targets);
        }
        let [target] = detached_targets.as_slice() else {
            panic!("one detached target: {:?}", out.state.detached);
        };
        assert_eq!(
            (
                target.attrs.names.as_slice(),
                target.attrs.dupnames.as_slice()
            ),
            (&[][..], &["x".to_string()][..])
        );
        let new = first(&out.doctree.root, kinds::TARGET).expect("in the tree");
        assert_eq!(
            (new.attrs.ids.as_slice(), new.attrs.names.as_slice()),
            (&["id1".to_string()][..], &["x".to_string()][..])
        );

        let out = sphinx_parse(
            ".. table:: T\n\n   .. |s| replace:: a\n\n   not a table\n\n\
             .. |s| replace:: b\n\nUse |s|.\n",
        );
        let mut detached_definitions = Vec::new();
        for node in &out.state.detached {
            all(node, "substitution_definition", &mut detached_definitions);
        }
        let [old] = detached_definitions.as_slice() else {
            panic!("one detached definition: {:?}", out.state.detached);
        };
        assert_eq!(
            (old.attrs.names.as_slice(), old.attrs.dupnames.as_slice()),
            (&[][..], &["s".to_string()][..])
        );
        let new = first(&out.doctree.root, "substitution_definition").expect("in the tree");
        assert_eq!(new.attrs.names, ["s"]);
        assert!(new.attrs.dupnames.is_empty());
    }

    /// An indirect target is registered, so its identity comes before its
    /// first `set_id`: the id is held by that identity, and the same node
    /// registering it again is not a duplicate (`IdAllocator::register`).
    #[test]
    fn indirect_targets_hold_their_ids_by_identity() {
        for (source, id) in [(".. _a: x_\n", "a"), ("__ x_\n", "id1")] {
            let mut out = sphinx_parse(source);
            let target = first(&out.doctree.root, kinds::TARGET).expect("a target");
            let uid = target.uid().expect("registered");
            assert_eq!(target.attrs.ids, [id], "{source:?}");
            assert!(out.state.ids.register(id, Some(uid)), "{source:?}");
        }
    }

    /// One row per `LogLocation` rule ([TW §5.4]): Sphinx's
    /// `doc2path(docname)` is `<srcdir>/<docname>.rst`; a tuple whose first
    /// element is a source path goes through `doc2path` too, doubling the
    /// suffix and putting a relative path under the source directory; a
    /// node location is the source made absolute, `path:` without a line.
    /// A name joined under the source directory takes the platform's
    /// separator (`Path::join`, `/src\index.rst` on Windows); a rooted
    /// table path replaces the source directory and keeps its spelling.
    #[test]
    fn log_locations_render_like_sphinx() {
        use crate::diagnostics::Location;
        let srcdir = std::path::Path::new("/src");
        let sources = ["/src/index.rst".to_string(), "part.rst".to_string()];
        let path = |path: &str, line: Option<u32>| Location::Path {
            path: path.to_string(),
            line,
        };
        let under = |name: &str| srcdir.join(name).display().to_string();
        let rows = [
            (LogLocation::None, Location::None),
            (LogLocation::Doc, path(&under("index.rst"), None)),
            (LogLocation::DocLine(7), path(&under("index.rst"), Some(7))),
            (LogLocation::DocLine(0), path(&under("index.rst"), None)),
            (
                LogLocation::Node { source: 0, line: 3 },
                path("/src/index.rst", Some(3)),
            ),
            (
                LogLocation::Node { source: 1, line: 6 },
                path(&under("part.rst"), Some(6)),
            ),
            (
                LogLocation::Node { source: 1, line: 0 },
                Location::Verbatim(format!("{}:", under("part.rst"))),
            ),
            (
                LogLocation::Tuple { source: 0, line: 1 },
                path("/src/index.rst.rst", Some(1)),
            ),
            (
                LogLocation::Tuple { source: 1, line: 6 },
                path(&under("part.rst.rst"), Some(6)),
            ),
        ];
        for (location, expected) in rows {
            assert_eq!(
                location.render(&sources, srcdir, "index"),
                expected,
                "{location:?}"
            );
        }
        // The record a `ParseLogWarning` becomes: a `WARNING` with no
        // category, so no suffix.
        let warning = ParseLogWarning {
            message: "non-whitespace stripped by dedent".to_string(),
            location: LogLocation::Tuple {
                source: 1,
                line: 12,
            },
            seq: 0,
        };
        assert_eq!(
            warning.diagnostic(&sources, srcdir, "index").render(true),
            format!(
                "{}:12: WARNING: non-whitespace stripped by dedent",
                under("part.rst.rst")
            )
        );
    }

    /// A node location is the source made absolute as `os.path.abspath`
    /// makes it ([`node_source_path`]): joined under the source directory,
    /// with `.` and `..` collapsed lexically.
    #[test]
    fn node_locations_collapse_dot_segments_like_abspath() {
        let root = std::env::temp_dir().join("sphinx-ultra-abspath");
        let srcdir = root.join("src");
        let spelled = |path: std::path::PathBuf| path.display().to_string();
        // Probed on Sphinx 9.1.0: `.. include:: ../outside.inc` and an
        // undefined label on its line 3 warn `<root>/outside.inc:3`.
        assert_eq!(
            node_source_path("../outside.inc", &srcdir),
            spelled(root.join("outside.inc"))
        );
        assert_eq!(
            node_source_path("sub/./../part.rst", &srcdir),
            spelled(srcdir.join("part.rst"))
        );
        assert_eq!(
            node_source_path("./part.rst", &srcdir),
            spelled(srcdir.join("part.rst"))
        );
        // Without a dot segment the path is the plain join, and a path
        // already under the source directory keeps its spelling.
        assert_eq!(
            node_source_path("sub/part.rst", &srcdir),
            spelled(srcdir.join("sub/part.rst"))
        );
        let own = spelled(srcdir.join("index.rst"));
        assert_eq!(node_source_path(&own, &srcdir), own);
    }

    /// A read with no project renders against a stand-in source directory
    /// whose document path is not rooted (`<SRCDIR>/index.rst`): a path
    /// already under it stands alone, as the rooted path it stands for
    /// does in a build, while a name is still joined under it.
    #[test]
    fn log_locations_render_under_a_stand_in_source_directory() {
        use crate::diagnostics::Location;
        let srcdir = std::path::Path::new("<SRCDIR>");
        let sources = ["<SRCDIR>/index.rst".to_string(), "part.rst".to_string()];
        let path = |path: &str, line: Option<u32>| Location::Path {
            path: path.to_string(),
            line,
        };
        let under = |name: &str| srcdir.join(name).display().to_string();
        for (location, expected) in [
            (
                LogLocation::Node { source: 0, line: 3 },
                path("<SRCDIR>/index.rst", Some(3)),
            ),
            (
                LogLocation::Tuple { source: 0, line: 1 },
                path("<SRCDIR>/index.rst.rst", Some(1)),
            ),
            (
                LogLocation::Node { source: 1, line: 6 },
                path(&under("part.rst"), Some(6)),
            ),
            (LogLocation::DocLine(2), path(&under("index.rst"), Some(2))),
        ] {
            assert_eq!(
                location.render(&sources, srcdir, "index"),
                expected,
                "{location:?}"
            );
        }
    }

    /// The first line of each record a Sphinx-mode parse of `source` logs
    /// that Sphinx prints, read as `index.rst`: the form the probes below
    /// print Sphinx 9.1.0's stream in, the source directory left out.
    fn printed_heads(source: &str) -> Vec<String> {
        printed_heads_with(
            source,
            &ParseOptions {
                source_path: "index.rst".into(),
                ..sphinx_options()
            },
        )
    }

    /// [`printed_heads`] of a parse with `options`.
    fn printed_heads_with(source: &str, options: &ParseOptions) -> Vec<String> {
        let out = parse_rst_full(source, options);
        render_records(out.state.log, &out.doctree.sources, &options.source_path)
            .into_iter()
            .map(|(_, diagnostic)| {
                let rendered = diagnostic.render(true);
                rendered.lines().next().unwrap_or_default().to_string()
            })
            .collect()
    }

    const EMPHASIS: &str = "WARNING: Inline emphasis start-string without end-string. [docutils]";

    fn at(line: u32, record: &str) -> String {
        format!("index.rst:{line}: {record}")
    }

    /// docutils' `set_id` reports a node whose preset id another node
    /// holds (`SP/docutils/nodes.py:1832-1843`): `Duplicate ID` with both
    /// nodes' start tags, at the node's location, which for a node in no
    /// tree yet is the top-level machine's position. Sphinx's `index`
    /// directive presets `index-N` without checking `document.ids`
    /// (`SP/sphinx/domains/index.py:76-85`). Probed on Sphinx 9.1.0
    /// (`-w`).
    #[test]
    fn a_preset_id_another_node_holds_is_a_duplicate_id() {
        let duplicate = |line: u32, id: &str, holder: &str| {
            at(
                line,
                &format!(
                    "ERROR: Duplicate ID: \"{id}\" used by {holder} and \
                     <target ids=\"{id}\"> [docutils]"
                ),
            )
        };
        assert_eq!(
            printed_heads("Index 0\n=======\n\n.. index:: foo\n\nText.\n"),
            [duplicate(
                5,
                "index-0",
                "<section ids=\"index-0\" names=\"index\\ 0\">"
            )]
        );
        assert_eq!(
            printed_heads(".. _index-0:\n\nPara.\n\n.. index:: foo\n"),
            [duplicate(
                5,
                "index-0",
                "<target ids=\"index-0\" names=\"index-0\">"
            )]
        );
        assert_eq!(
            printed_heads("Title\n=====\n\n.. index:: a\n\nIndex 1\n-------\n\n.. index:: b\n"),
            [duplicate(
                9,
                "index-1",
                "<section ids=\"index-1\" names=\"index\\ 1\">"
            )]
        );
        assert_eq!(
            printed_heads("Index 0\n=======\n\n.. note::\n\n   .. index:: foo\n\n   Text.\n"),
            [duplicate(
                8,
                "index-0",
                "<section ids=\"index-0\" names=\"index\\ 0\">"
            )]
        );
    }

    /// The node holding the id may sit in content the parse rejected (a
    /// malformed `list-table`), which no tree keeps: docutils' `document.ids`
    /// still holds the node, and `Duplicate ID` spells its start tag as the
    /// node is. Probed on Sphinx 9.1.0 (`-w`).
    #[test]
    fn a_duplicate_id_names_a_holder_in_rejected_content() {
        let rejected = |cell: &str| {
            format!("Title\n=====\n\n.. list-table::\n\n   * - {cell}\n     - b\n   * - c\n\n.. index:: foo\n")
        };
        let table = at(
            4,
            "ERROR: Error parsing content block for the \"list-table\" directive: uniform \
             two-level bullet list expected, but row 2 does not contain the same number of items \
             as row 1 (1 vs 2).",
        );
        let duplicate = |line: u32, holder: &str| {
            at(
                line,
                &format!(
                    "ERROR: Duplicate ID: \"index-0\" used by {holder} and \
                     <target ids=\"index-0\"> [docutils]"
                ),
            )
        };
        assert_eq!(
            printed_heads(&rejected(".. _index-0:\n\n       para")),
            [
                table.clone(),
                duplicate(11, "<target ids=\"index-0\" names=\"index-0\">")
            ]
        );
        assert_eq!(
            printed_heads(&rejected(".. image:: x.png\n          :name: index-0")),
            [
                table,
                duplicate(
                    10,
                    "<image ids=\"index-0\" names=\"index-0\" uri=\"x.png\">"
                )
            ]
        );
    }

    /// docutils' `anonymous` transition is `__( +|$)`
    /// (`SP/docutils/parsers/rst/states.py`, `Body.patterns`): a bare
    /// `__` is an anonymous target, so explicit markup goes on through it
    /// and only the line after it, `After.`, ends the markup without a
    /// blank line. Probed on Sphinx 9.1.0 (`-w`): nothing for the first
    /// input, one warning at line 9 for the second.
    #[test]
    fn a_bare_anonymous_target_goes_on_with_explicit_markup() {
        let unindent =
            "WARNING: Explicit markup ends without a blank line; unexpected unindent. [docutils]";
        assert_eq!(
            printed_heads(
                "Title\n=====\n\nPara `a`_ and `b`__.\n\n.. _a: http://x\n__\n\nAfter.\n"
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            printed_heads(
                "Title\n=====\n\nPara `b`__ x.\n\n.. _a: http://x\n__ http://y\n__\nAfter.\n"
            ),
            [at(9, unindent)]
        );
    }

    /// docutils parses each line of a line block at its own line
    /// (`LineBlock.line_block`, `states.py:1736-1742`), a continuation line
    /// joining its `|` line's text. Probed on Sphinx 9.1.0: lines 5, 5, 8.
    #[test]
    fn line_block_lines_report_at_their_own_lines() {
        assert_eq!(
            printed_heads("Title\n=====\n\n| LB one\n| LB two *a\n  continued *b\n\n| solo *c\n"),
            [at(5, EMPHASIS), at(5, EMPHASIS), at(8, EMPHASIS)]
        );
    }

    /// The `line-block` directive parses content line `i` at
    /// `self.lineno + self.content_offset`, the directive's 1-based line
    /// plus the content's 0-based offset, counting up per line
    /// (`LineBlock.run`, `SP/docutils/parsers/rst/directives/body.py:116-128`).
    /// Probed on Sphinx 9.1.0: the directive on line 3, its lines on 5 and
    /// 6, print 7 and 8.
    #[test]
    fn line_block_directive_reports_at_lineno_plus_content_offset() {
        assert_eq!(
            printed_heads("Para.\n\n.. line-block::\n\n   one *a\n   two *b\n"),
            [at(7, EMPHASIS), at(8, EMPHASIS)]
        );
    }

    /// Sphinx parses a glossary term at its content item's 0-based offset
    /// (`Glossary.run`, `SP/sphinx/domains/std/__init__.py:518-521`), one
    /// line before the term's own. Probed on Sphinx 9.1.0: terms on lines 3
    /// and 7 print 2 and 6.
    #[test]
    fn glossary_term_messages_take_the_zero_based_offset() {
        assert_eq!(
            printed_heads(
                ".. glossary::\n\n   term *a\n      Def.\n\n   other\n   second *b\n      Def.\n"
            ),
            [at(2, EMPHASIS), at(6, EMPHASIS)]
        );
    }

    /// A grid or simple table cell is parsed at docutils' `tableline +
    /// offset` (`build_table_row`, `states.py:1951-1952`), one past the
    /// 0-based offset it stands for, so every line its machine counts —
    /// a paragraph's inline messages, a nested caption's, each line of a
    /// line block, an explicit target's, a line block's `Line block ends
    /// without a blank line.` (`reporter.warning(..., line=lineno+1)`,
    /// `states.py:1727-1729`), and every message located at
    /// `abs_line_number()`: the unindent warning (`Explicit markup ends
    /// without a blank line`), a quoted literal block's messages,
    /// `Unexpected section title or transition.`, an explicit construct's
    /// `MarkupError`, a nested table's `Malformed table.` and `Blank line
    /// required after table.` — prints one high, while a paragraph's
    /// `Unexpected indentation.` takes the true line from the cell's own
    /// lines (`get_text_block`). A csv-table cell is parsed at
    /// `self.content_offset`, the first content line, whichever row (the
    /// `:header:` rows included) holds it. Probed on Sphinx 9.1.0.
    #[test]
    fn table_cell_messages_take_docutils_cell_offset() {
        let indentation = "ERROR: Unexpected indentation. [docutils]";
        let duplicate = "WARNING: Duplicate explicit target name: \"t\". [docutils]";
        let explicit =
            "WARNING: Explicit markup ends without a blank line; unexpected unindent. [docutils]";
        let quoting = "ERROR: Inconsistent literal block quoting. [docutils]";
        let none_found = "WARNING: Literal block expected; none found. [docutils]";
        let transition = "ERROR: Unexpected section title or transition.";
        let substitution = "WARNING: malformed substitution definition. [docutils]";
        let line_block = "WARNING: Line block ends without a blank line. [docutils]";
        let malformed = "ERROR: Malformed table.";
        let blank = "WARNING: Blank line required after table. [docutils]";
        for (source, expected) in [
            (
                "+-----------+\n| Cell *a   |\n|           |\n| Para      |\n| more      |\n\
                 |    ind.   |\n+-----------+\n",
                vec![at(3, EMPHASIS), at(6, indentation)],
            ),
            (
                "=====  =====\nA      B\n=====  =====\n*c     d\n=====  =====\n",
                vec![at(5, EMPHASIS)],
            ),
            (
                "+------------------------+\n| .. figure:: pic.png    |\n\
                 |                        |\n|    Caption *x          |\n\
                 +------------------------+\n",
                vec![at(5, EMPHASIS)],
            ),
            (
                "+-----------+\n| | one     |\n| | two *x  |\n+-----------+\n",
                vec![at(4, EMPHASIS)],
            ),
            (
                "Para.\n\n.. csv-table::\n   :header: \"h *h\", b\n\n   \"cell *a\", b\n\
                 \x20  \"c2 *b\", d\n",
                vec![at(6, EMPHASIS), at(6, EMPHASIS), at(6, EMPHASIS)],
            ),
            // `add_target`'s `target.line = lineno`: the machine's count.
            (
                ".. _t:\n\nText.\n\n+-----------------+\n| .. _t:          |\n\
                 |                 |\n| x               |\n+-----------------+\n",
                vec![at(7, duplicate)],
            ),
            (
                ".. _t:\n\nText.\n\n=====  ============\nA      B\n=====  ============\n\
                 1      .. _t:\n2      x\n=====  ============\n",
                vec![at(9, duplicate)],
            ),
            // `unindent_warning`: `abs_line_number() + 1`.
            (
                "+--------------+\n| Para.        |\n|              |\n| .. comment   |\n\
                 | Text.        |\n+--------------+\n",
                vec![at(6, explicit)],
            ),
            (
                "=====  ============\nA      B\n=====  ============\n1      .. comment\n\
                 \x20      Text.\n=====  ============\n",
                vec![at(6, explicit)],
            ),
            // `QuotedLiteralBlock`: its own machine's `abs_line_number()`.
            (
                "+---------------+\n| Para::        |\n|               |\n| > quoted      |\n\
                 |    indented   |\n+---------------+\n",
                vec![at(6, indentation)],
            ),
            (
                "+------------+\n| Para::     |\n|            |\n| > quoted   |\n| text       |\n\
                 +------------+\n",
                vec![at(6, quoting)],
            ),
            (
                "+----------+\n| Para::   |\n|          |\n| text     |\n+----------+\n",
                vec![at(5, none_found)],
            ),
            (
                "+----------+\n| Para::   |\n+----------+\n\nAfter.\n",
                vec![at(4, none_found)],
            ),
            // `Body.line`: `abs_line_number()`.
            (
                "+----------+\n| Text.    |\n|          |\n| ------   |\n+----------+\n",
                vec![at(5, transition)],
            ),
            (
                "=====  ========\nA      B\n=====  ========\n1      Text.\n\n       ~~~~~~\n\
                 =====  ========\n",
                vec![at(7, transition)],
            ),
            // `explicit_construct`'s `MarkupError`: `abs_line_number()`.
            (
                "+-----------+\n| Para.     |\n|           |\n| .. _foo   |\n|           |\n\
                 | Text.     |\n+-----------+\n",
                vec![at(5, "WARNING: malformed hyperlink target. [docutils]")],
            ),
            (
                "+-----------+\n| Para.     |\n|           |\n| .. |foo   |\n|           |\n\
                 | Text.     |\n+-----------+\n",
                vec![at(6, substitution)],
            ),
            // `line_block`: `lineno + 1`.
            (
                "+---------+\n| | one   |\n| Text.   |\n+---------+\n",
                vec![at(4, line_block)],
            ),
            // A nested table: `malformed_table`'s `startline + offset` and
            // `table_top`'s `abs_line_number() + 1`.
            (
                "+-----------+\n| +-----+   |\n| | a   |   |\n| +-----+   |\n| Text.     |\n\
                 +-----------+\n",
                vec![at(6, blank)],
            ),
            (
                "+------------+\n| +-----+    |\n| | a    |   |\n| +-----+    |\n+------------+\n",
                vec![at(4, malformed)],
            ),
            (
                "+----------------+\n| =====  =====   |\n| a      b       |\n| ====  =====    |\n\
                 | Text.          |\n+----------------+\n",
                vec![at(5, malformed), at(6, blank)],
            ),
        ] {
            assert_eq!(printed_heads(source), expected, "{source:?}");
        }
    }

    /// `line_block` warns `Line block ends without a blank line.` at
    /// `lineno + 1`, one past the machine's count for the block's FIRST
    /// line (`states.py:1708-1730`), however many lines the block has. A
    /// `malformed substitution definition.` is located where
    /// `substitution_def`'s `get_first_known_indented` left the machine
    /// (`explicit_construct`, `states.py:2492-2505`): on the last line of
    /// the definition's block, its trailing blank lines included. Probed on
    /// Sphinx 9.1.0.
    #[test]
    fn line_block_end_and_malformed_substitution_take_docutils_lines() {
        let line_block = "WARNING: Line block ends without a blank line. [docutils]";
        let substitution = "WARNING: malformed substitution definition. [docutils]";
        for (source, expected) in [
            ("| one\nText.\n", at(2, line_block)),
            ("| one\n| two\n| three\nText.\n", at(2, line_block)),
            ("| one\n  cont\n| two\nText.\n", at(2, line_block)),
            (".. |foo\n\nText.\n", at(2, substitution)),
            (".. |foo\n   bar\n\n\nText.\n", at(4, substitution)),
            (".. |foo\n", at(1, substitution)),
        ] {
            assert_eq!(printed_heads(source), [expected], "{source:?}");
        }
    }

    /// `add_name`'s duplicate-name message is located at the named node
    /// when the directive gave the node its location first — the
    /// directive's line — and otherwise at docutils' state-machine
    /// position: a top-level directive's block end, its trailing blank
    /// lines included. A `parsed-literal` node has only `line =
    /// content_offset + 1`, its first content line, which a table cell's
    /// machine counts one high. Probed on Sphinx 9.1.0 for every directive
    /// kind that takes `:name:`.
    #[test]
    fn duplicate_name_messages_follow_the_node_location_rule() {
        let duplicate = "WARNING: Duplicate explicit target name: \"a\". [docutils]";
        let head = ".. _a:\n\nText.\n\n";
        for (body, line) in [
            (".. note:: b\n   :name: a\n\n\n\nAfter.\n", 9),
            (".. seealso:: Body.\n   :name: a\n\nAfter.\n", 7),
            (
                ".. admonition:: Adm\n   :name: a\n\n   Body.\n\nAfter.\n",
                9,
            ),
            (".. rubric:: R\n   :name: a\n\nAfter.\n", 7),
            (
                ".. parsed-literal::\n   :name: a\n\n   lit one\n\nAfter.\n",
                8,
            ),
            (
                "+------------------------+\n| .. parsed-literal::    |\n\
                 |    :name: a            |\n|                        |\n\
                 |    lit one             |\n+------------------------+\n\nAfter.\n",
                10,
            ),
            (
                ".. code-block:: python\n   :name: a\n\n   pass\n\nAfter.\n",
                5,
            ),
            (
                ".. code-block:: python\n   :name: a\n   :caption: Cap\n\n   pass\n\nAfter.\n",
                10,
            ),
            (".. code:: python\n   :name: a\n\n   pass\n\nAfter.\n", 9),
            (".. math::\n   :name: a\n\n   a = b\n\nAfter.\n", 9),
            (
                ".. table:: T\n   :name: a\n\n   ===  ===\n   a    b\n   ===  ===\n\nAfter.\n",
                11,
            ),
            (".. csv-table:: T\n   :name: a\n\n   a, b\n\nAfter.\n", 9),
            (
                ".. list-table:: T\n   :name: a\n\n   * - a\n     - b\n\nAfter.\n",
                10,
            ),
            (".. image:: pic.png\n   :name: a\n\nAfter.\n", 5),
            (
                ".. figure:: pic.png\n   :name: a\n\n   Caption.\n\nAfter.\n",
                5,
            ),
            (".. topic:: T\n   :name: a\n\n   Body.\n\nAfter.\n", 5),
            (".. container:: c\n   :name: a\n\n   Body.\n\nAfter.\n", 5),
            (".. line-block::\n   :name: a\n\n   one\n\nAfter.\n", 5),
            // The toctree's wrapper: no location of its own.
            (".. toctree::\n   :name: a\n\n\n\nAfter.\n", 9),
        ] {
            assert_eq!(
                printed_heads(&format!("{head}{body}")),
                [at(line, duplicate)],
                "{body:?}"
            );
        }
        // `self.add_name(wrappernode)` names the wrapper, not the toctree.
        let out = parse_rst_full(
            &format!("{head}.. toctree::\n   :name: a\n\n\n\nAfter.\n"),
            &sphinx_options(),
        );
        let wrapper = first(&out.doctree.root, "compound").expect("the toctree's wrapper");
        assert_eq!(
            wrapper.pformat().lines().next(),
            Some("<compound classes=\"toctree-wrapper\" dupnames=\"a\" ids=\"id1\">")
        );
        // docutils 0.22 parses sections in the top-level machine.
        assert_eq!(
            printed_heads(".. _a:\n\nTitle\n=====\n\n.. note:: b\n   :name: a\n\nAfter.\n"),
            [at(8, duplicate)]
        );
    }

    /// A message with neither a line nor a located node is located where
    /// docutils' top-level state machine stands: inside a nested
    /// construct, on the last line of the first block it read for the
    /// enclosing top-level construct — a list's, field list's, option
    /// list's or definition list's first item, a block quote, a footnote,
    /// a table, a directive — with the block's trailing blank lines.
    /// Probed on Sphinx 9.1.0 with an `index` directive's `:name:`, which
    /// docutils locates that way everywhere.
    #[test]
    fn unlocated_messages_take_the_top_level_machine_position() {
        let duplicate = "WARNING: Duplicate explicit target name: \"a\". [docutils]";
        let index = |indent: &str| format!("{indent}.. index:: x\n{indent}   :name: a\n");
        let head = ".. _a:\n\nText.\n\n";
        for (body, line) in [
            (
                format!(
                    "* one\n\n  first more\n\n* two\n\n{}\n* three\n\nAfter.\n",
                    index("  ")
                ),
                8,
            ),
            (
                format!(
                    "term1\n   def1\n\n   def1 more\n\nterm2\n   def2\n\n{}\nterm3\n   def3\n\n\
                     After.\n",
                    index("   ")
                ),
                9,
            ),
            (
                format!(
                    ":f1: one\n\n   f1 more\n\n:f2: two\n\n{}\n:f3: three\n\nAfter.\n",
                    index("   ")
                ),
                8,
            ),
            (
                format!(
                    "-a  one\n\n    a more\n\n-b  two\n\n{}\n-c  three\n\nAfter.\n",
                    index("    ")
                ),
                8,
            ),
            (
                format!(
                    "   Quote.\n\n{}\n   More quote.\n\n\nAfter.\n",
                    index("   ")
                ),
                12,
            ),
            (
                format!(
                    ".. [1] Foot.\n\n{}\n   More.\n\nAfter [1]_.\n",
                    index("   ")
                ),
                11,
            ),
            (
                "+--------------+\n| Cell         |\n|              |\n| .. index:: x |\n\
                 |    :name: a  |\n+--------------+\n| Two          |\n+--------------+\n\n\
                 After.\n"
                    .to_string(),
                12,
            ),
            (
                "=====  ===============\nA      B\n=====  ===============\n1      .. index:: x\n\
                 \x20         :name: a\n2      two\n=====  ===============\n\nAfter.\n"
                    .to_string(),
                11,
            ),
            (
                ".. note::\n\n   Inner.\n\n   .. note:: b\n      :name: a\n\n   More.\n\nAfter.\n"
                    .to_string(),
                13,
            ),
        ] {
            assert_eq!(
                printed_heads(&format!("{head}{body}")),
                [at(line, duplicate)],
                "{body:?}"
            );
        }
    }

    /// docutils reads the explicit constructs after a top-level one in a
    /// nested machine (`Body.explicit_list`,
    /// `SP/docutils/parsers/rst/states.py:2508-2521`) while each next line
    /// starts another, so the top-level machine, where a message without a
    /// located node is located, stays where the first construct's block
    /// ended: its trailing blank lines included for a directive, a comment,
    /// a footnote or a substitution definition, the construct's own lines
    /// for a target or an anonymous target, whose block stops at a blank
    /// line, and which a blank line after it ends the list at. A paragraph
    /// or a section title ends the list too, and so does the padding line
    /// before an include's lines. Probed on Sphinx 9.1.0 with `:name:` and
    /// `index` duplicates and equation `Duplicate ID`s (each case's warning
    /// stream).
    #[test]
    fn explicit_lists_locate_at_their_first_construct() {
        let duplicate =
            |name: &str| format!("WARNING: Duplicate explicit target name: \"{name}\".");
        let title = "Title\n=====\n\n";
        let head = ".. _a:\n\nText.\n\n";
        let note_x = |text: &str| format!(".. note:: {text}\n   :name: x\n");
        let note_a = ".. note:: b\n   :name: a\n\nEnd.\n";
        for (source, lines) in [
            (
                format!("{title}Para.\n\n{}\n{}\nEnd.\n", note_x("a"), note_x("b")),
                vec![(8, "x")],
            ),
            (
                format!(
                    "{title}Para.\n\n{}\n\n\n{}\n\nEnd.\n",
                    note_x("a"),
                    note_x("b")
                ),
                vec![(10, "x")],
            ),
            (
                format!("{title}{}\nPara.\n\n{}\nEnd.\n", note_x("a"), note_x("b")),
                vec![(11, "x")],
            ),
            (
                format!(
                    "{title}.. _a:\n\n.. index:: x\n   :name: a\n\n.. index:: y\n   :name: a\n\n\
                     End.\n"
                ),
                vec![(8, "a"), (8, "a")],
            ),
            (format!("{head}.. _b:\n{note_a}"), vec![(5, "a")]),
            (format!("{head}.. _b:\n\n{note_a}"), vec![(9, "a")]),
            (format!("{head}.. a comment\n\n{note_a}"), vec![(6, "a")]),
            (format!("{head}..\n\n{note_a}"), vec![(9, "a")]),
            (format!("{head}__ http://x.org\n{note_a}"), vec![(5, "a")]),
            (
                format!("{head}`x`__\n\n__ http://x.org\n\n{note_a}"),
                vec![(11, "a")],
            ),
            (
                format!("{head}See [1]_.\n\n.. [1] Foot.\n\n{note_a}"),
                vec![(8, "a")],
            ),
            (
                format!("{head}Use |x|.\n\n.. |x| replace:: y\n\n{note_a}"),
                vec![(8, "a")],
            ),
            (
                format!("{head}.. note:: one\n\n\n.. note:: two\n\n{note_a}"),
                vec![(7, "a")],
            ),
            (
                format!("{head}.. note:: one\n\nSub\n---\n\n{note_a}"),
                vec![(12, "a")],
            ),
        ] {
            let expected: Vec<String> = lines
                .into_iter()
                .map(|(line, name)| at(line, &format!("{} [docutils]", duplicate(name))))
                .collect();
            assert_eq!(printed_heads(&source), expected, "{source:?}");
        }

        // An include's lines open with the padding line docutils inserts
        // before them (`insert_input`), which ends the explicit list the
        // include opens or belongs to, so the included file's first
        // construct is a top-level construct of its own: a list in the file
        // is located there. The constructs after the include in the
        // document are located in the document. Each row is a project
        // probed on Sphinx 9.1.0: an included file holding a single
        // construct, a list, or another include; the include in a list;
        // two includes in a list; explicit markup right after the include.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let index = tmp.path().join("index.rst").display().to_string();
        let options = ParseOptions {
            source_path: index.clone(),
            srcdir: Some(tmp.path().to_path_buf()),
            ..sphinx_options()
        };
        let named = |name: &str| format!("{} [docutils]", duplicate(name));
        let duplicate_id = "ERROR: Duplicate ID: \"equation-e1\" used by \
                            <target ids=\"equation-e1\"> and <target ids=\"equation-e1\"> \
                            [docutils]";
        let in_file = |file: &str, line: u32, record: &str| format!("{file}:{line}: {record}");
        let in_index = |line: u32, record: &str| format!("{index}:{line}: {record}");
        let chain = format!("{}\n{}", note_x("a"), note_x("b"));
        let math = |text: &str| format!(".. math:: {text}\n   :label: e1\n");
        let target_then = |text: &str| format!("{title}.. _a:\n\n.. include:: part.inc\n{text}");
        let named_a = |text: &str| format!(".. note:: {text}\n   :name: a\n");
        for (source, files, expected) in [
            (
                target_then("\nEnd.\n"),
                vec![("part.inc", named_a("b"))],
                vec![in_file("part.inc", 3, &named("a"))],
            ),
            (
                target_then(""),
                vec![("part.inc", named_a("b"))],
                vec![in_file("part.inc", 3, &named("a"))],
            ),
            (
                target_then("\nEnd.\n"),
                vec![("part.inc", ".. index:: x\n   :name: a\n".to_string())],
                vec![in_file("part.inc", 3, &named("a"))],
            ),
            (
                format!(
                    "{title}{}\nPara.\n\n.. include:: part.inc\n\nEnd.\n",
                    math("x")
                ),
                vec![("part.inc", math("y"))],
                vec![in_file("part.inc", 3, duplicate_id)],
            ),
            (
                format!("{title}.. include:: part.inc\n\nEnd.\n"),
                vec![("part.inc", chain.clone())],
                vec![in_file("part.inc", 3, &named("x"))],
            ),
            (
                format!("{title}.. include:: part.inc\n\nEnd.\n"),
                vec![("part.inc", format!("{}{}", note_x("a"), note_x("b")))],
                vec![in_file("part.inc", 2, &named("x"))],
            ),
            (
                format!("{title}.. include:: part.inc\n\nEnd.\n"),
                vec![("part.inc", format!("{}\n{}", math("x"), math("y")))],
                vec![in_file("part.inc", 3, duplicate_id)],
            ),
            (
                format!("{title}.. _a:\n\nText.\n\n.. include:: part.inc\n\nEnd.\n"),
                vec![("part.inc", format!(".. _b:\n\n{}", named_a("b")))],
                vec![in_file("part.inc", 5, &named("a"))],
            ),
            (
                format!("{title}.. _t:\n.. include:: part.inc\n\nEnd.\n"),
                vec![("part.inc", chain.clone())],
                vec![in_file("part.inc", 3, &named("x"))],
            ),
            (
                format!("{title}.. _a:\n.. include:: part.inc\n\nEnd.\n"),
                vec![("part.inc", named_a("b"))],
                vec![in_file("part.inc", 3, &named("a"))],
            ),
            (
                format!(
                    "{title}.. _t:\n.. include:: part.inc\n{}\nEnd.\n",
                    note_x("c")
                ),
                vec![("part.inc", note_x("a"))],
                vec![in_index(8, &named("x"))],
            ),
            (
                format!("{title}.. include:: p1.inc\n.. include:: p2.inc\n\nEnd.\n"),
                vec![("p1.inc", "One.\n".to_string()), ("p2.inc", chain.clone())],
                vec![in_file("p2.inc", 3, &named("x"))],
            ),
            (
                format!("{title}.. include:: p1.inc\n.. include:: p2.inc\n\nEnd.\n"),
                vec![("p1.inc", ".. _x:\n".to_string()), ("p2.inc", note_x("b"))],
                vec![in_file("p2.inc", 3, &named("x"))],
            ),
            (
                format!("{title}.. include:: part.inc\n\nEnd.\n"),
                vec![
                    ("part.inc", ".. include:: sub.inc\n".to_string()),
                    ("sub.inc", chain.clone()),
                ],
                vec![in_file("sub.inc", 3, &named("x"))],
            ),
            (
                target_then(&format!("{}\nEnd.\n", named_a("c"))),
                vec![("part.inc", "Inc.\n".to_string())],
                vec![in_index(9, &named("a"))],
            ),
            (
                format!(
                    "{title}.. include:: part.inc\n{}{}\nEnd.\n",
                    note_x("c"),
                    note_x("d")
                ),
                vec![("part.inc", ".. _p:\n".to_string())],
                vec![in_index(6, &named("x"))],
            ),
        ] {
            for (name, text) in &files {
                std::fs::write(tmp.path().join(name), text).expect("write");
            }
            assert_eq!(
                printed_heads_with(&source, &options),
                expected,
                "{source:?} {files:?}"
            );
        }
    }

    /// `table_top` warns `Blank line required after table.` whenever the
    /// isolated table does not end the input or a blank line: after a
    /// border that does not match the top one, and after a table whose
    /// parse fails (`states.py:1787-1815`, `:1871-1876`). Probed on Sphinx
    /// 9.1.0.
    #[test]
    fn malformed_simple_tables_warn_about_the_missing_blank_line() {
        let blank = "WARNING: Blank line required after table. [docutils]";
        assert_eq!(
            printed_heads("=====  =====\nA      B\n======  =====\nText after.\n"),
            [at(3, "ERROR: Malformed table."), at(4, blank)]
        );
        assert_eq!(
            printed_heads(
                "=====  =====\n*a     b\n=====  =====\nc      d\n--  --------\n=====  =====\n\
                 Text after.\n"
            ),
            [at(5, "ERROR: Malformed table."), at(7, blank)]
        );
    }

    /// The `PySigConfig` keys a fixture's `conf` sets; the other keys the
    /// fixtures carry steer no parse.
    fn py_config_of(conf: &serde_json::Map<String, Value>) -> crate::py::PySigConfig {
        let mut py = crate::py::PySigConfig::default();
        let flag = |key: &str, value: &Value| {
            value
                .as_bool()
                .unwrap_or_else(|| panic!("conf key {key}: expected a bool, got {value}"))
        };
        for (key, value) in conf {
            match key.as_str() {
                "maximum_signature_line_length" => {
                    py.maximum_signature_line_length = value.as_i64()
                }
                "python_maximum_signature_line_length" => {
                    py.python_maximum_signature_line_length = value.as_i64();
                }
                "python_trailing_comma_in_multi_line_signatures" => {
                    py.python_trailing_comma_in_multi_line_signatures = flag(key, value);
                }
                "python_display_short_literal_types" => {
                    py.python_display_short_literal_types = flag(key, value);
                }
                "python_use_unqualified_type_names" => {
                    py.python_use_unqualified_type_names = flag(key, value);
                }
                "toc_object_entries" => py.toc_object_entries = flag(key, value),
                "toc_object_entries_show_parents" => {
                    py.toc_object_entries_show_parents =
                        value.as_str().expect("a string value").to_string();
                }
                "add_function_parentheses" => py.add_function_parentheses = flag(key, value),
                "add_module_names" => py.add_module_names = flag(key, value),
                "strip_signature_backslash" => py.strip_signature_backslash = flag(key, value),
                "smartquotes"
                | "keep_warnings"
                | "nitpicky"
                | "numfig"
                | "numfig_format"
                | "numfig_secnum_depth"
                | "modindex_common_prefix" => {}
                other => panic!("unmapped conf key {other:?}: map it onto the parse options"),
            }
        }
        py
    }

    fn fixture(name: &str) -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let text = std::fs::read_to_string(&path).expect("fixture readable");
        serde_json::from_str(&text).expect("fixture parses")
    }

    /// The parse corpora the parse-level invariants are checked on, each
    /// input parsed the way its consumer parses it
    /// ([`for_each_corpus_parse`]).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Corpus {
        /// `doctree_differential.json`: each case in docutils mode, then
        /// again in sphinx mode.
        Docutils,
        /// `sphinx_doctree_differential.json`: each case in sphinx mode
        /// with its `conf`.
        Sphinx,
        /// `env_differential.json`: every document of every project, as
        /// the build parses it (sphinx mode, its srcdir, docname and
        /// docname set).
        Env,
        /// `read_state_differential.json`: each case as
        /// `tests/read_state_differential.rs` parses it (sphinx mode,
        /// `<SRCDIR>/index.rst`, docname `index`).
        ReadState,
    }

    /// Parses every input of the four corpora ([`Corpus`]) and hands each
    /// parse to `visit` with its corpus and a label naming the input.
    pub(crate) fn for_each_corpus_parse(mut visit: impl FnMut(Corpus, &str, ParseOutput)) {
        let docutils = fixture("doctree_differential.json");
        for case in docutils["cases"].as_array().expect("cases") {
            let name = case["name"].as_str().expect("name");
            let source = case["rst"].as_str().expect("rst");
            for sphinx in [false, true] {
                let options = ParseOptions {
                    source_path: "<snippet>".into(),
                    sphinx,
                    ..Default::default()
                };
                let label = format!("{name} (sphinx: {sphinx})");
                visit(Corpus::Docutils, &label, parse_rst_full(source, &options));
            }
        }

        let sphinx = fixture("sphinx_doctree_differential.json");
        for case in sphinx["cases"].as_array().expect("cases") {
            let name = case["name"].as_str().expect("name");
            let source = case["rst"].as_str().expect("rst");
            let options = ParseOptions {
                py: case["conf"]
                    .as_object()
                    .map(py_config_of)
                    .unwrap_or_default(),
                ..sphinx_options()
            };
            visit(Corpus::Sphinx, name, parse_rst_full(source, &options));
        }

        let env = fixture("env_differential.json");
        for project in env["projects"].as_array().expect("projects") {
            let name = project["name"].as_str().expect("name");
            let tmp = tempfile::TempDir::new().expect("tempdir");
            let srcdir = tmp.path();
            let files = project["files"].as_object().expect("files");
            let data_files = project["data_files"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            let write = |relpath: &str, body: &Value| {
                let path = srcdir.join(relpath);
                std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
                std::fs::write(path, body.as_str().expect("text")).expect("write");
            };
            for (docname, body) in files {
                write(&format!("{docname}.rst"), body);
            }
            for (relpath, body) in &data_files {
                write(relpath, body);
            }
            let found_docs: std::collections::BTreeSet<String> = files.keys().cloned().collect();
            let found_docs = std::sync::Arc::new(found_docs);
            let py = py_config_of(project["conf"].as_object().expect("conf"));
            for (docname, body) in files {
                let path = srcdir.join(format!("{docname}.rst"));
                let options = ParseOptions {
                    source_path: path.display().to_string(),
                    sphinx: true,
                    docname: docname.clone(),
                    found_docs: Some(std::sync::Arc::clone(&found_docs)),
                    py: py.clone(),
                    srcdir: Some(srcdir.to_path_buf()),
                    ..Default::default()
                };
                let source = body.as_str().expect("text");
                let label = format!("{name}/{docname}");
                visit(Corpus::Env, &label, parse_rst_full(source, &options));
            }
        }

        let read_state = fixture("read_state_differential.json");
        for case in read_state["cases"].as_array().expect("cases") {
            let name = case["name"].as_str().expect("name");
            let source = case["rst"].as_str().expect("rst");
            let options = ParseOptions {
                source_path: "<SRCDIR>/index.rst".to_string(),
                sphinx: true,
                docname: "index".to_string(),
                ..Default::default()
            };
            visit(Corpus::ReadState, name, parse_rst_full(source, &options));
        }
    }

    /// The end-of-parse assertions ([DS §8.2]: every registered `Uid` is
    /// reachable exactly once from the root or from `detached`, and every
    /// node a location override names at least once), checked explicitly on
    /// every input of the four parse corpora ([`for_each_corpus_parse`]).
    /// The corpora hold 444 location overrides.
    ///
    /// Only the docutils and read-state corpora register nodes: the other
    /// two hold no reference, footnote, citation or substitution definition
    /// (their targets are labels and `refuri` targets, which no registry
    /// keeps), and the one node the sphinx corpus holds is the pending
    /// `rst-class` of `sx_directives.rst_class`. The docutils corpus
    /// registers 186 nodes over the two modes and keeps one detached
    /// subtree per mode (`substitutions.replace_auto_footnote_error`). The
    /// read-state corpus registers 107 nodes and keeps five detached
    /// subtrees: the definitions of `state.rejected_subst_autofootnote` and
    /// `state.rejected_subst_indirect_target`, the title and the content of
    /// `state.rejected_table_refs` and the content of
    /// `state.rejected_list_table_refs`. Those counts are the floors below.
    #[test]
    fn every_registered_uid_is_reachable_once() {
        /// Per corpus: parses checked, distinct nodes registered, detached
        /// subtrees kept, location overrides held.
        #[derive(Debug, Default)]
        struct Tally {
            parses: usize,
            registered: usize,
            detached: usize,
            overrides: usize,
        }
        let mut tallies: [Tally; 4] = Default::default();
        for_each_corpus_parse(|corpus, label, output| {
            let state = &output.state;
            let misplaced = state.misplaced_registrations(&output.doctree.root);
            assert!(misplaced.is_empty(), "{label}: {misplaced:?}");
            let unreachable = state.unreachable_overrides(&output.doctree.root);
            assert_eq!(unreachable, 0, "{label}: overrides of dropped nodes");
            let tally = &mut tallies[corpus as usize];
            tally.parses += 1;
            let registered: std::collections::HashSet<Uid> = state.held_uids().collect();
            tally.registered += registered.len();
            tally.detached += state.detached.len();
            tally.overrides += state.loc_overrides.len();
        });

        // 735 docutils cases twice, 493 sphinx cases, 89 env documents, 99
        // read-state cases; the check is not vacuous.
        let [docutils, sphinx, env, read_state] = &tallies;
        assert!(
            docutils.parses >= 2 * 735
                && sphinx.parses >= 493
                && env.parses >= 89
                && read_state.parses >= 99,
            "too few parses checked: {tallies:?}"
        );
        assert!(
            docutils.registered >= 2 * 93 && docutils.detached >= 2,
            "the docutils corpus stopped exercising the check: {tallies:?}"
        );
        assert!(
            read_state.registered >= 107 && read_state.detached >= 5,
            "the read-state corpus stopped exercising the check: {tallies:?}"
        );
        assert!(
            tallies.iter().map(|tally| tally.overrides).sum::<usize>() >= 444,
            "the corpora stopped exercising the override check: {tallies:?}"
        );
    }

    /// A subtree the parse discards takes the location overrides of its
    /// nodes with it unless it holds a registered node
    /// ([`DocumentState::detach`]), so every override still names a node
    /// the tree or the detached forest holds. No corpus input discards
    /// such a subtree: a rejected `table`'s content holding a target, which
    /// stands at docutils' absolute line (an override), does.
    #[test]
    fn a_discarded_subtree_takes_its_location_overrides_with_it() {
        let source = ".. table:: Cap\n\n   .. _t:\n\n   Para.\n";
        for sphinx in [false, true] {
            let out = parse_rst_full(
                source,
                &ParseOptions {
                    source_path: "<snippet>".into(),
                    sphinx,
                    ..Default::default()
                },
            );
            assert!(out.state.detached.is_empty(), "{sphinx}: nothing is kept");
            assert_eq!(
                out.state.unreachable_overrides(&out.doctree.root),
                0,
                "{sphinx}"
            );
            assert!(out.state.loc_overrides.is_empty(), "{sphinx}");
        }
    }

    /// IdEvents raised inside a detached sub-parse (a csv-table cell is
    /// parsed over its own lifted text) reach the trace in document order
    /// (T5a review, ruling 4). Probed with a docutils 0.22.4 `set_id`
    /// wrapper: `title`, then the cell's message/problematic pair `id1`
    /// `id2`, then the trailing paragraph's `id3` `id4`.
    #[test]
    fn detached_sub_parse_ids_reach_the_trace_in_document_order() {
        let source = "Title\n=====\n\n.. csv-table::\n\n   *x, b\n\nTail *y\n";
        for sphinx in [false, true] {
            let out = parse_rst_full(
                source,
                &ParseOptions {
                    source_path: "<snippet>".into(),
                    sphinx,
                    trace_ids: true,
                    ..Default::default()
                },
            );
            let trace: Vec<(&str, &str)> = out
                .state
                .ids
                .trace()
                .iter()
                .map(|event| (event.kind, event.id.as_str()))
                .collect();
            assert_eq!(
                trace,
                [
                    ("section", "title"),
                    ("system_message", "id1"),
                    ("problematic", "id2"),
                    ("system_message", "id3"),
                    ("problematic", "id4"),
                ],
                "sphinx mode: {sphinx}"
            );
        }
    }

    /// Sphinx's `IndexDirective.run` notes its target in both branches
    /// (`SP/sphinx/domains/index.py:78-85`; T5a review, ruling 3). Without
    /// `:name:` the target carries `index-N`, which `document.ids` then
    /// holds, so a later label named `index-0` takes an auto id. With
    /// `:name:` the target's names are the raw option value and
    /// `note_explicit_target` allocates its id (`make_id` of the name, or
    /// the counter once that is taken), which the entries name; no index
    /// serial is drawn. Parse-layer trees probed on Sphinx 9.1.0 with
    /// `apply_transforms` intercepted.
    #[test]
    fn index_directive_registers_its_target() {
        let out = sphinx_parse(".. index:: single: foo\n\n.. _index-0:\n\nText.\n");
        assert_eq!(
            out.doctree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('single',\\ 'foo',\\ 'index-0',\\ '',\\ None)\" inline=\"0\">\n",
                "    <target ids=\"index-0\">\n",
                "    <target ids=\"id1\" names=\"index-0\">\n",
                "    <paragraph>\n",
                "        Text.\n",
            )
        );
        assert!(out.state.ids.is_taken("index-0"));

        let out = sphinx_parse(".. index:: single: foo\n   :name: Bar Baz\n\n.. index:: bar\n");
        assert_eq!(
            out.doctree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('single',\\ 'foo',\\ 'bar-baz',\\ '',\\ None)\" inline=\"0\">\n",
                "    <target ids=\"bar-baz\" names=\"Bar\\ Baz\">\n",
                "    <index entries=\"('single',\\ 'bar',\\ 'index-0',\\ '',\\ None)\" inline=\"0\">\n",
                "    <target ids=\"index-0\">\n",
            )
        );
        assert_eq!(
            out.state.registries_dump(&out.doctree.root)["names"],
            json!([["Bar Baz", "bar-baz", true]])
        );

        let out =
            sphinx_parse(".. _bar-baz:\n\nText.\n\n.. index:: single: foo\n   :name: Bar Baz\n");
        assert!(out.doctree.root.pformat().ends_with(concat!(
            "    <index entries=\"('single',\\ 'foo',\\ 'id1',\\ '',\\ None)\" inline=\"0\">\n",
            "    <target ids=\"id1\" names=\"Bar\\ Baz\">\n",
        )));
    }

    /// Every `pending` node is recorded with its transform, its
    /// `note_pending` order and its identity, and keeps docutils'
    /// `rawsource`: the directive's `block_text` (`misc.py:463-466`), which
    /// runs through the blank line after the marker (probed:
    /// `'.. class:: a\n'`).
    #[test]
    fn pending_nodes_are_recorded() {
        let source = ".. class:: a\n\n.. class:: b\n\nPara.\n";
        let out = parse_rst_full(source, &ParseOptions::default());
        let pending: Vec<&Node> = out
            .doctree
            .root
            .children
            .iter()
            .filter(|node| node.kind == "pending")
            .collect();
        assert_eq!(pending.len(), 2);
        let records: Vec<(crate::doctree::pending::PendingTransform, u32, Option<Uid>)> = out
            .state
            .pending
            .iter()
            .map(|record| (record.transform, record.order, Some(record.node)))
            .collect();
        let class_attribute = crate::doctree::pending::PendingTransform::ClassAttribute;
        assert_eq!(
            records,
            [
                (class_attribute, 0, pending[0].uid()),
                (class_attribute, 1, pending[1].uid()),
            ]
        );
        assert_eq!(pending[0].rawsource(), Some(".. class:: a\n"));
        assert_eq!(pending[1].rawsource(), Some(".. class:: b\n"));
    }

    /// The first node of `kind` in document order.
    fn first<'a>(node: &'a Node, kind: &str) -> Option<&'a Node> {
        if node.kind == kind {
            return Some(node);
        }
        node.children.iter().find_map(|child| first(child, kind))
    }

    /// Every node of `kind`, in document order.
    fn all<'a>(node: &'a Node, kind: &str, out: &mut Vec<&'a Node>) {
        if node.kind == kind {
            out.push(node);
        }
        for child in &node.children {
            all(child, kind, out);
        }
    }

    /// docutils keeps `rawsource` on the inline Referential nodes — the
    /// text a transform turns into a `problematic` when the reference
    /// fails (`references.py:138,294,556,596,703,982`) — and on
    /// `substitution_definition` (its block text). pformat never prints
    /// it. Values from the docutils 0.22.4 constructors: `reference`'s
    /// `referencename + refend` and the whole backquoted phrase,
    /// `'[%s]_' % label`, the substitution reference's `|name|` with its
    /// underscores, and a standalone URI's matched text.
    #[test]
    fn referential_nodes_keep_their_rawsource() {
        let source = "A ref_ and anon__ and `a phrase`_ and `alias <target_>`_ and \
                      [#]_ [#n]_ [*]_ [1]_ [cit]_ and |sub| |sub2|_ and \
                      https://x.example/p and :pep:`8`.\n\n\
                      .. |sub| replace:: text\n";
        for sphinx in [false, true] {
            let out = parse_rst_full(
                source,
                &ParseOptions {
                    source_path: "<snippet>".into(),
                    sphinx,
                    ..Default::default()
                },
            );
            let root = &out.doctree.root;
            let rawsources = |kind: &str| {
                let mut nodes = Vec::new();
                all(root, kind, &mut nodes);
                nodes
                    .iter()
                    .map(|node| node.rawsource().map(str::to_string))
                    .collect::<Vec<_>>()
            };
            let some = |values: &[&str]| -> Vec<Option<String>> {
                values.iter().map(|value| Some(value.to_string())).collect()
            };
            let mut references = some(&[
                "ref_",
                "anon__",
                "`a phrase`_",
                "`alias <target_>`_",
                "|sub2|_",
                "https://x.example/p",
            ]);
            // docutils' `:pep:` role builds `reference(rawtext, ...)`;
            // Sphinx's PEP role builds `reference('', '', ...)`.
            references.push((!sphinx).then(|| ":pep:`8`".to_string()));
            assert_eq!(rawsources(kinds::REFERENCE), references, "sphinx: {sphinx}");
            assert_eq!(
                rawsources(kinds::FOOTNOTE_REFERENCE),
                some(&["[#]_", "[#n]_", "[*]_", "[1]_"]),
                "sphinx: {sphinx}"
            );
            assert_eq!(rawsources(kinds::CITATION_REFERENCE), some(&["[cit]_"]));
            assert_eq!(
                rawsources(kinds::SUBSTITUTION_REFERENCE),
                some(&["|sub|", "|sub2|_"])
            );
            let definition = first(root, "substitution_definition").expect("defined");
            assert_eq!(definition.rawsource(), Some(".. |sub| replace:: text"));
            // The inline target an embedded alias builds keeps the alias
            // with the whitespace before it.
            assert_eq!(
                first(root, kinds::TARGET).and_then(Node::rawsource),
                Some(" <target_>")
            );
        }
    }

    /// A hyperlink target keeps its source too, which an indirect target's
    /// error turns into a `problematic` when the target refers to the
    /// erring name (`references.py:292-297`): the inline target of an
    /// embedded link its `<...>` with the whitespace before it
    /// (`target(match.group(1), ...)`, `states.py:929`, `:941`); an
    /// explicit target its block text, the continuation lines with their
    /// indentation for `.. _name:` and `.. __:` (`hyperlink_target`,
    /// `strip_indent=False`, `states.py:2055-2061`) and without it for
    /// `__ link` (`anonymous_target`, `states.py:2530-2535`). An embedded
    /// alias with no text before it takes the normalized alias as its text
    /// and name (`states.py:946-949`).
    //
    // oracle (docutils 0.22.4, parse layer): each input parsed by
    //   `docutils.parsers.rst.Parser` into `utils.new_document`, printing
    //   the pformat and each target's and reference's `rawsource`.
    #[test]
    fn targets_keep_their_rawsource() {
        let source = "`x <t_>`_ and `y <http://u>`_\n\n.. _t: missing_\n.. _u:\n   http://cont\n\
                      .. __: anon_\n\n__ http://one\n   two\n";
        for sphinx in [false, true] {
            let out = parse_rst_full(
                source,
                &ParseOptions {
                    source_path: "<snippet>".into(),
                    sphinx,
                    ..Default::default()
                },
            );
            let mut targets = Vec::new();
            all(&out.doctree.root, kinds::TARGET, &mut targets);
            let rawsources: Vec<Option<&str>> =
                targets.iter().map(|node| node.rawsource()).collect();
            assert_eq!(
                rawsources,
                [
                    Some(" <t_>"),
                    Some(" <http://u>"),
                    Some(".. _t: missing_"),
                    Some(".. _u:\n   http://cont"),
                    Some(".. __: anon_"),
                    Some("__ http://one\ntwo"),
                ],
                "sphinx: {sphinx}"
            );

            let out = parse_rst_full(
                "`<Foo Bar_>`_\n",
                &ParseOptions {
                    source_path: "<snippet>".into(),
                    sphinx,
                    ..Default::default()
                },
            );
            assert_eq!(
                out.doctree.root.pformat(),
                "<document source=\"<snippet>\">\n    <paragraph>\n        \
                 <reference name=\"foo bar\" refname=\"foo bar\">\n            foo bar\n        \
                 <target ids=\"foo-bar\" names=\"foo\\ bar\" refname=\"foo bar\">\n",
                "sphinx: {sphinx}"
            );
            let target = first(&out.doctree.root, kinds::TARGET).expect("a target");
            assert_eq!(target.rawsource(), Some("<Foo Bar_>"));
        }
    }

    // ------------------------------------------------------------------
    // Sphinx-mode directive dispatch and the read configuration (T7)
    // ------------------------------------------------------------------

    use super::roles::tests::{parse as read_parse, read_tree};

    /// The pformat of the post-read tree of `source` read as `<SRC>/index.rst`
    /// under `read`, `keep_warnings = True` and smart quotes off, as the
    /// probes read it: what a transform changes, such as the classes
    /// ClassAttribute (210) places, shows there and not in the parse
    /// ([`read_tree`]).
    fn read_post(source: &str, read: ReadConfig) -> String {
        let options = ParseOptions {
            source_path: "<SRC>/index.rst".to_string(),
            sphinx: true,
            docname: "index".to_string(),
            read,
            ..ParseOptions::default()
        };
        let settings = crate::read::ReadSettings {
            keep_warnings: true,
            smartquotes: None,
            ..crate::read::ReadSettings::sphinx_defaults()
        };
        crate::read::read_standalone(source, &options, &settings)
            .outcome
            .doctree()
            .root
            .pformat()
    }

    /// The `ParseLogWarning`s of a parse, as `(message, location)`.
    fn logged(out: &ParseOutput) -> Vec<(String, LogLocation)> {
        out.registry
            .log_warnings
            .iter()
            .map(|warning| (warning.message.clone(), warning.location))
            .collect()
    }

    /// `sphinx_domains.directive` (`SP/sphinx/util/docutils.py:313-346`):
    /// an unprefixed name asks the default domain first, so `class`,
    /// `function` and `module` are `py`'s (`docutils-0528`…`-0533`,
    /// `-0715`; `.. class::` with no argument is `py:class`'s argument
    /// error, worded like docutils' `class`); with no default domain —
    /// `primary_domain = None`, or a `.. default-domain::` naming no domain
    /// — `class` is docutils' again, and the directive lowercases its
    /// argument. A `domain:` prefix asks that domain and then the std
    /// domain by the bare name, so the six `std:` names this crate has
    /// resolve like their bare spelling, `py:envvar` reaches std too, and
    /// an unknown prefix logs `unknown directive name: X` with no location
    /// before it does. Expected trees probed on Sphinx 9.1.0 with
    /// `keep_warnings = True`.
    #[test]
    fn directive_lookup_uses_default_domain() {
        for (source, expected) in [
        (
            ".. class:: myclass\n\nSome paragraph following.\n\n.. function:: f()\n\n.. module:: m\n",
            "<document source=\"<SRC>/index.rst\">\n    <index entries=\"('single',\\ 'myclass\\ (built-in\\ class)',\\ 'myclass',\\ '',\\ None)\">\n    <desc classes=\"py class\" desctype=\"class\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"class\">\n        <desc_signature _toc_name=\"myclass\" _toc_parts=\"('myclass',)\" class=\"\" classes=\"sig sig-object\" fullname=\"myclass\" ids=\"myclass\" module=\"True\">\n            <desc_annotation xml:space=\"preserve\">\n                <desc_sig_keyword classes=\"k\">\n                    class\n                <desc_sig_space classes=\"w\">\n                     \n            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                myclass\n        <desc_content>\n    <paragraph>\n        Some paragraph following.\n    <index entries=\"('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ '',\\ None)\">\n    <desc classes=\"py function\" desctype=\"function\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n        <desc_signature _toc_name=\"f()\" _toc_parts=\"('f',)\" class=\"\" classes=\"sig sig-object\" fullname=\"f\" ids=\"f\" module=\"True\">\n            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                f\n            <desc_parameterlist xml:space=\"preserve\">\n        <desc_content>\n    <index entries=\"('pair',\\ 'module;\\ m',\\ 'module-m',\\ '',\\ None)\">\n    <target ids=\"module-m\" ismod=\"1\">\n",
        ),
        (
            ".. class::\n\n   Body.\n",
            "<document source=\"<SRC>/index.rst\">\n    <system_message level=\"3\" line=\"1\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Error in \"class\" directive:\n            1 argument(s) required, 0 supplied.\n        <literal_block xml:space=\"preserve\">\n            .. class::\n            \n               Body.\n",
        ),
        (
            ".. std:envvar:: E\n\n.. foo:envvar:: F\n\n.. py:envvar:: G\n\n.. std:program:: prog\n\n.. std:option:: -x\n\n.. std:cmdoption:: -y\n\n.. std:confval:: cv\n\n.. std:glossary::\n\n   term\n      def\n",
            "<document source=\"<SRC>/index.rst\">\n    <index entries=\"('single',\\ 'environment\\ variable;\\ E',\\ 'envvar-E',\\ '',\\ None)\">\n    <desc classes=\"std envvar\" desctype=\"envvar\" domain=\"std\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"envvar\">\n        <desc_signature _toc_name=\"\" _toc_parts=\"()\" classes=\"sig sig-object\" ids=\"envvar-E\">\n            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                E\n        <desc_content>\n    <index entries=\"('single',\\ 'environment\\ variable;\\ F',\\ 'envvar-F',\\ '',\\ None)\">\n    <desc classes=\"std envvar\" desctype=\"envvar\" domain=\"std\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"envvar\">\n        <desc_signature _toc_name=\"\" _toc_parts=\"()\" classes=\"sig sig-object\" ids=\"envvar-F\">\n            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                F\n        <desc_content>\n    <index entries=\"('single',\\ 'environment\\ variable;\\ G',\\ 'envvar-G',\\ '',\\ None)\">\n    <desc classes=\"std envvar\" desctype=\"envvar\" domain=\"std\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"envvar\">\n        <desc_signature _toc_name=\"\" _toc_parts=\"()\" classes=\"sig sig-object\" ids=\"envvar-G\">\n            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                G\n        <desc_content>\n    <index entries=\"('pair',\\ 'prog\\ command\\ line\\ option;\\ -x',\\ 'cmdoption-prog-x',\\ '',\\ None)\">\n    <desc classes=\"std option\" desctype=\"option\" domain=\"std\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"option\">\n        <desc_signature _toc_name=\"\" _toc_parts=\"()\" allnames=\"-x\" classes=\"sig sig-object\" ids=\"cmdoption-prog-x\">\n            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                -x\n            <desc_addname classes=\"sig-prename descclassname\" xml:space=\"preserve\">\n        <desc_content>\n    <index entries=\"('pair',\\ 'prog\\ command\\ line\\ option;\\ -y',\\ 'cmdoption-prog-y',\\ '',\\ None)\">\n    <desc classes=\"std cmdoption\" desctype=\"cmdoption\" domain=\"std\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"cmdoption\">\n        <desc_signature _toc_name=\"\" _toc_parts=\"()\" allnames=\"-y\" classes=\"sig sig-object\" ids=\"cmdoption-prog-y\">\n            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                -y\n            <desc_addname classes=\"sig-prename descclassname\" xml:space=\"preserve\">\n        <desc_content>\n    <index entries=\"('pair',\\ 'cv;\\ configuration\\ value',\\ 'confval-cv',\\ '',\\ None)\">\n    <desc classes=\"std confval\" desctype=\"confval\" domain=\"std\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"confval\">\n        <desc_signature _toc_name=\"cv\" _toc_parts=\"('cv',)\" classes=\"sig sig-object\" fullname=\"cv\" ids=\"confval-cv\">\n            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                cv\n        <desc_content>\n    <glossary sorted=\"0\">\n        <definition_list classes=\"glossary\">\n            <definition_list_item>\n                <term ids=\"term-term\">\n                    term\n                    <index entries=\"('single',\\ 'term',\\ 'term-term',\\ 'main',\\ None)\">\n                <definition>\n                    <paragraph>\n                        def\n",
        ),
        (
            ".. FUNCTION:: f()\n\n.. Std:EnvVar:: E\n",
            "<document source=\"<SRC>/index.rst\">\n    <index entries=\"('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ '',\\ None)\">\n    <desc classes=\"py function\" desctype=\"function\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n        <desc_signature _toc_name=\"f()\" _toc_parts=\"('f',)\" class=\"\" classes=\"sig sig-object\" fullname=\"f\" ids=\"f\" module=\"True\">\n            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                f\n            <desc_parameterlist xml:space=\"preserve\">\n        <desc_content>\n    <index entries=\"('single',\\ 'environment\\ variable;\\ E',\\ 'envvar-E',\\ '',\\ None)\">\n    <desc classes=\"std envvar\" desctype=\"envvar\" domain=\"std\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"envvar\">\n        <desc_signature _toc_name=\"\" _toc_parts=\"()\" classes=\"sig sig-object\" ids=\"envvar-E\">\n            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                E\n        <desc_content>\n",
        ),
        ] {
            assert_eq!(read_tree(source, ReadConfig::default()), expected, "{source}");
        }
        // docutils' `class` defers its classes to the ClassAttribute
        // transform (210), so the read, not the parse, puts them on the
        // paragraphs: this tree is the read's.
        let source = ".. default-domain:: std\n\n.. class:: x\n\nPara.\n\n.. default-domain:: nonexistent\n\n.. class:: y\n\nPara two.\n\n.. default-domain:: PY\n\n.. class:: z\n";
        assert_eq!(
            read_post(source, ReadConfig::default()),
            "<document source=\"<SRC>/index.rst\" translation_progress=\"{'total': 0, 'translated': 0}\">\n    <paragraph classes=\"x\">\n        Para.\n    <paragraph classes=\"y\">\n        Para two.\n    <index entries=\"('single',\\ 'z\\ (built-in\\ class)',\\ 'z',\\ '',\\ None)\">\n    <desc classes=\"py class\" desctype=\"class\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"class\">\n        <desc_signature _toc_name=\"z\" _toc_parts=\"('z',)\" class=\"\" classes=\"sig sig-object\" fullname=\"z\" ids=\"z\" module=\"True\">\n            <desc_annotation xml:space=\"preserve\">\n                <desc_sig_keyword classes=\"k\">\n                    class\n                <desc_sig_space classes=\"w\">\n                     \n            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                z\n        <desc_content>\n",
            "{source}"
        );
        let out = read_parse(
            ".. std:envvar:: E\n\n.. foo:envvar:: F\n\n.. Foo:Note:: n\n",
            ReadConfig::default(),
        );
        assert_eq!(
            logged(&out),
            [
                (
                    "unknown directive name: foo:envvar".to_string(),
                    LogLocation::None
                ),
                (
                    "unknown directive name: foo:note".to_string(),
                    LogLocation::None
                ),
            ]
        );
        let unknown: Vec<String> = tree_messages(&out)
            .iter()
            .map(|message| message.children[0].astext_docutils())
            .filter(|text| text.starts_with("Unknown"))
            .collect();
        assert_eq!(unknown, ["Unknown directive type \"Foo:Note\"."]);

        // `primary_domain = None`: docutils' `class`, and `:func:` is no
        // role at all.
        let none = ReadConfig {
            primary_domain: None,
            ..ReadConfig::default()
        };
        assert_eq!(
            read_post(".. class:: x\n\nPara.\n\n:func:`f`\n", none.clone()),
            "<document source=\"<SRC>/index.rst\" translation_progress=\"{'total': 0, 'translated': 0}\">\n    <paragraph classes=\"x\">\n        Para.\n    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :func:`f`\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"5\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"func\".\n"
        );
        // A name no domain is registered under is no domain either.
        let unknown_domain = ReadConfig {
            primary_domain: Some("nope".to_string()),
            ..ReadConfig::default()
        };
        assert_eq!(
            read_tree(".. class:: x\n\nPara.\n\n:func:`f`\n", unknown_domain),
            read_tree(".. class:: x\n\nPara.\n\n:func:`f`\n", none)
        );

        // The bare role names follow the default domain too: `func` and
        // `class` are py's, `ref` is std's, `py:ref` falls back to std's,
        // and an unknown prefix logs before the std lookup.
        let source = ":func:`f` :class:`C` :ref:`r` :py:ref:`r2` :foo:doc:`index`\n";
        let expected = "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <pending_xref py:class=\"True\" py:module=\"True\" refdoc=\"index\" refdomain=\"py\" refexplicit=\"0\" reftarget=\"f\" reftype=\"func\" refwarn=\"0\">\n            <literal classes=\"xref py py-func\">\n                f()\n         \n        <pending_xref py:class=\"True\" py:module=\"True\" refdoc=\"index\" refdomain=\"py\" refexplicit=\"0\" reftarget=\"C\" reftype=\"class\" refwarn=\"0\">\n            <literal classes=\"xref py py-class\">\n                C\n         \n        <pending_xref refdoc=\"index\" refdomain=\"std\" refexplicit=\"0\" reftarget=\"r\" reftype=\"ref\" refwarn=\"1\">\n            <inline classes=\"xref std std-ref\">\n                r\n         \n        <pending_xref refdoc=\"index\" refdomain=\"std\" refexplicit=\"0\" reftarget=\"r2\" reftype=\"ref\" refwarn=\"1\">\n            <inline classes=\"xref std std-ref\">\n                r2\n         \n        <pending_xref refdoc=\"index\" refdomain=\"std\" refexplicit=\"0\" reftarget=\"index\" reftype=\"doc\" refwarn=\"1\">\n            <inline classes=\"xref std std-doc\">\n                index\n";
        assert_eq!(read_tree(source, ReadConfig::default()), expected);

        // The c, cpp, js and rst domains are registered, so their prefixes
        // log nothing, but this crate implements none of their directives
        // (decision 16), and `productionlist` waits for T18c in both
        // spellings: each keeps `Unknown directive type`.
        for name in [
            "c:function",
            "cpp:class",
            "js:function",
            "rst:directive",
            "std:productionlist",
            "productionlist",
        ] {
            let out = read_parse(&format!(".. {name}:: x\n"), ReadConfig::default());
            assert!(logged(&out).is_empty(), "{name}");
            let texts: Vec<String> = tree_messages(&out)
                .iter()
                .map(|message| message.children[0].astext_docutils())
                .collect();
            assert!(
                texts.contains(&format!("Unknown directive type \"{name}\".")),
                "{name}: {texts:?}"
            );
        }
    }

    /// Sphinx's `code` directive (`SP/sphinx/directives/patches.py:85-134`)
    /// replaces docutils' in a Sphinx parse: a `literal_block` with
    /// `force`, `highlight_args` and a `language` (the argument, else the
    /// highlight language), `linenos` and `linenostart` from
    /// `:number-lines:` (Sphinx's `optional_int`, whose ValueError text the
    /// option error carries, `docutils-0509`), and no Pygments: no
    /// `code` class, no tokens (`docutils-0504`…`-0508`). Sphinx's `code`
    /// role keeps an empty `language` and loses the escapes ([RC §3.11],
    /// `docutils-0274`, `-0302`). Probed on Sphinx 9.1.0 with
    /// `keep_warnings = True`.
    #[test]
    fn sphinx_code_directive_and_role() {
        for (source, expected) in [
        (
            ".. code::\n\n   x = 1\n   y = 2\n\n.. code::\n   :class: extra\n   :name: snippet one\n\n   pass\n\n.. code:: python\n\n   x = 1\n\n.. code::\n   :number-lines:\n\n   one\n   two\n\n.. code::\n   :number-lines: 9\n   :force:\n\n   one\n   two\n",
            "<document source=\"<SRC>/index.rst\">\n    <literal_block force=\"0\" highlight_args=\"{}\" language=\"default\" xml:space=\"preserve\">\n        x = 1\n        y = 2\n    <literal_block classes=\"extra\" force=\"0\" highlight_args=\"{}\" ids=\"snippet-one\" language=\"default\" names=\"snippet\\ one\" xml:space=\"preserve\">\n        pass\n    <literal_block force=\"0\" highlight_args=\"{}\" language=\"python\" xml:space=\"preserve\">\n        x = 1\n    <literal_block force=\"0\" highlight_args=\"{}\" language=\"default\" linenos=\"1\" xml:space=\"preserve\">\n        one\n        two\n    <literal_block force=\"1\" highlight_args=\"{'linenostart': 9}\" language=\"default\" linenos=\"1\" xml:space=\"preserve\">\n        one\n        two\n",
        ),
        (
            ".. code::\n   :number-lines: xyz\n\n   one\n\n.. code::\n\n.. code::\n   :number-lines: -1\n\n   one\n",
            "<document source=\"<SRC>/index.rst\">\n    <system_message level=\"3\" line=\"1\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Error in \"code\" directive:\n            invalid option value: (option: \"number-lines\"; value: 'xyz')\n            invalid literal for int() with base 10: 'xyz'.\n        <literal_block xml:space=\"preserve\">\n            .. code::\n               :number-lines: xyz\n            \n               one\n    <system_message level=\"3\" line=\"6\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Content block expected for the \"code\" directive; none found.\n        <literal_block xml:space=\"preserve\">\n            .. code::\n    <system_message level=\"3\" line=\"8\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Error in \"code\" directive:\n            invalid option value: (option: \"number-lines\"; value: '-1')\n            negative value; must be positive or zero.\n        <literal_block xml:space=\"preserve\">\n            .. code::\n               :number-lines: -1\n            \n               one\n",
        ),
        (
            ".. highlight:: c\n\n.. code::\n\n   int x;\n\n.. code-block::\n\n   int y;\n",
            "<document source=\"<SRC>/index.rst\">\n    <highlightlang force=\"0\" lang=\"c\" linenothreshold=\"9223372036854775807\">\n    <literal_block force=\"0\" highlight_args=\"{}\" language=\"c\" xml:space=\"preserve\">\n        int x;\n    <literal_block force=\"0\" highlight_args=\"{}\" language=\"c\" xml:space=\"preserve\">\n        int y;\n",
        ),
        (
            ":code:`x = 1`\n\n:code:`a\\*b`\n\n:code:`a\\\\b`\n\n:CODE:`y`\n",
            "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <literal classes=\"code\" language=\"\">\n            x = 1\n    <paragraph>\n        <literal classes=\"code\" language=\"\">\n            a*b\n    <paragraph>\n        <literal classes=\"code\" language=\"\">\n            a\\b\n    <paragraph>\n        <literal classes=\"code\" language=\"\">\n            y\n",
        ),
        ] {
            assert_eq!(read_tree(source, ReadConfig::default()), expected, "{source}");
        }
    }

    /// `highlight_language` is the language of a `code-block` or `code`
    /// with no argument until a `.. highlight::` sets the document's
    /// own; a `::` literal block gets none at read time (Sphinx 9.1.0,
    /// `highlight_language = 'python3'`).
    #[test]
    fn highlight_language_reaches_code_block() {
        let read = ReadConfig {
            highlight_language: "python3".to_string(),
            ..ReadConfig::default()
        };
        assert_eq!(
            read_tree(
                ".. code-block::\n\n   x\n\n.. code::\n\n   y\n\n::\n\n   z\n\n\
                 .. highlight:: c\n\n.. code-block::\n\n   w\n",
                read
            ),
            "<document source=\"<SRC>/index.rst\">\n    <literal_block force=\"0\" highlight_args=\"{}\" language=\"python3\" xml:space=\"preserve\">\n        x\n    <literal_block force=\"0\" highlight_args=\"{}\" language=\"python3\" xml:space=\"preserve\">\n        y\n    <literal_block xml:space=\"preserve\">\n        z\n    <highlightlang force=\"0\" lang=\"c\" linenothreshold=\"9223372036854775807\">\n    <literal_block force=\"0\" highlight_args=\"{}\" language=\"c\" xml:space=\"preserve\">\n        w\n"
        );
    }

    /// Sphinx's `math` directive names its node (`add_name`) as well as
    /// labelling it from `:name:` when it has no `:label:`, so the block
    /// keeps the name and its id before the equation id
    /// (`docutils-0514`; Sphinx 9.1.0, `keep_warnings = True`).
    #[test]
    fn math_name_without_label_keeps_names() {
        let source = ".. math::\n   :class: eq\n   :name: my eq\n\n   x^2\n\n.. math::\n   :label: lab\n   :name: nm\n\n   y\n";
        let expected = "<document source=\"<SRC>/index.rst\">\n    <target refid=\"equation-my-eq\">\n    <math_block classes=\"eq\" docname=\"index\" ids=\"my-eq equation-my-eq\" label=\"my eq\" names=\"my\\ eq\" no-wrap=\"0\" nowrap=\"0\" number=\"1\" xml:space=\"preserve\">\n        x^2\n    <target refid=\"equation-lab\">\n    <math_block docname=\"index\" ids=\"nm equation-lab\" label=\"lab\" names=\"nm\" no-wrap=\"0\" nowrap=\"0\" number=\"2\" xml:space=\"preserve\">\n        y\n";
        assert_eq!(read_tree(source, ReadConfig::default()), expected);
    }

    /// F11 ([TW §6.3]): `code-block`'s `line number spec is out of
    /// range(1-N): <repr>` is a Sphinx logger warning, not a reporter
    /// message: it leaves the tree alone (the lines in range are kept), is
    /// located by the `(source, line)` tuple, which renders
    /// `<srcdir>/index.rst.rst:N` (`sphinx_read-0297`), and is ordered
    /// among the parse's records by its seq: between the empty `note`'s
    /// ERROR before it and the empty `warning`'s after it, as Sphinx
    /// prints them (probed on Sphinx 9.1.0).
    #[test]
    fn emphasize_lines_out_of_range_warns() {
        let [(source, expected)] = [
        (
            ".. note::\n\n.. code-block:: python\n   :emphasize-lines: 1,99\n\n   a\n   b\n\n.. warning::\n",
            "<document source=\"<SRC>/index.rst\">\n    <system_message level=\"3\" line=\"1\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Content block expected for the \"note\" directive; none found.\n        <literal_block xml:space=\"preserve\">\n            .. note::\n    <literal_block force=\"0\" highlight_args=\"{'hl_lines': [1]}\" language=\"python\" xml:space=\"preserve\">\n        a\n        b\n    <system_message level=\"3\" line=\"9\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Content block expected for the \"warning\" directive; none found.\n        <literal_block xml:space=\"preserve\">\n            .. warning::\n",
        ),
        ];
        let out = read_parse(source, ReadConfig::default());
        assert_eq!(read_tree(source, ReadConfig::default()), expected);
        let [warning] = out.registry.log_warnings.as_slice() else {
            panic!("one logger record: {:?}", out.registry.log_warnings);
        };
        assert_eq!(
            (warning.message.as_str(), warning.location),
            (
                "line number spec is out of range(1-2): '1,99'",
                LogLocation::Tuple { source: 0, line: 3 }
            )
        );
        let errors: Vec<u32> = reporter_records(&out)
            .into_iter()
            .filter(|(_, level, _, _)| *level == crate::doctree::messages::ERROR)
            .map(|(seq, _, _, _)| seq)
            .collect();
        let [before, after] = errors.as_slice() else {
            panic!("two ERRORs: {errors:?}");
        };
        assert!(
            before < &warning.seq && &warning.seq < after,
            "{before} < {} < {after}",
            warning.seq
        );
        let sources = ["/src/index.rst".to_string()];
        assert_eq!(
            warning
                .diagnostic(&sources, std::path::Path::new("/src"), "index")
                .render(true),
            "/src/index.rst.rst:3: WARNING: line number spec is out of range(1-2): '1,99'"
        );
    }

    /// Decision 17's highlight edges, for `code-block` ([BL]): every
    /// emphasized line out of range still warns and leaves an empty
    /// `hl_lines`; `0` is the index `-1`, which passes the `< nlines`
    /// filter and the `+ 1` back as a silent `0` (and `0-1` as `[0, 1]`);
    /// an open left end counts from 1; and an invalid spec is the
    /// `ValueError` funnel, a reporter WARNING in place of the block.
    /// (`literalinclude`'s own edges, the too-far-negative `:lines:` index
    /// among them, are pinned in `block.rs`'s
    /// `the_negative_index_edges_of_linenos_specs_match_the_probe`.)
    /// Probed on Sphinx 9.1.0 with `keep_warnings = True`.
    #[test]
    fn emphasize_lines_edges_match_sphinx() {
        for (source, expected, warning) in [
        (
            ".. code-block:: python\n   :emphasize-lines: 9\n\n   a\n   b\n",
            "<document source=\"<SRC>/index.rst\">\n    <literal_block force=\"0\" highlight_args=\"{'hl_lines': []}\" language=\"python\" xml:space=\"preserve\">\n        a\n        b\n",
            Some("line number spec is out of range(1-2): '9'"),
        ),
        (
            ".. code-block:: python\n   :emphasize-lines: 0\n\n   a\n   b\n",
            "<document source=\"<SRC>/index.rst\">\n    <literal_block force=\"0\" highlight_args=\"{'hl_lines': [0]}\" language=\"python\" xml:space=\"preserve\">\n        a\n        b\n",
            None,
        ),
        (
            ".. code-block:: python\n   :emphasize-lines: 0-1\n\n   a\n   b\n",
            "<document source=\"<SRC>/index.rst\">\n    <literal_block force=\"0\" highlight_args=\"{'hl_lines': [0, 1]}\" language=\"python\" xml:space=\"preserve\">\n        a\n        b\n",
            None,
        ),
        (
            ".. code-block:: python\n   :emphasize-lines: -5\n\n   a\n   b\n",
            "<document source=\"<SRC>/index.rst\">\n    <literal_block force=\"0\" highlight_args=\"{'hl_lines': [1, 2]}\" language=\"python\" xml:space=\"preserve\">\n        a\n        b\n",
            Some("line number spec is out of range(1-2): '-5'"),
        ),
        (
            ".. code-block:: python\n   :emphasize-lines: 5-3\n\n   a\n   b\n",
            "<document source=\"<SRC>/index.rst\">\n    <system_message level=\"2\" line=\"1\" source=\"<SRC>/index.rst\" type=\"WARNING\">\n        <paragraph>\n            invalid line number spec: '5-3'\n",
            None,
        ),
        (
            ".. code-block:: python\n   :emphasize-lines: -\n\n   a\n   b\n",
            "<document source=\"<SRC>/index.rst\">\n    <system_message level=\"2\" line=\"1\" source=\"<SRC>/index.rst\" type=\"WARNING\">\n        <paragraph>\n            invalid line number spec: '-'\n",
            None,
        ),
        ] {
            let out = read_parse(source, ReadConfig::default());
            assert_eq!(read_tree(source, ReadConfig::default()), expected, "{source}");
            let messages: Vec<&str> = out
                .registry
                .log_warnings
                .iter()
                .map(|warning| warning.message.as_str())
                .collect();
            assert_eq!(messages, warning.into_iter().collect::<Vec<_>>(), "{source}");
        }
    }

    /// The kinds that keep the duplicate-name message `add_name` appends
    /// to them (`keeps_duplicate_name_message`) hold it as their own
    /// child, appended when the directive names the node: before the
    /// content the nested parse adds, after a topic's or sidebar's title
    /// and subtitle (`msgnode` is the node, `SP/docutils/nodes.py:1974-1990`).
    /// Probed on Sphinx 9.1.0 with `keep_warnings = True`.
    #[test]
    fn kept_duplicate_name_message_is_the_named_nodes_child() {
        for (source, kind, expected) in [
            (
                ".. _x:\n\nPara.\n\n.. container:: c\n   :name: x\n\n   Body.\n",
                "container",
                "<container classes=\"c\" dupnames=\"x\" ids=\"id1\">\n    <system_message backrefs=\"id1\" level=\"2\" line=\"5\" source=\"<SRC>/index.rst\" type=\"WARNING\">\n        <paragraph>\n            Duplicate explicit target name: \"x\".\n    <paragraph>\n        Body.\n",
            ),
            (
                ".. _x:\n\nPara.\n\n.. topic:: T\n   :name: x\n\n   Body.\n",
                "topic",
                "<topic dupnames=\"x\" ids=\"id1\">\n    <title>\n        T\n    <system_message backrefs=\"id1\" level=\"2\" line=\"5\" source=\"<SRC>/index.rst\" type=\"WARNING\">\n        <paragraph>\n            Duplicate explicit target name: \"x\".\n    <paragraph>\n        Body.\n",
            ),
            (
                ".. _x:\n\nPara.\n\n.. sidebar:: S\n   :name: x\n\n   Body.\n",
                "sidebar",
                "<sidebar dupnames=\"x\" ids=\"id1\">\n    <title>\n        S\n    <system_message backrefs=\"id1\" level=\"2\" line=\"5\" source=\"<SRC>/index.rst\" type=\"WARNING\">\n        <paragraph>\n            Duplicate explicit target name: \"x\".\n    <paragraph>\n        Body.\n",
            ),
            (
                ".. _x:\n\nPara.\n\n.. sidebar:: S\n   :name: x\n   :subtitle: Sub\n\n   Body.\n",
                "sidebar",
                "<sidebar dupnames=\"x\" ids=\"id1\">\n    <title>\n        S\n    <subtitle>\n        Sub\n    <system_message backrefs=\"id1\" level=\"2\" line=\"5\" source=\"<SRC>/index.rst\" type=\"WARNING\">\n        <paragraph>\n            Duplicate explicit target name: \"x\".\n    <paragraph>\n        Body.\n",
            ),
            (
                ".. _x:\n\nPara.\n\n.. compound::\n   :name: x\n\n   Body.\n",
                "compound",
                "<compound dupnames=\"x\" ids=\"id1\">\n    <system_message backrefs=\"id1\" level=\"2\" line=\"5\" source=\"<SRC>/index.rst\" type=\"WARNING\">\n        <paragraph>\n            Duplicate explicit target name: \"x\".\n    <paragraph>\n        Body.\n",
            ),
        ] {
            let out = read_parse(source, ReadConfig::default());
            let node = first(&out.doctree.root, kind).expect("the named node");
            assert_eq!(node.pformat(), expected, "{source}");
            assert_eq!(tree_messages(&out).len(), 1, "{source}");
        }
        // An admonition and `seealso` hold it first too, and Sphinx locates
        // it at the end of the directive block: lines 6, 10 and 8 (probed
        // on Sphinx 9.1.0).
        let duplicate = "WARNING: Duplicate explicit target name: \"x\". [docutils]";
        for (source, kind, paragraphs, line) in [
            (
                ".. _x:\n\nPara.\n\n.. note:: b\n   :name: x\n",
                "note",
                1,
                6,
            ),
            (
                ".. _x:\n\nPara.\n\n.. note::\n   :name: x\n\n   Body one.\n\n   Body two.\n",
                "note",
                2,
                10,
            ),
            (
                ".. _x:\n\nPara.\n\n.. seealso::\n   :name: x\n\n   Body.\n",
                "seealso",
                1,
                8,
            ),
        ] {
            assert_eq!(printed_heads(source), [at(line, duplicate)], "{source}");
            let out = read_parse(source, ReadConfig::default());
            let node = first(&out.doctree.root, kind).expect("the named node");
            let children: Vec<&str> = node.children.iter().map(|child| child.kind).collect();
            let mut expected = vec![kinds::SYSTEM_MESSAGE];
            expected.extend(std::iter::repeat_n(kinds::PARAGRAPH, paragraphs));
            assert_eq!(children, expected, "{source}");
            let message = &node.children[0];
            assert_eq!(message.attrs.backrefs, node.attrs.ids, "{source}");
            assert_eq!(
                message.children[0].astext_docutils(),
                "Duplicate explicit target name: \"x\".",
                "{source}"
            );
            assert_eq!(tree_messages(&out).len(), 1, "{source}");
        }
    }

    // ------------------------------------------------------------------
    // T7 review round 1: term classifiers, code captions, nested titles
    // ------------------------------------------------------------------

    /// The records of a Sphinx-mode parse as Sphinx prints them (no
    /// transform runs yet): the reporter's WARNING-and-up records and the
    /// logger records, in the order they were created.
    fn printed(out: &ParseOutput) -> Vec<String> {
        let mut keyed: Vec<(u32, String)> = render_records(
            out.state.log.clone(),
            &out.doctree.sources,
            "<SRC>/index.rst",
        )
        .into_iter()
        .map(|(key, diagnostic)| (key.seq, diagnostic.render(true)))
        .collect();
        keyed.extend(out.registry.log_warnings.iter().map(|warning| {
            let diagnostic =
                warning.diagnostic(&out.doctree.sources, std::path::Path::new("<SRC>"), "index");
            (warning.seq, diagnostic.render(true))
        }));
        keyed.sort_by_key(|(seq, _)| *seq);
        keyed.into_iter().map(|(_, record)| record).collect()
    }

    /// Each case's tree ([`read_tree`]) and printed records ([`printed`]).
    fn assert_reads(cases: &[(&str, &str, &[&str])]) {
        for (source, tree, records) in cases {
            let out = read_parse(source, ReadConfig::default());
            assert_eq!(read_tree(source, ReadConfig::default()), *tree, "{source}");
            assert_eq!(printed(&out), *records, "{source}");
        }
    }

    /// docutils' `Text.term` (`states.py:3001-3022`): the term line parses
    /// once, whole, and only then do its Text nodes split on ` +: +`, the
    /// first part right-stripped into the term (or the latest classifier),
    /// each further part a new classifier. So `*term : x*` is one emphasis
    /// with no spurious WARNING (and the sentinel keeps `id1`/`id2`), a
    /// classifier may hold `*x : y*`, and `:nod:bogus:`a : b`` is one
    /// unknown role, logged with its ERROR. The glossary keeps Sphinx's
    /// split of the raw line (`domains/std/__init__.py:518-521`): its
    /// classifier, the index key, is never parsed, so `:nod:gcls:` logs
    /// nothing. Lines that split the same either way are unchanged: the
    /// classifier text unstripped, the term's NBSP stripped. Expected trees
    /// and records probed on Sphinx 9.1.0 with `keep_warnings = True`.
    #[test]
    fn a_term_line_parses_whole_before_its_text_splits_into_classifiers() {
        assert_reads(&[
        (
            "Title\n=====\n\n*term : x*\n   Definition.\n\nSentinel :nod:sentinel:`z`.\n",
            "<document source=\"<SRC>/index.rst\">\n    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n        <definition_list>\n            <definition_list_item>\n                <term>\n                    <emphasis>\n                        term : x\n                <definition>\n                    <paragraph>\n                        Definition.\n        <paragraph>\n            Sentinel \n            <problematic ids=\"id2\" refid=\"id1\">\n                :nod:sentinel:`z`\n            .\n        <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"7\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n            <paragraph>\n                Unknown interpreted text role \"nod:sentinel\".\n",
            &["WARNING: unknown role name: nod:sentinel", "<SRC>/index.rst:7: ERROR: Unknown interpreted text role \"nod:sentinel\". [docutils]"],
        ),
        (
            "Title\n=====\n\nterm : *x : y*\n   Definition.\n\nterm2 : :nod:bogus:`a : b`\n   Definition.\n\nSentinel :nod:sentinel:`z`.\n",
            "<document source=\"<SRC>/index.rst\">\n    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n        <definition_list>\n            <definition_list_item>\n                <term>\n                    term\n                <classifier>\n                    <emphasis>\n                        x : y\n                <definition>\n                    <paragraph>\n                        Definition.\n            <definition_list_item>\n                <term>\n                    term2\n                <classifier>\n                    <problematic ids=\"id2\" refid=\"id1\">\n                        :nod:bogus:`a : b`\n                <definition>\n                    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"7\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n                        <paragraph>\n                            Unknown interpreted text role \"nod:bogus\".\n                    <paragraph>\n                        Definition.\n        <paragraph>\n            Sentinel \n            <problematic ids=\"id4\" refid=\"id3\">\n                :nod:sentinel:`z`\n            .\n        <system_message backrefs=\"id4\" ids=\"id3\" level=\"3\" line=\"10\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n            <paragraph>\n                Unknown interpreted text role \"nod:sentinel\".\n",
            &["WARNING: unknown role name: nod:bogus", "<SRC>/index.rst:7: ERROR: Unknown interpreted text role \"nod:bogus\". [docutils]", "WARNING: unknown role name: nod:sentinel", "<SRC>/index.rst:10: ERROR: Unknown interpreted text role \"nod:sentinel\". [docutils]"],
        ),
        (
            "Title\n=====\n\nterm :nod:bogus:`a : b`\n   Definition.\n\nSentinel :nod:sentinel:`z`.\n",
            "<document source=\"<SRC>/index.rst\">\n    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n        <definition_list>\n            <definition_list_item>\n                <term>\n                    term \n                    <problematic ids=\"id2\" refid=\"id1\">\n                        :nod:bogus:`a : b`\n                <definition>\n                    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"4\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n                        <paragraph>\n                            Unknown interpreted text role \"nod:bogus\".\n                    <paragraph>\n                        Definition.\n        <paragraph>\n            Sentinel \n            <problematic ids=\"id4\" refid=\"id3\">\n                :nod:sentinel:`z`\n            .\n        <system_message backrefs=\"id4\" ids=\"id3\" level=\"3\" line=\"7\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n            <paragraph>\n                Unknown interpreted text role \"nod:sentinel\".\n",
            &["WARNING: unknown role name: nod:bogus", "<SRC>/index.rst:4: ERROR: Unknown interpreted text role \"nod:bogus\". [docutils]", "WARNING: unknown role name: nod:sentinel", "<SRC>/index.rst:7: ERROR: Unknown interpreted text role \"nod:sentinel\". [docutils]"],
        ),
        (
            "Title\n=====\n\n.. glossary::\n\n   Gterm : :nod:gcls:`x`\n      Def one.\n\n``lit : eral`` term\n   Def three.\n\nSentinel :nod:sentinel:`z`.\n",
            "<document source=\"<SRC>/index.rst\">\n    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n        <glossary sorted=\"0\">\n            <definition_list classes=\"glossary\">\n                <definition_list_item>\n                    <term ids=\"term-Gterm\">\n                        Gterm\n                        <index entries=\"('single',\\ 'Gterm',\\ 'term-Gterm',\\ 'main',\\ ':nod:gcls:`x`')\">\n                    <definition>\n                        <paragraph>\n                            Def one.\n        <definition_list>\n            <definition_list_item>\n                <term>\n                    <literal>\n                        lit : eral\n                     term\n                <definition>\n                    <paragraph>\n                        Def three.\n        <paragraph>\n            Sentinel \n            <problematic ids=\"id2\" refid=\"id1\">\n                :nod:sentinel:`z`\n            .\n        <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"12\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n            <paragraph>\n                Unknown interpreted text role \"nod:sentinel\".\n",
            &["WARNING: unknown role name: nod:sentinel", "<SRC>/index.rst:12: ERROR: Unknown interpreted text role \"nod:sentinel\". [docutils]"],
        ),
        (
            "plain *term* here\n   Def.\n\nterm : c1 : c2\n   Def.\n\na : b *em* c : d\n   Def.\n\n*em* : cls\n   Def.\n\nterm\u{a0} : cls\n   Def.\n\nterm : c\\*ls : d\\\\e\n   Def.\n\nterm : cls *open\n   Def.\n",
            "<document source=\"<SRC>/index.rst\">\n    <definition_list>\n        <definition_list_item>\n            <term>\n                plain \n                <emphasis>\n                    term\n                 here\n            <definition>\n                <paragraph>\n                    Def.\n        <definition_list_item>\n            <term>\n                term\n            <classifier>\n                c1\n            <classifier>\n                c2\n            <definition>\n                <paragraph>\n                    Def.\n        <definition_list_item>\n            <term>\n                a\n            <classifier>\n                b \n                <emphasis>\n                    em\n                 c\n            <classifier>\n                d\n            <definition>\n                <paragraph>\n                    Def.\n        <definition_list_item>\n            <term>\n                <emphasis>\n                    em\n            <classifier>\n                cls\n            <definition>\n                <paragraph>\n                    Def.\n        <definition_list_item>\n            <term>\n                term\n            <classifier>\n                cls\n            <definition>\n                <paragraph>\n                    Def.\n        <definition_list_item>\n            <term>\n                term\n            <classifier>\n                c*ls\n            <classifier>\n                d\\e\n            <definition>\n                <paragraph>\n                    Def.\n        <definition_list_item>\n            <term>\n                term\n            <classifier>\n                cls \n                <problematic ids=\"id2\" refid=\"id1\">\n                    *\n                open\n            <definition>\n                <system_message backrefs=\"id2\" ids=\"id1\" level=\"2\" line=\"19\" source=\"<SRC>/index.rst\" type=\"WARNING\">\n                    <paragraph>\n                        Inline emphasis start-string without end-string.\n                <paragraph>\n                    Def.\n",
            &["<SRC>/index.rst:19: WARNING: Inline emphasis start-string without end-string. [docutils]"],
        ),
        ]);
    }

    /// Sphinx's admonitions (`SP/sphinx/directives/admonitions.py:20-37`)
    /// add `collapsible` to docutils' option table, converted by
    /// `_collapsible_arg` (a bare option is `open`, otherwise `open` or
    /// `closed` whatever the case, else a `ValueError` whose own period
    /// docutils' option error follows with another), and its `Rubric`
    /// patch (`SP/sphinx/directives/patches.py:203-225`) adds
    /// `heading-level`, a choice of `1` to `6` made an int. Both land on
    /// the node as attributes. Trees and records probed on Sphinx 9.1.0
    /// (`-w`, post-read trees).
    #[test]
    fn sphinx_admonitions_and_rubric_take_their_sphinx_options() {
        const HEAD: &str = "<document source=\"<SRC>/index.rst\">\n    <section ids=\"title\" \
                            names=\"title\">\n        <title>\n            Title\n";
        let body = |text: &str| format!("            <paragraph>\n                {text}\n");
        let rows: Vec<(String, String)> =
            vec![
            (
                "Title\n=====\n\n.. note::\n   :collapsible:\n\n   BodyA.\n".to_string(),
                format!("{HEAD}        <note collapsible=\"open\">\n{}", body("BodyA.")),
            ),
            (
                "Title\n=====\n\n.. warning::\n   :collapsible: open\n\n   BodyB.\n".to_string(),
                format!("{HEAD}        <warning collapsible=\"open\">\n{}", body("BodyB.")),
            ),
            (
                "Title\n=====\n\n.. admonition:: Generic\n   :collapsible: closed\n\n   BodyC.\n"
                    .to_string(),
                format!(
                    "{HEAD}        <admonition classes=\"admonition-generic\" \
                     collapsible=\"closed\">\n            <title>\n                Generic\n{}",
                    body("BodyC.")
                ),
            ),
            (
                "Title\n=====\n\n.. seealso::\n   :collapsible:\n\n   BodyD.\n".to_string(),
                format!("{HEAD}        <seealso collapsible=\"open\">\n{}", body("BodyD.")),
            ),
            (
                "Title\n=====\n\n.. tip::\n   :class: x\n   :collapsible: CLOSED\n\n   BodyE.\n"
                    .to_string(),
                format!(
                    "{HEAD}        <tip classes=\"x\" collapsible=\"closed\">\n{}",
                    body("BodyE.")
                ),
            ),
            (
                "Title\n=====\n\n.. rubric:: RubricG\n   :heading-level: 2\n\nAfter.\n".to_string(),
                format!(
                    "{HEAD}        <rubric heading-level=\"2\">\n            RubricG\n        \
                     <paragraph>\n            After.\n"
                ),
            ),
        ];
        let kinds = [
            "danger",
            "hint",
            "important",
            "caution",
            "error",
            "attention",
        ];
        let rows = rows.into_iter().chain(kinds.iter().map(|kind| {
            (
                format!("Title\n=====\n\n.. {kind}::\n   :collapsible:\n\n   Body.\n"),
                format!(
                    "{HEAD}        <{kind} collapsible=\"open\">\n{}",
                    body("Body.")
                ),
            )
        }));
        for (source, tree) in rows {
            let out = read_parse(&source, ReadConfig::default());
            assert_eq!(read_tree(&source, ReadConfig::default()), tree, "{source}");
            assert_eq!(printed(&out), Vec::<String>::new(), "{source}");
        }
        for (source, directive, option, value, reason) in [
            (
                "Title\n=====\n\n.. note::\n   :collapsible: bogus\n\n   BodyF.\n",
                "note",
                "collapsible",
                "bogus",
                "\"bogus\" unknown; choose from \"open\" or \"closed\"..",
            ),
            (
                "Title\n=====\n\n.. rubric:: RubricH\n   :heading-level: 9\n\nAfter.\n",
                "rubric",
                "heading-level",
                "9",
                "\"9\" unknown; choose from \"1\", \"2\", \"3\", \"4\", \"5\", or \"6\".",
            ),
        ] {
            let out = read_parse(source, ReadConfig::default());
            let block = source[13..].split("\n\nAfter.").next().unwrap_or_default();
            assert_eq!(
                printed(&out),
                [format!(
                    "<SRC>/index.rst:4: ERROR: Error in \"{directive}\" directive:\n\
                     invalid option value: (option: \"{option}\"; value: '{value}')\n\
                     {reason}\n\n{} [docutils]",
                    block.trim_end()
                )],
                "{source}"
            );
        }
    }

    /// Sphinx 9.1.0's option converters and `CodeBlock.run`
    /// (`SP/sphinx/directives/code.py:30-56`, `:58-75`, `:108-183`;
    /// `SP/sphinx/directives/other.py:36-39`): `code-block` and
    /// `sourcecode` type `:dedent:` `optional_int`, so a bare one runs
    /// `textwrap.dedent` and a negative one is the option ERROR in place
    /// of the block, and dedent the content by `dedent_lines`, which warns
    /// `non-whitespace stripped by dedent` at the `(source, line)` tuple
    /// (`<srcdir>/index.rst.rst:N`); `:lineno-start:` turns `linenos` on
    /// and lands in `highlight_args` after `hl_lines`. `highlight` types
    /// `:linenothreshold:` `positive_int` and the toctree's `:numbered:`
    /// is `int_or_nothing`, whose bad values are the option ERROR too.
    /// Trees and records probed on Sphinx 9.1.0 (`-w`, post-read trees).
    #[test]
    fn code_block_options_convert_and_apply_as_sphinx_does() {
        const HEAD: &str = "<document source=\"<SRC>/index.rst\">\n    <section ids=\"title\" \
                            names=\"title\">\n        <title>\n            Title\n";
        let block = |attrs: &str, text: &str| {
            format!(
                "        <literal_block force=\"0\" highlight_args=\"{attrs}\" \
                 language=\"python\"{}xml:space=\"preserve\">\n{text}",
                if attrs == "{}" {
                    " "
                } else {
                    " linenos=\"1\" "
                }
            )
        };
        let after = "        <paragraph>\n            After.\n";
        let dedented = format!("{HEAD}{}{after}", block("{}", "            x = 1\n"));
        let rows: Vec<(&str, String, Vec<&str>)> = vec![
            (
                "Title\n=====\n\n.. code-block:: python\n   :dedent: 2\n\n     x = 1\n\n\
                 Para.\n\n.. code-block:: python\n   :lineno-start: 10\n\n   y = 2\n",
                format!(
                    "{HEAD}{}        <paragraph>\n            Para.\n{}",
                    block("{}", "            x = 1\n"),
                    block("{'linenostart': 10}", "            y = 2\n")
                ),
                vec![],
            ),
            (
                "Title\n=====\n\n.. code-block:: python\n   :dedent:\n\n      x = 1\n\nAfter.\n",
                dedented.clone(),
                vec![],
            ),
            (
                "Title\n=====\n\n.. sourcecode:: python\n   :dedent:\n\n      x = 1\n\nAfter.\n",
                dedented,
                vec![],
            ),
            (
                "Title\n=====\n\n.. code-block:: python\n   :emphasize-lines: 1\n   \
                 :lineno-start: -3\n   :dedent:\n\n      x = 1\n        y\n\n\
                 .. sourcecode:: python\n   :lineno-start: 5\n   :linenos:\n\n   z\n",
                format!(
                    "{HEAD}{}{}",
                    block(
                        "{'hl_lines': [1], 'linenostart': -3}",
                        "            x = 1\n              y\n"
                    ),
                    block("{'linenostart': 5}", "            z\n")
                ),
                vec![],
            ),
            (
                "Title\n=====\n\n.. code-block:: python\n   :dedent: 8\n\n   x = 1\n",
                format!("{HEAD}{}", block("{}", "")),
                vec!["<SRC>/index.rst.rst:4: WARNING: non-whitespace stripped by dedent"],
            ),
        ];
        for (source, tree, records) in rows {
            let out = read_parse(source, ReadConfig::default());
            assert_eq!(read_tree(source, ReadConfig::default()), tree, "{source}");
            assert_eq!(printed(&out), records, "{source}");
        }
        // The option ERRORs, each in place of its directive.
        for (source, option, value, reason) in [
            (
                "Title\n=====\n\n.. code-block:: python\n   :dedent: -1\n\n   x = 1\n",
                "dedent",
                "'-1'",
                "negative value; must be positive or zero.",
            ),
            (
                "Title\n=====\n\n.. highlight:: python\n   :linenothreshold: 0\n",
                "linenothreshold",
                "'0'",
                "negative or zero value; must be positive.",
            ),
            (
                "Title\n=====\n\n.. highlight:: python\n   :linenothreshold: -3\n",
                "linenothreshold",
                "'-3'",
                "negative or zero value; must be positive.",
            ),
            (
                "Title\n=====\n\n.. toctree::\n   :numbered: x\n",
                "numbered",
                "'x'",
                "invalid literal for int() with base 10: 'x'.",
            ),
        ] {
            let out = read_parse(source, ReadConfig::default());
            let directive = source[16..].split("::").next().unwrap_or_default();
            let block = source[13..].trim_end();
            assert_eq!(
                printed(&out),
                [format!(
                    "<SRC>/index.rst:4: ERROR: Error in \"{directive}\" directive:\n\
                     invalid option value: (option: \"{option}\"; value: {value})\n\
                     {reason}\n\n{block} [docutils]"
                )],
                "{source}"
            );
        }
        // `int_or_nothing`: a bare `:numbered:` is 999, a number itself.
        for (source, numbered) in [
            ("Title\n=====\n\n.. toctree::\n   :numbered:\n", "999"),
            ("Title\n=====\n\n.. toctree::\n   :numbered: -1\n", "-1"),
            ("Title\n=====\n\n.. toctree::\n   :numbered: 2\n", "2"),
        ] {
            let tree = read_tree(source, ReadConfig::default());
            assert!(
                tree.contains(&format!(" numbered=\"{numbered}\" ")),
                "{source}: {tree}"
            );
        }
    }

    /// `CodeBlock.run`'s caption goes through `container_wrapper`
    /// (`SP/sphinx/directives/code.py:78-96`, `:172-183`): a nested parse
    /// of the caption numbered from the content line (`offset =
    /// content_offset`), whose first node's children become the caption,
    /// everything after it dropped, and the name lands after it. So a
    /// caption starting `.. ` is a comment and `>>> ` a doctest block, with
    /// nothing parsed inline, and a leading system_message is Sphinx's
    /// `Invalid caption` WARNING at the directive's line in place of the
    /// block, naming the message's own location: the content line for an
    /// ERROR, line 1 for the unknown directive's INFO, which docutils
    /// locates by `document.current_line`, the line's offset in the
    /// caption's own `<generated text>` plus one. `literalinclude` shares
    /// the wrapper; its caption counts from the line past its option
    /// block. Expected trees are Sphinx 9.1.0's parse-time trees
    /// (`keep_warnings = True`, INFO messages dropped), which
    /// `AutoNumbering` (210) numbers in the read.
    #[test]
    fn a_code_caption_parses_through_container_wrapper() {
        assert_reads(&[
        (
            "Title\n=====\n\n.. code-block:: python\n   :caption: .. :nod:bogus:`x`\n\n   pass\n\nSentinel :nod:sentinel:`z`.\n",
            "<document source=\"<SRC>/index.rst\">\n    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n        <container classes=\"literal-block-wrapper\" literal_block=\"1\">\n            <caption>\n                :nod:bogus:`x`\n            <literal_block force=\"0\" highlight_args=\"{}\" language=\"python\" xml:space=\"preserve\">\n                pass\n        <paragraph>\n            Sentinel \n            <problematic ids=\"id2\" refid=\"id1\">\n                :nod:sentinel:`z`\n            .\n        <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"9\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n            <paragraph>\n                Unknown interpreted text role \"nod:sentinel\".\n",
            &["WARNING: unknown role name: nod:sentinel", "<SRC>/index.rst:9: ERROR: Unknown interpreted text role \"nod:sentinel\". [docutils]"],
        ),
        (
            "Title\n=====\n\n.. code-block:: python\n   :caption: >>> a *open\n\n   pass\n\nSentinel :nod:sentinel:`z`.\n",
            "<document source=\"<SRC>/index.rst\">\n    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n        <container classes=\"literal-block-wrapper\" literal_block=\"1\">\n            <caption>\n                >>> a *open\n            <literal_block force=\"0\" highlight_args=\"{}\" language=\"python\" xml:space=\"preserve\">\n                pass\n        <paragraph>\n            Sentinel \n            <problematic ids=\"id2\" refid=\"id1\">\n                :nod:sentinel:`z`\n            .\n        <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"9\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n            <paragraph>\n                Unknown interpreted text role \"nod:sentinel\".\n",
            &["WARNING: unknown role name: nod:sentinel", "<SRC>/index.rst:9: ERROR: Unknown interpreted text role \"nod:sentinel\". [docutils]"],
        ),
        (
            "Para.\n\n.. code-block:: python\n   :caption: Cap *h\n\n\n   pass\n\n.. code-block:: python\n   :caption: Two *k\n   :name: blk\n\n.. code-block:: python\n   :caption: Three :nod:bogus:`x`\n      *m\n\n   pass\n",
            "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        Para.\n    <container classes=\"literal-block-wrapper\" literal_block=\"1\">\n        <caption>\n            Cap \n            <problematic ids=\"id2\" refid=\"id1\">\n                *\n            h\n        <literal_block force=\"0\" highlight_args=\"{}\" language=\"python\" xml:space=\"preserve\">\n            pass\n    <container classes=\"literal-block-wrapper\" ids=\"blk\" literal_block=\"1\" names=\"blk\">\n        <caption>\n            Two \n            <problematic ids=\"id4\" refid=\"id3\">\n                *\n            k\n        <literal_block force=\"0\" highlight_args=\"{}\" language=\"python\" xml:space=\"preserve\">\n    <container classes=\"literal-block-wrapper\" literal_block=\"1\">\n        <caption>\n            Three \n            <problematic ids=\"id6\" refid=\"id5\">\n                :nod:bogus:`x`\n            \n            <problematic ids=\"id8\" refid=\"id7\">\n                *\n            m\n        <literal_block force=\"0\" highlight_args=\"{}\" language=\"python\" xml:space=\"preserve\">\n            pass\n",
            &["<SRC>/index.rst:7: WARNING: Inline emphasis start-string without end-string. [docutils]", "<SRC>/index.rst:13: WARNING: Inline emphasis start-string without end-string. [docutils]", "WARNING: unknown role name: nod:bogus", "<SRC>/index.rst:17: ERROR: Unknown interpreted text role \"nod:bogus\". [docutils]", "<SRC>/index.rst:17: WARNING: Inline emphasis start-string without end-string. [docutils]"],
        ),
        (
            ".. code-block:: python\n   :caption: ----------\n   :name: nm\n\n   pass\n\n.. code-block:: python\n   :caption: .. bogusdir:: x\n\n   pass\n",
            "<document source=\"<SRC>/index.rst\">\n    <system_message level=\"2\" line=\"1\" source=\"<SRC>/index.rst\" type=\"WARNING\">\n        <paragraph>\n            Invalid caption: <SRC>/index.rst:5: (ERROR/3) Unexpected section title or transition.\n            \n            ----------\n    <system_message level=\"2\" line=\"7\" source=\"<SRC>/index.rst\" type=\"WARNING\">\n        <paragraph>\n            Invalid caption: <SRC>/index.rst:1: (INFO/1) No directive entry for \"bogusdir\" in module \"docutils.parsers.rst.languages.en\".\n            Trying \"bogusdir\" as canonical directive name.\n",
            &["<SRC>/index.rst:5: ERROR: Unexpected section title or transition.\n\n---------- [docutils]", "<SRC>/index.rst:1: WARNING: Invalid caption: <SRC>/index.rst:5: (ERROR/3) Unexpected section title or transition.\n\n---------- [docutils]", "<SRC>/index.rst:10: ERROR: Unknown directive type \"bogusdir\".\n\n.. bogusdir:: x [docutils]", "<SRC>/index.rst:7: WARNING: Invalid caption: <SRC>/index.rst:1: (INFO/1) No directive entry for \"bogusdir\" in module \"docutils.parsers.rst.languages.en\".\nTrying \"bogusdir\" as canonical directive name. [docutils]"],
        ),
        ]);

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("code.py"), "pass\n").unwrap();
        let path = dir.path().join("index.rst").display().to_string();
        let out = parse_rst_full(
            "Title\n=====\n\n.. literalinclude:: code.py\n   :caption: Cap :nod:bogus:`x` *open\n\n.. literalinclude:: code.py\n   :caption: ----------\n\nSentinel :nod:sentinel:`z`.\n",
            &ParseOptions {
                source_path: path.clone(),
                sphinx: true,
                srcdir: Some(dir.path().to_path_buf()),
                ..ParseOptions::default()
            },
        );
        let caption = first(&out.doctree.root, "caption").expect("the first caption");
        assert_eq!(
            caption.pformat(),
            "<caption>\n    Cap \n    <problematic ids=\"id2\" refid=\"id1\">\n        :nod:bogus:`x`\n     \n    <problematic ids=\"id4\" refid=\"id3\">\n        *\n    open\n"
        );
        let records: Vec<String> = [
            "WARNING: unknown role name: nod:bogus",
            "<SRC>/index.rst:7: ERROR: Unknown interpreted text role \"nod:bogus\". [docutils]",
            "<SRC>/index.rst:7: WARNING: Inline emphasis start-string without end-string. [docutils]",
            "<SRC>/index.rst:10: ERROR: Unexpected section title or transition.\n\n---------- [docutils]",
            "<SRC>/index.rst:7: WARNING: Invalid caption: <SRC>/index.rst:10: (ERROR/3) Unexpected section title or transition.\n\n---------- [docutils]",
            "WARNING: unknown role name: nod:sentinel",
            "<SRC>/index.rst:10: ERROR: Unknown interpreted text role \"nod:sentinel\". [docutils]",
        ]
        .iter()
        .map(|record| record.replace("<SRC>/index.rst", &path))
        .collect();
        assert_eq!(printed(&out), records);
    }

    /// Section titles parse inside `only` (`SP/sphinx/directives/other.py:320-368`)
    /// and inside object description and `py:module` content
    /// (`nested_parse_to_nodes(..., allow_section_headings=True)`), each in
    /// a fresh title-style context, and transitions parse there too. An
    /// `only` whose first title style the document already uses goes up to
    /// the section that style belongs under, after the section open there,
    /// while the paragraphs after it stay where they were (`Upper` reaches
    /// the root after `Title`, `Same` lands in `Title` after `Sub`, and
    /// `C`'s in the outer `only` after `B`); otherwise, or inside a note,
    /// it stays. The document's styles come back after it (`Next` is
    /// `Title`'s subsection) and a level the content skips is docutils'
    /// ERROR. Expected trees and records probed on Sphinx 9.1.0 with
    /// `keep_warnings = True`; `py:module`'s is the parse-time tree, before
    /// `MoveModuleTargets` (T10) moves the module target.
    #[test]
    fn section_titles_parse_in_only_and_object_content() {
        assert_reads(&[
        (
            "Title\n=====\n\n.. only:: html\n\n   Inner :nod:inner:`x`\n   --------------------\n\n   Body.\n\nSentinel :nod:sentinel:`z`.\n",
            "<document source=\"<SRC>/index.rst\">\n    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n        <only expr=\"html\">\n            <section ids=\"inner-nod-inner-x\" names=\"inner\\ :nod:inner:`x`\">\n                <title>\n                    Inner \n                    <problematic ids=\"id2\" refid=\"id1\">\n                        :nod:inner:`x`\n                <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"6\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n                    <paragraph>\n                        Unknown interpreted text role \"nod:inner\".\n                <paragraph>\n                    Body.\n        <paragraph>\n            Sentinel \n            <problematic ids=\"id4\" refid=\"id3\">\n                :nod:sentinel:`z`\n            .\n        <system_message backrefs=\"id4\" ids=\"id3\" level=\"3\" line=\"11\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n            <paragraph>\n                Unknown interpreted text role \"nod:sentinel\".\n",
            &["WARNING: unknown role name: nod:inner", "<SRC>/index.rst:6: ERROR: Unknown interpreted text role \"nod:inner\". [docutils]", "WARNING: unknown role name: nod:sentinel", "<SRC>/index.rst:11: ERROR: Unknown interpreted text role \"nod:sentinel\". [docutils]"],
        ),
        (
            "Title\n=====\n\nSub\n---\n\n.. only:: html\n\n   Upper\n   =====\n\n.. only:: latex\n\n   Same\n   ----\n\nAfter.\n\nSub2\n----\n\nTwo.\n",
            "<document source=\"<SRC>/index.rst\">\n    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n        <section ids=\"sub\" names=\"sub\">\n            <title>\n                Sub\n            <paragraph>\n                After.\n        <only expr=\"latex\">\n            <section ids=\"same\" names=\"same\">\n                <title>\n                    Same\n        <section ids=\"sub2\" names=\"sub2\">\n            <title>\n                Sub2\n            <paragraph>\n                Two.\n    <only expr=\"html\">\n        <section ids=\"upper\" names=\"upper\">\n            <title>\n                Upper\n",
            &[],
        ),
        (
            ".. only:: html\n\n   Lone\n   ====\n\n   A\n   ~\n\nTitle\n=====\n\n.. note::\n\n   .. only:: html\n\n      Inner\n      =====\n\n.. only:: html\n\n   B\n   ^\n\n   .. only:: latex\n\n      C\n      ^\n\n      Body.\n\n   D\n   ~\n\n   E\n   ^\n\n   F\n   #\n\nNext\n~~~~\n\nText.\n",
            "<document source=\"<SRC>/index.rst\">\n    <only expr=\"html\">\n        <section ids=\"lone\" names=\"lone\">\n            <title>\n                Lone\n            <section ids=\"a\" names=\"a\">\n                <title>\n                    A\n    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n        <note>\n            <only expr=\"html\">\n                <section ids=\"inner\" names=\"inner\">\n                    <title>\n                        Inner\n        <only expr=\"html\">\n            <section ids=\"b\" names=\"b\">\n                <title>\n                    B\n                <section ids=\"d\" names=\"d\">\n                    <title>\n                        D\n            <only expr=\"latex\">\n                <section ids=\"c\" names=\"c\">\n                    <title>\n                        C\n                    <paragraph>\n                        Body.\n            <section ids=\"e\" names=\"e\">\n                <title>\n                    E\n                <system_message level=\"3\" line=\"37\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n                    <paragraph>\n                        Inconsistent title style: skip from level 1 to 3.\n                    <literal_block xml:space=\"preserve\">\n                        F\n                        #\n                    <paragraph>\n                        Established title styles: ^ ~\n        <section ids=\"next\" names=\"next\">\n            <title>\n                Next\n            <paragraph>\n                Text.\n",
            &["<SRC>/index.rst:37: ERROR: Inconsistent title style: skip from level 1 to 3.\n\nF\n#\n\nEstablished title styles: ^ ~ [docutils]"],
        ),
        (
            ".. py:class:: C\n\n   Sec :nod:x:`y`\n   --------------\n\n   Body.\n\n   ----------\n\n   .. py:method:: m()\n\n      Inner\n      ~~~~~\n\n      More.\n\nSentinel :nod:sentinel:`z`.\n",
            "<document source=\"<SRC>/index.rst\">\n    <index entries=\"('single',\\ 'C\\ (built-in\\ class)',\\ 'C',\\ '',\\ None)\">\n    <desc classes=\"py class\" desctype=\"class\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"class\">\n        <desc_signature _toc_name=\"C\" _toc_parts=\"('C',)\" class=\"\" classes=\"sig sig-object\" fullname=\"C\" ids=\"C\" module=\"True\">\n            <desc_annotation xml:space=\"preserve\">\n                <desc_sig_keyword classes=\"k\">\n                    class\n                <desc_sig_space classes=\"w\">\n                     \n            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                C\n        <desc_content>\n            <section ids=\"sec-nod-x-y\" names=\"sec\\ :nod:x:`y`\">\n                <title>\n                    Sec \n                    <problematic ids=\"id2\" refid=\"id1\">\n                        :nod:x:`y`\n                <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"3\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n                    <paragraph>\n                        Unknown interpreted text role \"nod:x\".\n                <paragraph>\n                    Body.\n                <transition>\n                <index entries=\"('single',\\ 'm()\\ (C\\ method)',\\ 'C.m',\\ '',\\ None)\">\n                <desc classes=\"py method\" desctype=\"method\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"method\">\n                    <desc_signature _toc_name=\"C.m()\" _toc_parts=\"('C', 'm')\" class=\"C\" classes=\"sig sig-object\" fullname=\"C.m\" ids=\"C.m\" module=\"True\">\n                        <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                            m\n                        <desc_parameterlist xml:space=\"preserve\">\n                    <desc_content>\n                        <section ids=\"inner\" names=\"inner\">\n                            <title>\n                                Inner\n                            <paragraph>\n                                More.\n    <paragraph>\n        Sentinel \n        <problematic ids=\"id4\" refid=\"id3\">\n            :nod:sentinel:`z`\n        .\n    <system_message backrefs=\"id4\" ids=\"id3\" level=\"3\" line=\"17\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"nod:sentinel\".\n",
            &["WARNING: unknown role name: nod:x", "<SRC>/index.rst:3: ERROR: Unknown interpreted text role \"nod:x\". [docutils]", "WARNING: unknown role name: nod:sentinel", "<SRC>/index.rst:17: ERROR: Unknown interpreted text role \"nod:sentinel\". [docutils]"],
        ),
        (
            "Title\n=====\n\n.. py:module:: m\n\n   Sec :nod:x:`y`\n   ==============\n\n   Body.\n\nAfter.\n",
            "<document source=\"<SRC>/index.rst\">\n    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n        <index entries=\"('pair',\\ 'module;\\ m',\\ 'module-m',\\ '',\\ None)\">\n        <target ids=\"module-m\" ismod=\"1\">\n        <section ids=\"sec-nod-x-y\" names=\"sec\\ :nod:x:`y`\">\n            <title>\n                Sec \n                <problematic ids=\"id2\" refid=\"id1\">\n                    :nod:x:`y`\n            <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"6\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n                <paragraph>\n                    Unknown interpreted text role \"nod:x\".\n            <paragraph>\n                Body.\n        <paragraph>\n            After.\n",
            &["WARNING: unknown role name: nod:x", "<SRC>/index.rst:6: ERROR: Unknown interpreted text role \"nod:x\". [docutils]"],
        ),
        ]);
    }
}
