//! Recursive-descent RST parser with docutils-0.22.4 fidelity (M2 wave 1:
//! block grammar only — the inline parser arrives in wave 2).
//!
//! Fidelity contract: output `pformat()` is byte-identical to
//! `docutils.parsers.rst.Parser` parse-layer output for the construct set in
//! `tests/fixtures/doctree_differential.json`. Transforms (doctitle
//! promotion, target propagation, transition hoisting, message filtering)
//! are explicitly NOT applied here: sphinx mode runs them afterwards, as the
//! separate read-transform pass in [`crate::transforms`]. Behavior sources:
//! the committed differential fixture and the probe notes in
//! docs/superpowers/plans/2026-08-07-m2-wave1-probes.md.

pub(crate) mod block;
pub mod diagnostics;
pub(crate) mod digits;
pub mod inline;
pub mod lines;
mod punctuation;

use crate::doctree::Doctree;

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
    /// Sphinx's `highlight_language` config value (`config.py:257`, default
    /// `'default'`): the language a sphinx-mode `code-block` with no
    /// argument takes when no `.. highlight::` is in force
    /// (`self.env.current_document.highlight_language or
    /// self.config.highlight_language`, `sphinx/directives/code.py:157-166`).
    /// A `::` literal block takes none at read time — the write phase's
    /// HighlightLanguageTransform stamps it. Ignored outside sphinx mode.
    pub highlight_language: String,
}

/// Sphinx's default `highlight_language` (`config.py:257`).
pub const DEFAULT_HIGHLIGHT_LANGUAGE: &str = "default";

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
            highlight_language: DEFAULT_HIGHLIGHT_LANGUAGE.to_string(),
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
    /// Source-table index of the `.. toctree::` line — the section-
    /// numbering warning (`location=toctreenode`) names this source's
    /// path. Not `#[serde(default)]` (see [`DirectiveRecord::source`]).
    pub source: u16,
    /// 1-based line of the directive, within `source`.
    pub line: u32,
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
    /// The registration's place in the document's diagnostics sequence
    /// ([`diagnostics::Diagnostic::seq`]): `note_object` logs its
    /// duplicate warning at parse time, between the records around it,
    /// but only the environment replay knows whether it is a duplicate.
    /// Not `#[serde(default)]` (cache-shape rule, see
    /// [`RegistryExport::program_options`]).
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
    /// Where the duplicate-description warning belongs in the document's
    /// diagnostics sequence — see [`PyObjectRecord::seq`].
    pub seq: u32,
}

/// One `StandardDomain._note_term` call the `glossary` directive made
/// (`make_glossary_term`, `sphinx/domains/std/__init__.py:375-407`): the
/// term registers as a `term` object *and* under its lowercased text, and
/// `note_object` warns about a duplicate right there, at parse time —
/// after the term's own inline parse, before its definition's.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GlossaryTermRecord {
    /// `termtext = term.astext()`, taken from the parsed term before the
    /// index node is appended (`domains/std/__init__.py:389`).
    pub term: String,
    pub node_id: String,
    /// Source-table index of the term's line (a glossary inside an
    /// included file attributes its terms to that file). Not
    /// `#[serde(default)]` (cache-shape rule, see
    /// [`RegistryExport::program_options`]).
    pub source: u16,
    /// The line the duplicate warning prints: `location=term`, whose line
    /// `make_glossary_term` set from the content item's **0-based** offset
    /// (`self.content.items`, from `abs_line_offset()`), so one *less* than
    /// the term's own 1-based line. Verified against sphinx 9.1.0: a term
    /// on source line 8 reports `b.rst:7`, one on line 16 `second.rst:15`.
    pub line: u32,
    /// Where the duplicate warning belongs in the document's diagnostics
    /// sequence — see [`PyObjectRecord::seq`].
    pub seq: u32,
}

/// One `CitationDomain.note_citation` call (`sphinx/domains/citation.py:
/// 70-82`), which CitationDefinitionTransform makes for every citation of
/// the document at read time (priority 619, `:133-148`) — recorded by the
/// read-transform pass ([`crate::transforms`]), not the parse. The call
/// registers `label -> (docname, node_id, line)` with the environment and
/// warns `duplicate citation %s, other instance in %s` when the label is
/// already there, from this or any other document: that decision needs the
/// environment, so the merge phase replays the record
/// ([`crate::env::citation_domain`]) and puts the warning at `seq`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CitationRecord {
    /// `node[0].astext()`: the citation's label as written (`CIT`, not the
    /// normalized name `cit`).
    pub label: String,
    /// `node['ids'][0]`.
    pub node_id: String,
    /// Source-table index of the citation (`location=node`): a citation in
    /// an included file is that file's. Not `#[serde(default)]`
    /// (cache-shape rule, see [`RegistryExport::program_options`]).
    pub source: u16,
    /// `node.line`: the citation marker's line in `source` — the duplicate
    /// warning's line, and the one `check_consistency`'s `Citation [%s] is
    /// not referenced.` prints (`:88-97`).
    pub line: u32,
    /// Where the duplicate warning belongs in the document's diagnostics
    /// sequence: the number the transform spent when it made the call,
    /// among the records the transforms around it print — see
    /// [`PyObjectRecord::seq`].
    pub seq: u32,
}

/// What the parse layer hands the environment besides the doctree itself:
/// state that lives in the parser (the docutils id/name registry, Sphinx's
/// `env.ref_context`) and dies with it, but that env collectors need.
///
/// Named for its original single job — the `document.nameids` snapshot
/// harvested from [`crate::doctree::ids::IdRegistry`] right before it drops,
/// which wave 4's std-domain label harvest reads. Intended to eventually
/// ride the document cache, so it stays serde-serializable and cheap to
/// clone.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RegistryExport {
    /// `(name, id, explicit)`, one entry per registered name. `id` is
    /// `None` once a name has been duplicated away.
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
    /// The glossary terms the `glossary` directives registered, in the
    /// order they ran — source order, which a `:sorted:` glossary's tree no
    /// longer shows (`GlossarySorter` runs afterwards; probe-pinned by
    /// `a_sorted_glossary_registers_its_terms_in_source_order`). A replay
    /// from the finished doctree could neither recover that order nor put
    /// a term's duplicate warning among the parse's other records. Not
    /// `#[serde(default)]` — see [`Self::program_options`]: a stale entry
    /// decoding with no terms would drop every glossary term from a
    /// re-read document's environment.
    pub glossary_terms: Vec<GlossaryTermRecord>,
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
    /// Every diagnostic the parse raised, in creation order (`seq`): the
    /// docutils reporter messages of level >= 2, recorded as each was
    /// created — including those whose node never reached the tree — and
    /// the directives' and domains' `logger.warning`s (toctree resolution,
    /// malformed option descriptions, py signature warnings, the
    /// literalinclude reader) in between; see [`diagnostics::Reporter`].
    /// They ride the export (and therefore the document cache) because a
    /// cache hit that skipped the parse must still reproduce them. Not
    /// `#[serde(default)]` — see [`Self::program_options`].
    pub diagnostics: Vec<diagnostics::Diagnostic>,
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
    /// The citation registrations the read transforms made
    /// (CitationDefinitionTransform, priority 619), one per citation in
    /// document order: the one environment registration a read transform
    /// makes whose warning depends on other documents. Empty from the parse
    /// itself; [`crate::transforms::apply_read_transforms`] fills it. Not
    /// `#[serde(default)]` — see [`Self::program_options`]: a stale entry
    /// decoding with no citations would register none of the document's.
    pub citations: Vec<CitationRecord>,
    /// What MetadataCollector (`doctree-read`, priority 880) read off the
    /// document's docinfo before taking it out of the tree — the merge
    /// phase's `env.metadata[docname]`. Empty from the parse itself and for
    /// a document without a docinfo;
    /// [`crate::transforms::apply_read_transforms`] fills it, because the
    /// read has to see the tree before FilterSystemMessages does
    /// ([`crate::env::metadata`]). Not `#[serde(default)]` — see
    /// [`Self::program_options`]: a stale entry decoding with no metadata
    /// would drop a re-read document's `:orphan:` and `:tocdepth:`.
    pub metadata: crate::env::metadata::Metadata,
}

#[cfg(test)]
impl RegistryExport {
    /// The logger-channel records — the directives' and domains' own
    /// `logger.warning`s — for the tests that pin them.
    pub(crate) fn log_warnings(&self) -> Vec<&diagnostics::Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.channel == diagnostics::DiagnosticChannel::Logger)
            .collect()
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
    /// The parser's id/name registry as the parse left it (docutils'
    /// `document.ids`/`nameids`/`nametypes` and the per-prefix id
    /// counters), handed on so a later pass over this doctree continues
    /// numbering where the parse stopped instead of re-deriving it. Lives
    /// only as long as the parse output: never serialized.
    pub ids: crate::doctree::ids::IdRegistry,
    /// The document's diagnostics counter as the parse left it: the `seq`
    /// ([`diagnostics::Diagnostic::seq`]) its next record takes. Every
    /// number below it is spent — on [`RegistryExport::diagnostics`] and on
    /// the registrations whose duplicate warnings the merge phase replays
    /// ([`PyObjectRecord::seq`] and its kin) — so the read-transform pass
    /// continues from here ([`crate::transforms::apply_read_transforms`])
    /// rather than from the highest recorded diagnostic, which a later
    /// registration can outnumber. Never serialized.
    pub next_seq: u32,
    /// Where docutils' reporter locates a message raised after the parse
    /// with no node to locate it by — the Substitutions line-length error,
    /// the anonymous-hyperlink mismatch (research `2026-09-30-m2-wave5-
    /// transforms.md` §9.3): the `(source, line)` one past the last line of
    /// the top-level input, spliced `include` lines counted. Its
    /// `get_source_and_line()` is still bound to the finished top-level
    /// state machine (`states.py:244-246`), whose cursor stops there
    /// (`statemachine.py:358-377`) — unless the last top-level construct
    /// moved it: past the end, with no line at all (`None`), when its nested
    /// list parse ran to the end of the input, or onto the last line after
    /// a final `::` paragraph (the parser's `TopCursor`). `None` too for an
    /// input without lines (docutils: no line either). Never serialized.
    pub end_of_input: Option<(u16, u32)>,
}

/// The stack every thread that parses a document or runs the read
/// transforms on it gets: 64 MiB — address space reserved when the thread
/// starts, committed only as it is touched.
///
/// The parser recurses once a nesting level and stops at its 200-level
/// guard (`MAX_NEST_DEPTH`, `block.rs`), which must be reachable on every
/// input — it stands in for the `RecursionError` that ends a `sphinx-build`
/// run there. A directive level is the costliest: probed in a debug build,
/// the parse, the read transforms and a `pformat` of 260 nested
/// admonitions, containers or `py:function`s need more than 4 MiB and fit
/// in 5 (260 nested `note`s fit in 4, bullet lists in 2), where Rust's
/// default 2 MiB for a spawned thread overflows at about 90 nested `note`s.
/// 64 MiB leaves twelve times the deepest need measured, for heavier
/// directives and for what a release build's smaller frames never need.
pub const PARSE_STACK_SIZE: usize = 64 * 1024 * 1024;

thread_local! {
    /// Whether this thread was started by this crate with
    /// [`PARSE_STACK_SIZE`] (the build's read pool, [`on_parse_stack`]).
    static ON_PARSE_STACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Record that the current thread has [`PARSE_STACK_SIZE`] — for a thread
/// pool's start handler that built its threads with it (and for a test
/// that marks a smaller thread on purpose, to keep the work on it).
pub(crate) fn mark_parse_stack_thread() {
    ON_PARSE_STACK.with(|marked| marked.set(true));
}

/// Run `work` on a thread with [`PARSE_STACK_SIZE`]: right here when this
/// thread already has it, otherwise on a scoped thread started with it
/// (a panic in `work` resumes here). Every public entry point that parses
/// or runs the read transforms goes through this, so no caller's thread —
/// a test's, a library user's, the main thread — can be too small for the
/// nesting guard. Should the system refuse a thread that size (a strict
/// overcommit policy, an address-space limit), `work` runs here, on
/// whatever stack this thread has — as it did before the guard needed the
/// room — and the first refusal in a process logs a warning that deep
/// nesting can now overflow that stack.
///
/// It is also the batching idiom: an entry point called from a thread
/// without the stack starts a thread per call, while a whole loop of
/// calls inside one `on_parse_stack` —
/// `on_parse_stack(|| sources.iter().map(|s| parse_rst(s, &opts)).collect::<Vec<_>>())`
/// — starts one thread for the batch, every call in it running in place.
pub fn on_parse_stack<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    if ON_PARSE_STACK.with(std::cell::Cell::get) {
        return work();
    }
    on_new_stack(PARSE_STACK_SIZE, work)
}

/// [`on_parse_stack`]'s spawn: `work` on a scoped thread with `stack`
/// bytes, marked as having the parse stack — or here, if the system
/// refuses the thread ([`report_refused_stack`] says so, once).
fn on_new_stack<T: Send>(stack: usize, work: impl FnOnce() -> T + Send) -> T {
    #[cfg(test)]
    THREADS_STARTED.with(|count| count.set(count.get() + 1));
    let mut work = Some(work);
    let ran = std::thread::scope(|scope| {
        let slot = &mut work;
        let spawned =
            std::thread::Builder::new()
                .stack_size(stack)
                .spawn_scoped(scope, move || {
                    mark_parse_stack_thread();
                    slot.take().map(|work| work())
                });
        match spawned {
            Ok(thread) => thread
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            Err(error) => {
                report_refused_stack(&REFUSED_STACK_REPORTED, stack, &error);
                None
            }
        }
    });
    match ran {
        Some(out) => out,
        None => (work.take().expect("work runs once"))(),
    }
}

/// Whether [`on_new_stack`] has warned about a refused thread yet: it says
/// so once a process, not once a parse.
static REFUSED_STACK_REPORTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Warn, unless `reported` says it was done already, that a thread with
/// `stack` bytes was refused and the work runs on the caller's own stack —
/// which may be too small for the nesting guard, so the warning is the one
/// hint a user gets before a deep document aborts the process.
fn report_refused_stack(
    reported: &std::sync::atomic::AtomicBool,
    stack: usize,
    error: &std::io::Error,
) {
    if reported.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    log::warn!(
        "the system refused a thread with a {} MiB stack to parse on ({error}), so parsing \
         runs on the calling thread's own stack: a deeply nested document (on the order of a \
         hundred nested directives) can now overflow it and abort the process before the \
         parser's 200-level nesting guard reports it. Reported once per process.",
        stack / (1024 * 1024)
    );
}

/// Whether the current thread has the parse stack, for the tests of the
/// threads that are given it.
#[cfg(test)]
pub(crate) fn has_parse_stack() -> bool {
    ON_PARSE_STACK.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    /// How many times [`on_new_stack`] was entered from this thread — each
    /// entry starts (or tries to start) one thread — for the tests that
    /// count them.
    static THREADS_STARTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// [`THREADS_STARTED`] for the current thread.
#[cfg(test)]
fn threads_started() -> usize {
    THREADS_STARTED.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    /// The `warn!`/`error!` records [`capture_warnings`] collected on this
    /// thread.
    static CAPTURED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The test logger: keeps each thread's WARN-and-above records apart, so
/// tests running in parallel never see each other's.
#[cfg(test)]
struct CaptureLogger;

#[cfg(test)]
impl log::Log for CaptureLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            CAPTURED.with(|captured| captured.borrow_mut().push(record.args().to_string()));
        }
    }

    fn flush(&self) {}
}

/// Run `work` and return what it logged at WARN or above on this thread.
/// Installs [`CaptureLogger`] for the test process on first use.
#[cfg(test)]
pub(crate) fn capture_warnings<T>(work: impl FnOnce() -> T) -> (T, Vec<String>) {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        log::set_logger(&CaptureLogger).expect("no other test installs a logger");
        log::set_max_level(log::LevelFilter::Warn);
    });
    CAPTURED.with(|captured| captured.borrow_mut().clear());
    let out = work();
    (out, CAPTURED.with(|captured| captured.take()))
}

/// Parse RST source into a doctree. Total: never panics, never errors —
/// problems become `system_message` nodes, exactly like docutils.
///
/// Like [`parse_rst_full`], it starts a [`PARSE_STACK_SIZE`] thread per
/// call from a thread without one; to parse many documents, run the loop
/// inside [`on_parse_stack`] and every call in it runs in place.
pub fn parse_rst(source: &str, opts: &ParseOptions) -> Doctree {
    parse_rst_full(source, opts).doctree
}

/// [`parse_rst`] with every record the build pipeline consumes. Runs on a
/// [`PARSE_STACK_SIZE`] thread, whatever thread calls it — a new one per
/// call unless the caller's thread has the stack, so a batch of documents
/// belongs inside one [`on_parse_stack`], where every call runs in place.
pub fn parse_rst_full(source: &str, opts: &ParseOptions) -> ParseOutput {
    on_parse_stack(|| {
        let mut parser = block::BlockParser::new(source, &opts.source_path);
        parser.sphinx = opts.sphinx;
        parser.docname = opts.docname.clone();
        parser.found_docs = opts.found_docs.clone();
        parser.exclude_patterns = opts.exclude_patterns.clone();
        parser.py = opts.py.clone();
        parser.srcdir = opts.srcdir.clone();
        parser.source_encoding = opts.source_encoding.clone();
        parser.config_highlight_language = opts.highlight_language.clone();
        parser.parse_document_full()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 260 nested admonitions — the costliest nesting per level — past the
    /// 200-level guard.
    fn nested_admonitions() -> String {
        let mut source = String::new();
        for depth in 0..260 {
            source.push_str(&"   ".repeat(depth));
            source.push_str(".. admonition:: T\n\n");
        }
        source
    }

    fn guard_records(diagnostics: &[diagnostics::Diagnostic]) -> usize {
        diagnostics
            .iter()
            .filter(|d| d.text == "Maximum nesting depth exceeded; deeper content skipped.")
            .count()
    }

    /// `work` on a thread with a 64 KiB stack: far less than a debug build
    /// needs for [`nested_admonitions`] — the parse between 4 and 5 MiB,
    /// the read transforms of the parsed tree alone between 64 and 128 KiB
    /// (probed) — so it reaches the end only if each entry point moves its
    /// own work onto a [`PARSE_STACK_SIZE`] thread.
    fn on_a_small_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(work)
            .unwrap()
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }

    /// Whether any `system_message` is left in the tree (an explicit
    /// stack: the tree is deep).
    fn has_a_message(root: &crate::doctree::Node) -> bool {
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if node.kind == crate::doctree::kinds::SYSTEM_MESSAGE {
                return true;
            }
            stack.extend(node.children.iter());
        }
        false
    }

    /// Every public entry point that parses or transforms runs on a
    /// [`PARSE_STACK_SIZE`] thread whatever thread calls it, so the nesting
    /// guard, not the caller's stack, ends deep nesting. Each is called
    /// from a small thread on its own: the transforms get a tree parsed
    /// beforehand, and FilterSystemMessages taking the guard's message out
    /// of it shows they ran to the end. (The deep results are dropped on a
    /// parse-stack thread: their drop recurses once a level.)
    #[test]
    fn the_entry_points_reach_the_nesting_guard_from_a_small_thread() {
        fn opts() -> ParseOptions {
            ParseOptions {
                sphinx: true,
                ..Default::default()
            }
        }
        let parsed = on_a_small_thread(|| parse_rst_full(&nested_admonitions(), &opts()));
        assert_eq!(
            guard_records(&parsed.registry.diagnostics),
            1,
            "parse_rst_full"
        );
        assert!(on_parse_stack(|| has_a_message(&parsed.doctree.root)));

        let transformed = on_a_small_thread(move || {
            let mut out = parsed;
            crate::transforms::apply_read_transforms(
                &mut out.doctree,
                std::mem::take(&mut out.ids),
                out.next_seq,
                out.end_of_input,
                "index",
                &crate::transforms::TransformConfig::default(),
                &mut out.registry,
            );
            out
        });
        assert!(
            !on_parse_stack(|| has_a_message(&transformed.doctree.root)),
            "apply_read_transforms ran to FilterSystemMessages"
        );

        let read = on_a_small_thread(|| {
            crate::transforms::parse_and_transform(
                &nested_admonitions(),
                &opts(),
                &crate::transforms::TransformConfig::default(),
            )
        });
        assert_eq!(guard_records(&read.1), 1, "parse_and_transform");
        on_parse_stack(move || drop((transformed, read)));
    }

    /// A thread with the parse stack runs the work in place; one without it
    /// hands the work to a new thread, marked as having the stack.
    #[test]
    fn on_parse_stack_runs_in_place_only_where_the_stack_is() {
        let here = std::thread::current().id();
        let (there, marked) = on_parse_stack(|| (std::thread::current().id(), has_parse_stack()));
        assert_ne!(there, here);
        assert!(marked);
        let nested = on_parse_stack(|| {
            let outer = std::thread::current().id();
            on_parse_stack(|| std::thread::current().id()) == outer
        });
        assert!(nested, "a marked thread keeps the work");
    }

    /// The batching idiom the entry points' docs name: called from a thread
    /// without the stack, each call starts a thread of its own; a whole
    /// batch inside one `on_parse_stack` starts one thread, and every call
    /// in it — the parse, the read transforms, both together — runs in
    /// place on it.
    #[test]
    fn a_batch_inside_on_parse_stack_starts_one_thread() {
        let opts = ParseOptions {
            sphinx: true,
            ..Default::default()
        };
        let config = crate::transforms::TransformConfig::default();
        let sources = ["A\n=\n\nOne.\n", "B\n=\n\nTwo.\n", "C\n=\n\nThree.\n"];

        // One call from this (unmarked) test thread: one thread started.
        let before = threads_started();
        parse_rst(sources[0], &opts);
        assert_eq!(threads_started(), before + 1, "a lone call starts a thread");

        let before = threads_started();
        let (trees, started_inside) = on_parse_stack(|| {
            let inside = threads_started();
            let trees: Vec<String> = sources
                .iter()
                .map(|source| {
                    let mut out = parse_rst_full(source, &opts);
                    crate::transforms::apply_read_transforms(
                        &mut out.doctree,
                        std::mem::take(&mut out.ids),
                        out.next_seq,
                        out.end_of_input,
                        "index",
                        &config,
                        &mut out.registry,
                    );
                    let (doctree, _) =
                        crate::transforms::parse_and_transform(source, &opts, &config);
                    assert_eq!(doctree.root.pformat(), out.doctree.root.pformat());
                    parse_rst(source, &opts).root.pformat()
                })
                .collect();
            (trees, threads_started() - inside)
        });
        assert_eq!(threads_started(), before + 1, "the batch starts one thread");
        assert_eq!(started_inside, 0, "every call in the batch runs in place");
        assert_eq!(trees.len(), sources.len());
    }

    /// When the system refuses a thread that size, the work runs in place
    /// rather than failing: a 2^60-byte stack cannot be mapped anywhere.
    #[test]
    fn a_refused_stack_runs_the_work_in_place() {
        let here = std::thread::current().id();
        assert_eq!(on_new_stack(1 << 60, || std::thread::current().id()), here);
    }

    /// A refused parse stack is not silent: the first refusal in a process
    /// warns that deep nesting can now overflow the stack, and why; every
    /// later one stays quiet (one warning per process, not one per parse).
    /// Driven through its own flag, since the process-wide one is shared
    /// with every other test that refuses a stack.
    #[test]
    fn a_refused_stack_warns_once() {
        let reported = std::sync::atomic::AtomicBool::new(false);
        let refused = std::io::Error::other("refused");
        let ((), logged) = capture_warnings(|| {
            report_refused_stack(&reported, 1 << 60, &refused);
            report_refused_stack(&reported, 1 << 60, &refused);
        });
        assert_eq!(logged.len(), 1, "{logged:?}");
        assert!(
            logged[0].contains("refused")
                && logged[0].contains("nest")
                && logged[0].contains("overflow"),
            "names the cause and the risk: {logged:?}"
        );
    }

    /// The work runs exactly once, its result comes back, and a panic in it
    /// reaches the caller with its own payload.
    #[test]
    fn on_parse_stack_returns_the_result_and_resumes_a_panic() {
        assert_eq!(on_parse_stack(|| 6 * 7), 42);
        let panic = std::panic::catch_unwind(|| on_parse_stack(|| panic!("inside")))
            .expect_err("the panic comes back");
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"inside"));
    }

    /// The complete current [`RegistryExport`] shape, with one record of
    /// every kind, so that a guard below can remove exactly ONE field and
    /// know the decode failed for no other reason.
    const COMPLETE_REGISTRY: &str = r#"{"nameids":[],"index_serial":0,
        "program_options":[{"source":0,"program":null,"name":"-f","node_id":"a"}],
        "std_objects":[{"source":0,"objtype":"envvar","name":"P","node_id":"b","line":1,
            "seq":1}],
        "glossary_terms":[{"term":"t","node_id":"term-t","source":0,"line":3,"seq":3}],
        "py_objects":[{"fullname":"m.f","objtype":"function","node_id":"m.f",
            "aliased":false,"source":0,"lineno":1,"seq":2}],
        "py_modules":[{"name":"m","node_id":"module-m","synopsis":"","platform":"",
            "deprecated":false,"source":0,"lineno":1}],
        "diagnostics":[{"seq":0,"channel":"Logger","level":2,"category":null,"text":"m",
            "source":0,"line":2,"doc2path_location":false}],
        "dependencies":["part.rst"],"included":["part"],
        "citations":[{"label":"CIT","node_id":"cit","source":0,"line":4,"seq":5}],
        "metadata":{"orphan":{"Str":""}}}"#;

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
            // A registry from before glossary terms were recorded at parse
            // time must MISS: a defaulted empty list would register no
            // term of a re-read document.
            "glossary_terms",
            // A pre-wave-5 registry (logger warnings in `log_warnings`, no
            // diagnostics stream) must MISS: a defaulted empty stream would
            // make a document-cache hit print none of the parse's
            // diagnostics.
            "diagnostics",
            // A wave-4.5 pre-include registry (no dependencies/included
            // stream) must MISS: a defaulted empty list would never
            // re-read the document when an included file changes, and
            // would silently un-suppress the orphan warning.
            "dependencies",
            "included",
            // A registry from before the read pass recorded its citation
            // registrations must MISS: a defaulted empty list would
            // register none of the document's citations — no duplicate
            // warning, and its citations missing from the environment.
            "citations",
            // A registry from before the read pass collected the docinfo's
            // metadata must MISS: a defaulted empty map would lose the
            // document's `:orphan:` (a false orphan warning) and
            // `:tocdepth:`.
            "metadata",
        ] {
            must_miss(&[], field);
        }
    }

    /// Each field of a [`CitationRecord`] follows the same rule: a record
    /// decoding with a zeroed `seq` would print its duplicate warning first
    /// of the document's stream, a zeroed `line` at line 0.
    #[test]
    fn citation_records_missing_a_field_fail_to_decode() {
        for field in ["label", "node_id", "source", "line", "seq"] {
            must_miss(&["citations", "0"], field);
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
        must_miss(&["glossary_terms", "0"], "source");
    }

    /// The fields of a [`diagnostics::Diagnostic`] record, and the `seq`
    /// the warning-producing registrations carry, follow the same rule: a
    /// stale record must MISS rather than decode with a zeroed position
    /// (every replayed duplicate warning would sort first) or a defaulted
    /// flag (`doc2path_location` off would render the literalinclude
    /// reader warnings at the un-doubled path). The two `Option` fields
    /// (`category`, `line`) are serde's exception — a missing `Option`
    /// decodes as `None` without any attribute — and need none: they were
    /// in the record from its first version, and a registry from before
    /// the record existed misses on `diagnostics` itself.
    #[test]
    fn records_written_before_the_diagnostics_stream_existed_fail_to_decode() {
        for field in [
            "seq",
            "channel",
            "level",
            "text",
            "source",
            "doc2path_location",
        ] {
            must_miss(&["diagnostics", "0"], field);
        }
        must_miss(&["std_objects", "0"], "seq");
        must_miss(&["py_objects", "0"], "seq");
        for field in ["term", "node_id", "line", "seq"] {
            must_miss(&["glossary_terms", "0"], field);
        }
    }

    /// The provenance fields panel fix round B added to the DOCUMENT-side
    /// records (`DirectiveRecord`, `RoleRecord`, `ToctreeRecord`) follow
    /// the same rule. These ride the
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
        must_miss::<ToctreeRecord>(
            r#"{"glob":false,"entries":[],"source":1,"line":3}"#,
            r#"{"glob":false,"entries":[],"line":3}"#,
        );
    }
}
