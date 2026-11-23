//! Reading one document: the parse, then the read transforms (wave 5a).
//!
//! Sphinx reads a document by parsing it into a docutils `document` and
//! applying every read transform to that same object; what it pickles is
//! the post-read doctree ([DS §2.1]). The read here has the same two halves
//! ([DS §9]): stage R ([`read_document`]) parses and runs the transforms
//! that need nothing but the document, in parallel with the other reads;
//! stage M ([`merge_document`]) runs the rest, which merge the document
//! into the build environment, one document at a time in sorted docname
//! order. The transforms themselves, and the table that orders them, are
//! [`crate::transforms`].
//!
//! [`read_standalone`] reads one snippet the way the oracle harnesses do,
//! with no project around it: the doctree differentials and the read-state
//! differential compare its output with Sphinx's.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::BuildConfig;
use crate::diagnostics::{Category, Diagnostic, OrderKey, Severity};
use crate::doctree::{Doctree, Node, Uid};
use crate::env::BuildEnvironment;
use crate::rst::{LogLocation, ParseOptions, ParseOutput, RegistryExport};
use crate::transforms::reporter::{render_records, LogRecord};
use crate::transforms::state::DocumentState;
use crate::transforms::{Transform, TransformId};

/// The configuration the read consumes. Every field holds Sphinx's default
/// unless the configuration says otherwise ([`Self::from_config`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSettings {
    /// `keep_warnings` (`SP/sphinx/config.py:261`, default `False`): whether
    /// `FilterSystemMessages` (999) keeps the reporter's `system_message`s
    /// of level WARNING and up in the post-read doctree, rather than
    /// removing every one.
    pub keep_warnings: bool,
    /// Record the parse's id allocations
    /// ([`IdEvent`](crate::transforms::state::IdEvent)) and, in
    /// [`read_standalone`], whether each id's node is still in the read
    /// tree: the read-state differential's `id_trace`. Tests only; it turns
    /// the trace on whatever [`ParseOptions::trace_ids`] says.
    pub trace_ids: bool,
}

impl ReadSettings {
    /// Sphinx's defaults.
    pub fn sphinx_defaults() -> Self {
        ReadSettings {
            keep_warnings: false,
            trace_ids: false,
        }
    }

    /// The read settings of a build configured by `config`.
    pub fn from_config(config: &BuildConfig) -> Self {
        ReadSettings {
            keep_warnings: config.keep_warnings,
            ..Self::sphinx_defaults()
        }
    }
}

/// One document's read: the doctree and the docutils document state beside
/// it. Both live in the [`ParseOutput`], so there is one copy of each.
///
/// A build's read ([`crate::parser::Parser`]) moves the parse's toctree,
/// directive and role records and its registry export on to the
/// pipeline's `Document`, which the incremental cache persists, so in a
/// build `output` holds empty stand-ins for them; the build lends the
/// registry export and the toctree records back while stage M
/// ([`merge_document`]) runs, which reads them here, where a standalone
/// read ([`read_standalone`]) keeps them.
pub struct ReadOutcome {
    pub output: ParseOutput,
}

impl ReadOutcome {
    /// The outcome of a document that was not parsed: a source this crate
    /// has no reader for (Markdown, plain text), or one recovered whole
    /// from the build cache. Its state is empty, and it has no records.
    pub fn without_parse(doctree: Doctree) -> ReadOutcome {
        ReadOutcome {
            output: ParseOutput {
                doctree,
                directive_records: Vec::new(),
                role_records: Vec::new(),
                toctrees: Vec::new(),
                registry: RegistryExport::default(),
                state: DocumentState::default(),
            },
        }
    }

    pub fn doctree(&self) -> &Doctree {
        &self.output.doctree
    }

    pub fn doctree_mut(&mut self) -> &mut Doctree {
        &mut self.output.doctree
    }

    pub fn state(&self) -> &DocumentState {
        &self.output.state
    }

    pub fn state_mut(&mut self) -> &mut DocumentState {
        &mut self.output.state
    }
}

/// Stage R: parse `source`, then, for a Sphinx-mode read, run the read
/// transforms that need nothing but the document
/// ([`crate::transforms::run_read_transforms`]); `TranslationProgressTotaliser`
/// (025) stamps `translation_progress` on every such document (decision 9).
/// A docutils-mode read is the parse alone: Sphinx's transforms belong to
/// a Sphinx read.
///
/// The document's log ([`DocumentState::log`]) then holds every record of
/// the parse, the reporter's and the logger's ([`log_parse_warnings`]),
/// followed by the stage's own.
pub fn read_document(source: &str, parse: &ParseOptions, settings: &ReadSettings) -> ReadOutcome {
    read_document_with(source, parse, settings, &TransformId::transform)
}

/// [`read_document`], with each row of the read-transform table run by the
/// transform `transform` gives it: the read's own
/// ([`TransformId::transform`]), or, in a test, one swapped in.
fn read_document_with(
    source: &str,
    parse: &ParseOptions,
    settings: &ReadSettings,
    transform: &dyn Fn(TransformId) -> &'static dyn Transform,
) -> ReadOutcome {
    let mut output = if settings.trace_ids && !parse.trace_ids {
        crate::rst::parse_rst_full(
            source,
            &ParseOptions {
                trace_ids: true,
                ..parse.clone()
            },
        )
    } else {
        crate::rst::parse_rst_full(source, parse)
    };
    log_parse_warnings(&mut output, parse);
    if parse.sphinx {
        crate::transforms::read_transforms_with(output, settings, &parse.docname, transform)
    } else {
        ReadOutcome { output }
    }
}

/// Adds the parse's logger records to the document's log, each keyed
/// `(0, 0, seq)` with the seq it drew when it was logged, so the log holds
/// the reporter's and the logger's records in the order the parse created
/// them ([TW §5.5] rule 1; [DS §6.4]): every `ParseLogWarning`, moved out
/// of the export and located by its [`LogLocation`] rule, and every
/// toctree warning, copied (the merge still reads the toctree records'
/// warnings) and located as `TocTree.parse_content` logs it,
/// `location=toctree`: the `Node` rule, with its category.
///
/// Locations render against the project's source directory, or, for a
/// read with no project, the one the document's path implies
/// ([`source_dir_of`]).
fn log_parse_warnings(output: &mut ParseOutput, parse: &ParseOptions) {
    let srcdir = source_dir_of(parse);
    let sources = &output.doctree.sources;
    let key = |seq| OrderKey {
        priority: 0,
        serial: 0,
        seq,
    };
    let logged = std::mem::take(&mut output.registry.log_warnings)
        .into_iter()
        .map(|warning| LogRecord::Logger {
            key: key(warning.seq),
            diagnostic: warning.diagnostic(sources, &srcdir, &parse.docname),
        });
    let toctree = output
        .toctrees
        .iter()
        .flat_map(|toctree| &toctree.warnings)
        .map(|warning| LogRecord::Logger {
            key: key(warning.seq),
            diagnostic: Diagnostic {
                location: LogLocation::Node {
                    source: warning.source,
                    line: warning.line,
                }
                .render(sources, &srcdir, &parse.docname),
                severity: Severity::Warning,
                message: warning.message.clone(),
                category: warning
                    .category
                    .as_deref()
                    .map_or(Category::None, Category::parse),
            },
        });
    let log = &mut output.state.log;
    log.extend(logged.chain(toctree));
    // Every parse record drew its own seq: the stable sort restores the
    // creation order the reporter's records were logged in.
    log.sort_by_key(LogRecord::key);
}

/// The source directory a read's logger locations render against: the
/// project's, or, for a read with none (a standalone snippet), the one
/// the document's path implies, Sphinx's `doc2path(docname)` read
/// backwards — the path less one level per docname component
/// (`<SRCDIR>/index.rst` → `<SRCDIR>`).
fn source_dir_of(parse: &ParseOptions) -> PathBuf {
    if let Some(srcdir) = &parse.srcdir {
        return srcdir.clone();
    }
    let mut dir = Path::new(&parse.source_path);
    for _ in parse.docname.split('/') {
        dir = dir.parent().unwrap_or(Path::new(""));
    }
    dir.to_path_buf()
}

/// What stage M needs to know about a document besides its read: where
/// its source is, the project's source directory, how a docname maps to
/// its source (Sphinx's `env.doc2path`) and when the document was read.
pub struct MergeInputs<'a> {
    /// The document's source file: docutils' `reporter.source`, where a
    /// record with no source of its own is located.
    pub source_path: &'a Path,
    /// The project's source directory (`env.srcdir`).
    pub source_dir: &'a Path,
    /// `env.doc2path`: the source path of a docname.
    pub doc2path: &'a dyn Fn(&str) -> PathBuf,
    /// The read's completion time, in microseconds since the epoch
    /// (`env.all_docs[docname]`).
    pub read_time_us: u64,
}

/// Stage M for one document: merges its read into `env`
/// ([`crate::transforms::run_merge_transforms`]) and returns the
/// document's records in the order Sphinx prints them: its log (stage R's
/// records) and stage M's, rendered by [`render_records`] (a record with no
/// source of its own is located at [`MergeInputs::source_path`]),
/// stable-sorted by key, keys dropped. The log itself is left in place.
pub fn merge_document(
    env: &mut BuildEnvironment,
    docname: &str,
    outcome: &mut ReadOutcome,
    inputs: &MergeInputs<'_>,
    settings: &ReadSettings,
) -> Vec<Diagnostic> {
    let merged = crate::transforms::run_merge_transforms(outcome, env, docname, inputs, settings);
    let mut records = outcome.state().log.clone();
    records.extend(merged);
    in_key_order(records, &outcome.doctree().sources, inputs.source_path)
}

/// A document read on its own, as the oracle harnesses read a snippet.
pub struct StandaloneRead {
    pub outcome: ReadOutcome,
    /// The environment stage M merged the document into, fresh for this
    /// read.
    pub env: BuildEnvironment,
    /// The read's warning-stream records, in the order Sphinx prints them.
    pub diagnostics: Vec<Diagnostic>,
}

/// Stage R and a one-document stage M ([`merge_document`]) against a fresh
/// environment, which it returns, with the records stage M returns.
/// Stage M's inputs come from `parse`: the source path; the project's
/// source directory or, for a read with none, the one the source path
/// implies ([`source_dir_of`]), whose `<docname>.rst` is a docname's
/// source; a read time of 0; and `parse.docname`.
///
/// With the id trace on ([`ReadSettings::trace_ids`] or
/// [`ParseOptions::trace_ids`]), every trace event's `attached` then says
/// whether the final tree still holds the node the event's id names
/// ([`fill_attached`]).
///
/// A docutils-mode read is the parse alone ([`read_document`]): it has no
/// stage M, so its environment stays fresh, its records are its log,
/// rendered and ordered the same way, and its trace events stay
/// unattached.
pub fn read_standalone(
    source: &str,
    parse: &ParseOptions,
    settings: &ReadSettings,
) -> StandaloneRead {
    read_standalone_with(source, parse, settings, &TransformId::transform)
}

/// [`read_standalone`], with stage R's transforms given as
/// [`read_document_with`] takes them.
fn read_standalone_with(
    source: &str,
    parse: &ParseOptions,
    settings: &ReadSettings,
    transform: &dyn Fn(TransformId) -> &'static dyn Transform,
) -> StandaloneRead {
    let mut outcome = read_document_with(source, parse, settings, transform);
    let mut env = BuildEnvironment::default();
    let source_path = Path::new(&parse.source_path);
    if !parse.sphinx {
        let log = outcome.state().log.clone();
        let diagnostics = in_key_order(log, &outcome.doctree().sources, source_path);
        return StandaloneRead {
            outcome,
            env,
            diagnostics,
        };
    }
    let source_dir = source_dir_of(parse);
    let doc2path = |docname: &str| source_dir.join(format!("{docname}.rst"));
    let inputs = MergeInputs {
        source_path,
        source_dir: &source_dir,
        doc2path: &doc2path,
        read_time_us: 0,
    };
    let diagnostics = merge_document(&mut env, &parse.docname, &mut outcome, &inputs, settings);
    if !outcome.state().ids.trace().is_empty() {
        fill_attached(&mut outcome.output);
    }
    StandaloneRead {
        outcome,
        env,
        diagnostics,
    }
}

/// `records` rendered by [`render_records`], stable-sorted by key, keys
/// dropped: the order Sphinx prints one document's records in ([DS §6.4]).
fn in_key_order(records: Vec<LogRecord>, sources: &[String], doc_path: &Path) -> Vec<Diagnostic> {
    let mut keyed = render_records(records, sources, &doc_path.display().to_string());
    keyed.sort_by_key(|(key, _)| *key);
    keyed
        .into_iter()
        .map(|(_, diagnostic)| diagnostic)
        .collect()
}

/// Sets each id-trace event's `attached` to whether the final tree still
/// holds the node the event's id named: stage R tagged the event with that
/// node's identity when the node was in the tree at the end of the stage
/// ([`crate::transforms::state::IdAllocator::tag_event`]), and stage M can
/// only have taken it out since. An untagged event's node was detached or
/// nowhere when stage R ended (T5a's rule), so the event is not attached.
fn fill_attached(output: &mut ParseOutput) {
    fn held(node: &Node, uids: &mut HashSet<Uid>) {
        uids.extend(node.uid());
        for child in &node.children {
            held(child, uids);
        }
    }
    let ParseOutput { doctree, state, .. } = output;
    let mut uids = HashSet::new();
    held(&doctree.root, &mut uids);
    let tags: Vec<Option<Uid>> = (0..state.ids.trace().len())
        .map(|event| state.ids.event_tag(event))
        .collect();
    for (event, tag) in state.ids.trace_mut().iter_mut().zip(tags) {
        event.attached = tag.is_some_and(|uid| uids.contains(&uid));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Location;
    use crate::doctree::kinds;

    fn sphinx_options() -> ParseOptions {
        ParseOptions {
            source_path: "<SRCDIR>/index.rst".to_string(),
            sphinx: true,
            ..ParseOptions::default()
        }
    }

    /// The read settings follow the configuration, Sphinx's defaults
    /// otherwise; the trace is a test switch no configuration sets.
    #[test]
    fn read_settings_follow_the_configuration() {
        assert_eq!(
            ReadSettings::sphinx_defaults(),
            ReadSettings {
                keep_warnings: false,
                trace_ids: false,
            }
        );
        assert_eq!(
            ReadSettings::from_config(&BuildConfig::default()),
            ReadSettings::sphinx_defaults()
        );
        let config = BuildConfig {
            keep_warnings: true,
            ..BuildConfig::default()
        };
        assert!(ReadSettings::from_config(&config).keep_warnings);
        assert!(!ReadSettings::from_config(&config).trace_ids);
    }

    /// A Sphinx-mode read stamps `translation_progress` on the document,
    /// beside its source; a docutils-mode parse does not.
    #[test]
    fn read_document_stamps_translation_progress() {
        let read = read_document(
            "Text.\n",
            &sphinx_options(),
            &ReadSettings::sphinx_defaults(),
        );
        assert_eq!(
            read.doctree().root.pformat(),
            "<document source=\"<SRCDIR>/index.rst\" \
             translation_progress=\"{'total': 0, 'translated': 0}\">\n    \
             <paragraph>\n        Text.\n"
        );
        let docutils = ParseOptions {
            sphinx: false,
            ..sphinx_options()
        };
        let read = read_document("Text.\n", &docutils, &ReadSettings::sphinx_defaults());
        assert_eq!(read.doctree().root.get("translation_progress"), None);
    }

    /// The read's records are the document's log, rendered as Sphinx's
    /// warning stream prints it (probed on Sphinx 9.1.0: the INFO about the
    /// too-short underline of `Short` does not print) and in key order;
    /// stage M merged the document into the read's environment.
    #[test]
    fn read_standalone_renders_the_log_in_key_order() {
        let read = read_standalone(
            "Short\n===\n\nTitle\n====\n\nPara\nmore\n   indented.\n",
            &sphinx_options(),
            &ReadSettings::sphinx_defaults(),
        );
        let rendered: Vec<String> = read
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render(true))
            .collect();
        assert_eq!(
            rendered,
            [
                "<SRCDIR>/index.rst:5: WARNING: Title underline too short.\n\nTitle\n==== \
                 [docutils]",
                "<SRCDIR>/index.rst:9: ERROR: Unexpected indentation. [docutils]",
            ]
        );
        assert!(read
            .diagnostics
            .iter()
            .all(|diagnostic| matches!(diagnostic.location, Location::Verbatim(_))));
        assert_eq!(
            read.env.all_docs,
            std::collections::BTreeMap::from([("index".to_string(), 0)])
        );
        assert_eq!(read.env.titles["index"].astext(), "Title");
    }

    /// The read logs the parse's logger records beside the reporter's, in
    /// the order the parse created them, each located by its rule against
    /// the source directory the snippet's path implies: probed on Sphinx
    /// 9.1.0, the malformed option (`location=signode`), then the reporter's
    /// WARNING, the unknown role's bare record and its ERROR, then the
    /// `:emphasize-lines:` tuple location `doc2path` doubles. The export
    /// keeps none of them.
    #[test]
    fn read_logs_the_parse_logger_records_in_creation_order() {
        let read = read_standalone(
            "Index\n=====\n\n.. option:: =bad\n\nSee *this.\n\n:a:b:`x`\n\n\
             .. code-block:: python\n   :emphasize-lines: 9\n\n   pass\n",
            &sphinx_options(),
            &ReadSettings::sphinx_defaults(),
        );
        let rendered: Vec<String> = read
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render(true))
            .collect();
        assert_eq!(
            rendered,
            [
                "<SRCDIR>/index.rst:4: WARNING: Malformed option description '=bad', should \
                 look like \"opt\", \"-opt args\", \"--opt args\", \"/opt args\" or \"+opt args\"",
                "<SRCDIR>/index.rst:6: WARNING: Inline emphasis start-string without \
                 end-string. [docutils]",
                "WARNING: unknown role name: a:b",
                "<SRCDIR>/index.rst:8: ERROR: Unknown interpreted text role \"a:b\". [docutils]",
                "<SRCDIR>/index.rst.rst:10: WARNING: line number spec is out of range(1-1): '9'",
            ]
        );
        let keys: Vec<OrderKey> = read
            .outcome
            .state()
            .log
            .iter()
            .map(LogRecord::key)
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "the log is in creation order");
        assert!(read.outcome.output.registry.log_warnings.is_empty());
    }

    /// With the trace on, a parse event is attached when the read tree
    /// carries its id, and not when only a discarded subtree does: the
    /// footnote reference inside the rejected substitution definition
    /// ([DS §4.3]) keeps its id in the detached forest only.
    #[test]
    fn read_standalone_fills_the_trace_attachment() {
        let traced = ReadSettings {
            trace_ids: true,
            ..ReadSettings::sphinx_defaults()
        };
        let read = read_standalone(
            ".. |f| replace:: note [#]_\n\nText [#]_.\n\n.. [#] the footnote\n",
            &sphinx_options(),
            &traced,
        );
        let trace: Vec<(&str, &str, &str, bool)> = read
            .outcome
            .state()
            .ids
            .trace()
            .iter()
            .map(|event| {
                (
                    event.phase.as_str(),
                    event.kind,
                    event.id.as_str(),
                    event.attached,
                )
            })
            .collect();
        assert_eq!(
            trace,
            [
                ("parse", kinds::FOOTNOTE_REFERENCE, "id1", false),
                ("parse", kinds::FOOTNOTE_REFERENCE, "id2", true),
                ("parse", kinds::FOOTNOTE, "id3", true),
            ]
        );

        // The settings' switch traces whatever the parse options say; with
        // both off there is no trace.
        let untraced = read_standalone(
            "[#]_\n\n.. [#] n\n",
            &sphinx_options(),
            &ReadSettings::sphinx_defaults(),
        );
        assert!(untraced.outcome.state().ids.trace().is_empty());
    }

    /// An unparsed document has the doctree it was given, an empty state
    /// and no records.
    #[test]
    fn an_unparsed_outcome_has_an_empty_state() {
        let doctree = Doctree {
            root: Node::elem(kinds::DOCUMENT, crate::doctree::Span::ZERO),
            sources: vec!["<document>".to_string()],
        };
        let outcome = ReadOutcome::without_parse(doctree.clone());
        assert_eq!(outcome.doctree(), &doctree);
        assert!(outcome.state().log.is_empty());
        assert_eq!(outcome.state().ids.trace().len(), 0);
        assert!(outcome.output.toctrees.is_empty());
    }

    /// Stage M's inputs for the document at `source_path`, in a project
    /// at `<SRCDIR>`, read at time 0.
    fn merge_inputs<'a>(
        source_path: &'a Path,
        doc2path: &'a dyn Fn(&str) -> PathBuf,
    ) -> MergeInputs<'a> {
        MergeInputs {
            source_path,
            source_dir: Path::new("<SRCDIR>"),
            doc2path,
            read_time_us: 0,
        }
    }

    /// Stage M's records carry the key of the slot that raises them —
    /// `SphinxDomains`' `(850, 40)` for the index domain's and the std
    /// domain's label records — with seqs in the order they were raised,
    /// so they sort after the document's stage-R records, the parse's
    /// `(0, 0, seq)` ([DS §6.4]). Probed on Sphinx 9.1.0 (`a`, then `b`, in
    /// one project): `b` prints its parse WARNING, then the index domain's
    /// record, then the std domain's duplicate label, which is what
    /// [`merge_document`] returns.
    #[test]
    fn merge_records_carry_stage_m_keys() {
        let settings = ReadSettings::sphinx_defaults();
        let doc2path = |docname: &str| PathBuf::from(format!("<SRCDIR>/{docname}.rst"));
        let options = |docname: &str| ParseOptions {
            source_path: format!("<SRCDIR>/{docname}.rst"),
            docname: docname.to_string(),
            ..sphinx_options()
        };
        let mut env = BuildEnvironment::default();
        let mut a = read_document(
            ".. _dup:\n\nSection A\n=========\n",
            &options("a"),
            &settings,
        );
        let a_path = PathBuf::from("<SRCDIR>/a.rst");
        merge_document(
            &mut env,
            "a",
            &mut a,
            &merge_inputs(&a_path, &doc2path),
            &settings,
        );

        let b_source =
            "Title\n====\n\n.. _dup:\n\nSection B\n---------\n\n.. index:: pair: lonely\n";
        let b_path = PathBuf::from("<SRCDIR>/b.rst");
        let mut b = read_document(b_source, &options("b"), &settings);
        let stage_r: Vec<OrderKey> = b.state().log.iter().map(LogRecord::key).collect();
        assert_eq!(stage_r.len(), 1, "the short underline's WARNING");
        let mut merged_env = env.clone();
        let records = crate::transforms::run_merge_transforms(
            &mut b,
            &mut merged_env,
            "b",
            &merge_inputs(&b_path, &doc2path),
            &settings,
        );
        let keys: Vec<OrderKey> = records.iter().map(LogRecord::key).collect();
        let slots: Vec<(u16, u16)> = keys.iter().map(|key| (key.priority, key.serial)).collect();
        assert_eq!(slots, [(850, 40), (850, 40)]);
        assert!(
            keys[0] < keys[1],
            "the index domain's record was raised first"
        );
        assert!(stage_r
            .iter()
            .all(|parse| keys.iter().all(|merge| parse < merge)));

        let mut b = read_document(b_source, &options("b"), &settings);
        let rendered: Vec<String> = merge_document(
            &mut env,
            "b",
            &mut b,
            &merge_inputs(&b_path, &doc2path),
            &settings,
        )
        .iter()
        .map(|diagnostic| diagnostic.render(true))
        .collect();
        assert_eq!(
            rendered,
            [
                "<SRCDIR>/b.rst:2: WARNING: Title underline too short.\n\nTitle\n==== \
                 [docutils]",
                "<SRCDIR>/b.rst:9: WARNING: invalid pair index entry 'lonely' [index]",
                "<SRCDIR>/b.rst:7: WARNING: duplicate label dup, other instance in \
                 <SRCDIR>/a.rst",
            ]
        );
    }

    /// A document's records print in key order, not in the order they
    /// were raised: stage M raises the collectors' records `(880, 30)`
    /// before the domains' `(850, 40)` until T13a runs them in Sphinx's
    /// order, and both sort after the parse's `(0, 0)` whatever their
    /// seqs. Records with equal keys keep their order.
    #[test]
    fn records_print_in_key_order() {
        let logged = |priority, serial, seq, message: &str| LogRecord::Logger {
            key: OrderKey {
                priority,
                serial,
                seq,
            },
            diagnostic: Diagnostic {
                location: crate::diagnostics::Location::None,
                severity: Severity::Warning,
                message: message.to_string(),
                category: Category::None,
            },
        };
        let raised = vec![
            logged(880, 30, 3, "collector"),
            logged(0, 0, 5, "parse, later"),
            logged(850, 40, 4, "domain"),
            logged(0, 0, 1, "parse"),
            logged(850, 40, 4, "domain, same key"),
        ];
        let printed: Vec<String> = in_key_order(raised, &[], Path::new("<SRCDIR>/index.rst"))
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect();
        assert_eq!(
            printed,
            [
                "parse",
                "parse, later",
                "domain",
                "domain, same key",
                "collector"
            ]
        );
    }

    /// A transform that allocates an id for a message it never inserts,
    /// as a transform reporting a loose message does.
    struct AllocatesForALooseMessage;

    impl Transform for AllocatesForALooseMessage {
        fn apply(&self, cx: &mut crate::transforms::TransformCx<'_>) {
            let message = cx.tree.new_node(kinds::SYSTEM_MESSAGE);
            let id = cx.state.ids.allocate(kinds::SYSTEM_MESSAGE, &[], None);
            cx.tree.node_mut(message).attrs.ids.push(id.clone());
            cx.tree.note_id(&id, message);
        }
    }

    /// The id trace names the transform each id was allocated in, by its
    /// `python_name()`, and an event is attached only when the final tree,
    /// after stage M's `FilterSystemMessages` (999), still holds its node.
    /// A test transform in the `ExposeInternals` row allocates an id for a
    /// message it never inserts: its event is that row's and unattached.
    /// For the parse's events of `Title\n=====\n\n*x\n`, probed on Sphinx
    /// 9.1.0 with the read-state fixture's instrumentation: the section
    /// and the `problematic` stay attached, and the inline-markup message
    /// is attached only under `keep_warnings = True`, because under
    /// `False` 999 removes it.
    #[test]
    fn id_trace_names_transform_phases() {
        let transform = |id: TransformId| -> &'static dyn Transform {
            match id {
                TransformId::ExposeInternals => &AllocatesForALooseMessage,
                _ => id.transform(),
            }
        };
        let trace = |keep_warnings: bool| -> Vec<(String, &'static str, String, bool)> {
            let settings = ReadSettings {
                keep_warnings,
                trace_ids: true,
            };
            let read = read_standalone_with(
                "Title\n=====\n\n*x\n",
                &sphinx_options(),
                &settings,
                &transform,
            );
            read.outcome
                .state()
                .ids
                .trace()
                .iter()
                .map(|event| {
                    (
                        event.phase.clone(),
                        event.kind,
                        event.id.clone(),
                        event.attached,
                    )
                })
                .collect()
        };
        let event = |phase: &str, kind, id: &str, attached| {
            (phase.to_string(), kind, id.to_string(), attached)
        };
        let loose = event(
            "docutils.transforms.universal.ExposeInternals",
            kinds::SYSTEM_MESSAGE,
            "id3",
            false,
        );
        assert_eq!(
            trace(false),
            [
                event("parse", kinds::SECTION, "title", true),
                event("parse", kinds::SYSTEM_MESSAGE, "id1", false),
                event("parse", kinds::PROBLEMATIC, "id2", true),
                loose.clone(),
            ]
        );
        assert_eq!(
            trace(true),
            [
                event("parse", kinds::SECTION, "title", true),
                event("parse", kinds::SYSTEM_MESSAGE, "id1", true),
                event("parse", kinds::PROBLEMATIC, "id2", true),
                loose,
            ]
        );
    }
}
