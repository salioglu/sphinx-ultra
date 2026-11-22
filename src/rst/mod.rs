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

use std::collections::HashMap;

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
    /// directive's entries. They ride the record (and therefore the
    /// document cache) because the parser has no warning sink, and because
    /// a cache hit that skipped the parse must still reproduce them.
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
    /// The parse's record sequence number, drawn at the registration (see
    /// [`ProgramOptionRecord::seq`]).
    pub seq: u32,
}

/// What the parse layer hands the environment besides the doctree itself:
/// state that lives in the parser (the docutils id/name registry, Sphinx's
/// `env.ref_context`) and dies with it, but that env collectors need.
///
/// Named for its original single job — the `document.nameids` snapshot of
/// the parse's name table ([`DocumentState::names`]), which wave 4's
/// std-domain label harvest reads. Intended to eventually ride the document
/// cache, so it stays serde-serializable and cheap to clone.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RegistryExport {
    /// `(name, id, explicit)`, one entry per registered name, in
    /// registration order. `id` is `None` once a name has been duplicated
    /// away.
    pub nameids: Vec<(String, Option<String>, bool)>,
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
    /// Diagnostics the parse raised through Sphinx's *logger* rather than
    /// into the tree, which have nowhere else to go: docutils turns a
    /// directive error into a `system_message` node, but a Sphinx directive
    /// calling `logger.warning` produces no node at all. They ride the
    /// export (and therefore the document cache) for the same reason
    /// [`ToctreeRecord::warnings`] does — a cache hit that skipped the parse
    /// must still reproduce them.
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
    /// - `Node` is `get_node_location`'s `abspath(source)`: Sphinx resolves
    ///   the included file's cwd-relative spelling against the cwd, which
    ///   the oracle runs in the source directory, so `<srcdir>/part.rst:N`
    ///   (`SP/sphinx/util/logging.py:554-562`); a node without a line
    ///   prints `path:`;
    /// - `None` prints nothing.
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
        let doc2path = |name: &str| srcdir.join(format!("{name}.rst")).display().to_string();
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
                let path = srcdir.join(table_path(source)).display().to_string();
                match line {
                    0 => Location::Verbatim(format!("{path}:")),
                    line => path_line(path, line),
                }
            }
        }
    }
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
        crate::diagnostics::Diagnostic {
            location: self.location.render(sources, srcdir, docname),
            severity: crate::diagnostics::Severity::Warning,
            message: self.message.clone(),
            category: crate::diagnostics::Category::None,
        }
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
    /// `sphinx.util.nodes.make_id` prefix that asked for one
    /// (`cmdoption-myprog`, `confval`, …): per document and category, from
    /// 0, like [`Self::index_serial`].
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
    /// `logger.warning` diagnostics raised while running directives.
    pub(crate) log_warnings: Vec<ParseLogWarning>,
    /// Parse-recorded dependency paths and included docnames (see
    /// [`RegistryExport::dependencies`]); sphinx mode only.
    pub(crate) dependency_records: Vec<String>,
    pub(crate) included_records: Vec<String>,
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
    if opts.trace_ids {
        parser.state.ids.enable_trace();
    }
    parser.parse_document_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctree::{kinds, Node, Uid};
    use serde_json::{json, Value};

    /// The complete current [`RegistryExport`] shape, with one record of
    /// every kind, so that a guard below can remove exactly ONE field and
    /// know the decode failed for no other reason.
    const COMPLETE_REGISTRY: &str = r#"{"nameids":[],"index_serial":0,
        "program_options":[{"source":0,"program":null,"name":"-f","node_id":"a","seq":0}],
        "std_objects":[{"source":0,"objtype":"envvar","name":"P","node_id":"b","line":1,
            "seq":1}],
        "py_objects":[{"fullname":"m.f","objtype":"function","node_id":"m.f",
            "aliased":false,"source":0,"lineno":1,"seq":2}],
        "py_modules":[{"name":"m","node_id":"module-m","synopsis":"","platform":"",
            "deprecated":false,"source":0,"lineno":1,"seq":3}],
        "glossary_terms":[{"term":"T","id":"term-T","seq":4}],
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
        // (id1), alpha's second (id2), and the tree puts alpha first.
        let source = ".. glossary::\n   :sorted:\n\n   zeta\n      Last letter [#]_.\n\n   \
                      alpha\n      First letter [#]_.\n\n.. [#] z\n.. [#] a\n";
        let out = sphinx_parse(source);
        let d = out.state.registries_dump(&out.doctree.root);
        assert_eq!(
            d["autofootnote_refs"],
            json!([auto_ref("id1", true), auto_ref("id2", true)])
        );
        assert_eq!(footnote_ref_tree_order(&out), ["id2", "id1"]);

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
    /// the include's padding line, `n + 4` for `n` included lines.
    /// Every row probed on docutils 0.22.4 (`reporter.get_source_and_line(None)`
    /// after the parse).
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
        for (source, padding, line) in [
            ("Para.\n\n.. include:: para.rst\n", "para.rst", 5),
            ("Para.\n\n.. include:: list.rst\n", "list.rst", 6),
            ("Para.\n\n.. include:: target.rst\n", "target.rst", 5),
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
                seq: 1,
            }]
        );
        let seqs: Vec<u32> = reporter_records(&out)
            .into_iter()
            .map(|(seq, ..)| seq)
            .collect();
        assert_eq!(seqs, [0, 2]);
    }

    /// The `reference` an image's `:target:` wraps is appended right after
    /// the option block's nested state machine cleared the line, so at
    /// document or section level it has the source and no line
    /// (`docutils-0439`, [DS §6.1]); one a nested parse appends to a
    /// container not yet in the document has neither, the default rule,
    /// and gets no override. Probed on docutils 0.22.4 (`reference.source`
    /// and `.line` after the parse).
    #[test]
    fn loc_overrides_for_image_target_refs() {
        for source in [
            ".. image:: pic.png\n   :target: sometarget_\n",
            ".. image:: pic.png\n   :target: https://x/\n",
            "Title\n=====\n\n.. image:: pic.png\n   :target: sometarget_\n",
        ] {
            let out = sphinx_parse(source);
            let reference = first(&out.doctree.root, kinds::REFERENCE).expect("a reference");
            let uid = reference
                .uid()
                .expect("an override gives the node an identity");
            let expected = DocLoc {
                source: Some(SourceRef::Table(0)),
                line: None,
            };
            assert_eq!(
                out.state.loc_overrides,
                std::collections::HashMap::from([(uid, expected)]),
                "{source:?}"
            );
        }
        for source in [
            ".. note::\n\n   .. image:: pic.png\n      :target: sometarget_\n",
            ".. figure:: pic.png\n   :target: sometarget_\n",
            ".. |x| image:: pic.png\n   :target: sometarget_\n",
            "- item\n\n  .. image:: pic.png\n     :target: sometarget_\n",
        ] {
            assert!(
                sphinx_parse(source).state.loc_overrides.is_empty(),
                "{source:?}"
            );
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
    #[test]
    fn log_locations_render_like_sphinx() {
        use crate::diagnostics::Location;
        let srcdir = std::path::Path::new("/src");
        let sources = ["/src/index.rst".to_string(), "part.rst".to_string()];
        let path = |path: &str, line: Option<u32>| Location::Path {
            path: path.to_string(),
            line,
        };
        let rows = [
            (LogLocation::None, Location::None),
            (LogLocation::Doc, path("/src/index.rst", None)),
            (LogLocation::DocLine(7), path("/src/index.rst", Some(7))),
            (LogLocation::DocLine(0), path("/src/index.rst", None)),
            (
                LogLocation::Node { source: 0, line: 3 },
                path("/src/index.rst", Some(3)),
            ),
            (
                LogLocation::Node { source: 1, line: 6 },
                path("/src/part.rst", Some(6)),
            ),
            (
                LogLocation::Node { source: 1, line: 0 },
                Location::Verbatim("/src/part.rst:".to_string()),
            ),
            (
                LogLocation::Tuple { source: 0, line: 1 },
                path("/src/index.rst.rst", Some(1)),
            ),
            (
                LogLocation::Tuple { source: 1, line: 6 },
                path("/src/part.rst.rst", Some(6)),
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
            "/src/part.rst.rst:12: WARNING: non-whitespace stripped by dedent"
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

    /// The end-of-parse assertion ([DS §8.2]: every registered `Uid` is
    /// reachable exactly once from the root or from `detached`), checked
    /// explicitly on every input of the three parse corpora: each
    /// `doctree_differential.json` case in docutils mode and again in
    /// sphinx mode, each `sphinx_doctree_differential.json` case in sphinx
    /// mode with its `conf`, and every document of every
    /// `env_differential.json` project, parsed the way the build parses it
    /// (sphinx mode, its srcdir, docname and docname set).
    ///
    /// Only the docutils corpus registers nodes: the other two hold no
    /// reference, footnote, citation or substitution definition (their
    /// targets are labels and `refuri` targets, which no registry keeps).
    /// It registers 186 nodes over the two modes and keeps one detached
    /// subtree per mode (`substitutions.replace_auto_footnote_error`), the
    /// floors below; its parses mint 190 identities, the four more being
    /// the image `:target:` references that hold only a location override.
    #[test]
    fn every_registered_uid_is_reachable_once() {
        /// Per corpus: parses checked, distinct nodes registered, detached
        /// subtrees kept.
        #[derive(Debug, Default)]
        struct Tally {
            parses: usize,
            registered: usize,
            detached: usize,
        }
        let mut tallies: [Tally; 3] = Default::default();
        let mut check = |corpus: usize, label: &str, output: &ParseOutput| {
            let state = &output.state;
            let misplaced = state.misplaced_registrations(&output.doctree.root);
            assert!(misplaced.is_empty(), "{label}: {misplaced:?}");
            let tally = &mut tallies[corpus];
            tally.parses += 1;
            // Every registered node holds one identity, and so does a node
            // with a location override.
            tally.registered += state.next_uid as usize;
            tally.detached += state.detached.len();
        };

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
                check(0, &label, &parse_rst_full(source, &options));
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
            check(1, name, &parse_rst_full(source, &options));
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
                check(2, &label, &parse_rst_full(source, &options));
            }
        }

        // 735 docutils cases twice, 489 sphinx cases, 84 env documents; the
        // check is not vacuous.
        let [docutils, sphinx, env] = &tallies;
        assert!(
            docutils.parses >= 2 * 735 && sphinx.parses >= 489 && env.parses >= 84,
            "too few parses checked: {tallies:?}"
        );
        assert!(
            docutils.registered >= 2 * 93 && docutils.detached >= 2,
            "the docutils corpus stopped exercising the check: {tallies:?}"
        );
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
            // The inline target an embedded alias builds is no Referential.
            assert_eq!(first(root, kinds::TARGET).and_then(Node::rawsource), None);
        }
    }
}
