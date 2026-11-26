//! What the transforms' unit tests share: a snippet read the way the
//! read-state fixture reads one (`tools/gen_read_state_fixture.py`), and the
//! spellings Sphinx's reads are compared in.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::builder::BuilderKind;
use crate::config::BuildConfig;
use crate::diagnostics::Diagnostic;
use crate::doctree::Doctree;
use crate::env::resolve::{resolve_document, NitpickConfig, Resolver};
use crate::env::BuildEnvironment;
use crate::intersphinx::Intersphinx;
use crate::read::{merge_document, read_document, read_standalone, MergeInputs, ReadSettings};
use crate::rst::ParseOptions;
use crate::transforms::reporter::LogRecord;

/// The document line every post-read pformat of `<SRCDIR>/index.rst` opens
/// with.
pub(crate) const DOC: &str = "<document source=\"<SRCDIR>/index.rst\" \
                              translation_progress=\"{'total': 0, 'translated': 0}\">\n";

/// One id-trace event, `[phase, kind, id, attached]`, the phase's Python
/// name shortened to its class ([`read_snippet`]).
pub(crate) type Event = (String, &'static str, String, bool);

/// One read of a snippet as the read-state harness reads it.
pub(crate) struct Read {
    /// The post-read pformat, less [`DOC`].
    pub(crate) body: String,
    /// The warning stream, one entry per line.
    pub(crate) warnings: Vec<String>,
    /// The id trace.
    pub(crate) trace: Vec<Event>,
    /// The reporter's records of every level, `(level, text)`.
    pub(crate) reported: Vec<(u8, String)>,
}

/// How Sphinx parses `<SRCDIR>/index.rst`.
pub(crate) fn index_rst() -> ParseOptions {
    ParseOptions {
        source_path: "<SRCDIR>/index.rst".to_string(),
        sphinx: true,
        docname: "index".to_string(),
        ..ParseOptions::default()
    }
}

/// The read-state fixture's read settings (M1's validation off, smart
/// quotes off, the dummy builder, the id trace on), with `keep_warnings`
/// as given.
pub(crate) fn harness_settings(keep_warnings: bool) -> ReadSettings {
    ReadSettings {
        trace_ids: true,
        ..ReadSettings::from_config(
            &BuildConfig {
                validate_directives: false,
                keep_warnings,
                smartquotes: false,
                ..BuildConfig::default()
            },
            BuilderKind::Dummy,
        )
    }
}

/// Our read of `rst` as `<SRCDIR>/index.rst`, the way the read-state
/// fixture reads a snippet ([`harness_settings`]), with `keep_warnings` as
/// given.
pub(crate) fn read_snippet(rst: &str, keep_warnings: bool) -> Read {
    read_snippet_with(rst, &harness_settings(keep_warnings))
}

/// [`read_snippet`] under `settings`.
pub(crate) fn read_snippet_with(rst: &str, settings: &ReadSettings) -> Read {
    let read = read_standalone(rst, &index_rst(), settings);
    let pformat = read.outcome.doctree().root.pformat();
    let body = pformat
        .strip_prefix(DOC)
        .unwrap_or_else(|| panic!("{rst:?}: unexpected document line:\n{pformat}"))
        .to_string();
    let warnings = read
        .diagnostics
        .iter()
        .flat_map(|diagnostic| {
            diagnostic
                .render(true)
                .split('\n')
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    let state = read.outcome.state();
    let trace = state
        .ids
        .trace()
        .iter()
        .map(|event| {
            let phase = event.phase.rsplit('.').next().unwrap_or_default();
            (
                phase.to_string(),
                event.kind,
                event.id.clone(),
                event.attached,
            )
        })
        .collect();
    let reported = state
        .log
        .iter()
        .filter_map(|record| match record {
            LogRecord::Reporter { level, text, .. } => Some((*level, text.clone())),
            LogRecord::Logger { .. } => None,
        })
        .collect();
    Read {
        body,
        warnings,
        trace,
        reported,
    }
}

/// A project read the way the build reads one: each document of
/// [`merge_project`], in the order given.
pub(crate) struct Project {
    /// The environment every document merged into.
    pub(crate) env: BuildEnvironment,
    /// Each document's read-phase records, rendered, in merge order.
    pub(crate) records: Vec<String>,
    /// Each document's post-read doctree, by docname, in merge order.
    pub(crate) doctrees: Vec<(String, Doctree)>,
}

/// `documents`, `(docname, source)` pairs, each read as
/// `<SRCDIR>/<docname>.rst` (stage R) and merged into one environment
/// (stage M) in the order given, as the build merges a project's documents
/// in sorted docname order. The project's documents are the parse's
/// `env.found_docs`, against which a toctree resolves its entries.
pub(crate) fn merge_project(documents: &[(&str, &str)]) -> Project {
    merge_project_in(documents, None)
}

/// [`merge_project`] with `files`, `(path, text)` pairs, written into a
/// temporary source directory, the parse's `srcdir`, against which an
/// `include` in a document resolves; the documents keep their
/// `<SRCDIR>/<docname>.rst` paths.
pub(crate) fn merge_project_beside(documents: &[(&str, &str)], files: &[(&str, &str)]) -> Project {
    let srcdir = tempfile::tempdir().expect("a temporary source directory");
    for (path, text) in files {
        std::fs::write(srcdir.path().join(path), text).expect("a file beside the documents");
    }
    merge_project_in(documents, Some(srcdir.path()))
}

/// [`read_standalone`] of `rst` as `index.rst` in a temporary source
/// directory that holds `files`, `(path, text)` pairs, beside it (the
/// parse's `srcdir`, against which an `include` resolves), read the way
/// [`read_snippet`] reads one with `keep_warnings` off: the read's records,
/// rendered with that directory spelled `<SRCDIR>/`.
pub(crate) fn read_beside(rst: &str, files: &[(&str, &str)]) -> Vec<String> {
    let srcdir = tempfile::tempdir().expect("a temporary source directory");
    for (path, text) in files {
        std::fs::write(srcdir.path().join(path), text).expect("a file beside the document");
    }
    let parse = ParseOptions {
        source_path: srcdir.path().join("index.rst").display().to_string(),
        srcdir: Some(srcdir.path().to_path_buf()),
        ..index_rst()
    };
    let read = read_standalone(rst, &parse, &harness_settings(false));
    let root = format!("{}{}", srcdir.path().display(), std::path::MAIN_SEPARATOR);
    read.diagnostics
        .iter()
        .map(|record| record.render(true).replace(&root, "<SRCDIR>/"))
        .collect()
}

/// [`merge_project`] with `srcdir` as the parse's source directory, if any.
fn merge_project_in(documents: &[(&str, &str)], srcdir: Option<&Path>) -> Project {
    let settings = ReadSettings::sphinx_defaults();
    let doc2path = |docname: &str| PathBuf::from(format!("<SRCDIR>/{docname}.rst"));
    let found_docs: BTreeSet<String> = documents
        .iter()
        .map(|(docname, _)| docname.to_string())
        .collect();
    let found_docs = Arc::new(found_docs);
    let mut project = Project {
        env: BuildEnvironment::default(),
        records: Vec::new(),
        doctrees: Vec::new(),
    };
    for (docname, source) in documents {
        let path = doc2path(docname);
        let parse = ParseOptions {
            source_path: path.display().to_string(),
            docname: docname.to_string(),
            found_docs: Some(Arc::clone(&found_docs)),
            srcdir: srcdir.map(Path::to_path_buf),
            ..index_rst()
        };
        let mut read = read_document(source, &parse, &settings);
        let inputs = MergeInputs {
            source_path: &path,
            source_dir: Path::new("<SRCDIR>"),
            doc2path: &doc2path,
            read_time_us: 0,
        };
        let records = merge_document(&mut project.env, docname, &mut read, &inputs, &settings);
        project
            .records
            .extend(records.iter().map(|diagnostic| diagnostic.render(true)));
        project
            .doctrees
            .push((docname.to_string(), read.output.doctree));
    }
    project
}

/// The write phase's view of one document of a [`Project`]
/// ([`resolved`]).
pub(crate) struct Resolved {
    /// The resolved doctree's pformat, less its document line.
    pub(crate) body: String,
    /// The records the resolution logged, rendered.
    pub(crate) warnings: Vec<String>,
    /// The references into a domain the build does not resolve.
    pub(crate) unresolvable: usize,
}

/// `docname` of `project` with its references resolved against the
/// project's environment the way the build's `ReferencesResolver` (10)
/// resolves them, the later post-transforms left out: the dummy builder's
/// `''` URIs, `numfig` off, no intersphinx, not nitpicky.
pub(crate) fn resolved(project: &Project, docname: &str) -> Resolved {
    resolved_with(
        project,
        docname,
        crate::env::math::MathConfig::default(),
        &Intersphinx::default(),
    )
}

/// [`resolved`] under the math domain's configuration `math`, with the
/// loaded intersphinx state `intersphinx` (its `intersphinx_resolve_self`,
/// for one).
pub(crate) fn resolved_with(
    project: &Project,
    docname: &str,
    math: crate::env::math::MathConfig,
    intersphinx: &Intersphinx,
) -> Resolved {
    let formats = BTreeMap::new();
    let resolver = Resolver {
        env: &project.env,
        numfig: false,
        numfig_format: &formats,
        math,
        doctree: &|_| None,
        relative_uri: &|_, _| String::new(),
        intersphinx,
    };
    let nitpick = NitpickConfig {
        nitpicky: false,
        ignore: &[],
        ignore_regex: &[],
    };
    let (_, doctree) = project
        .doctrees
        .iter()
        .find(|(name, _)| name == docname)
        .expect("a document of the project");
    let mut doctree = doctree.clone();
    let path = format!("<SRCDIR>/{docname}.rst");
    let resolution = resolve_document(
        &resolver,
        &nitpick,
        docname,
        &mut doctree,
        Path::new(&path),
        Path::new("<SRCDIR>"),
    );
    let pformat = doctree.root.pformat();
    let document = DOC.replace("index.rst", &format!("{docname}.rst"));
    let body = pformat
        .strip_prefix(&document)
        .unwrap_or_else(|| panic!("unexpected document line:\n{pformat}"))
        .to_string();
    Resolved {
        body,
        warnings: resolution
            .warnings
            .into_iter()
            .map(|warning| Diagnostic::from(warning).render(true))
            .collect(),
        unresolvable: resolution.unresolvable_domain_refs,
    }
}

/// An id-trace event of [`Read::trace`].
pub(crate) fn event(phase: &str, kind: &'static str, id: &str, attached: bool) -> Event {
    (phase.to_string(), kind, id.to_string(), attached)
}

/// `<SRCDIR>/index.rst:<line>: ERROR: <text> [docutils]`, as the stream's
/// lines.
pub(crate) fn error(line: &str, text: &str) -> Vec<String> {
    format!("<SRCDIR>/index.rst:{line}: ERROR: {text} [docutils]")
        .split('\n')
        .map(str::to_string)
        .collect()
}

/// What Sphinx 9.1.0's read of one snippet gives through the read-state
/// harness (`keep_warnings = True`): the post-read pformat less [`DOC`],
/// the warning stream's lines and the id trace, each event's phase
/// shortened to its class.
pub(crate) struct SphinxRead {
    pub(crate) rst: &'static str,
    pub(crate) body: &'static str,
    pub(crate) warnings: &'static [&'static str],
    pub(crate) trace: &'static [(&'static str, &'static str, &'static str, bool)],
}

/// Our read of each row ([`read_snippet`], `keep_warnings = True`) against
/// Sphinx's: one entry per row whose body, warning stream or id trace
/// differs, with both sides.
pub(crate) fn differences(rows: &[SphinxRead]) -> Vec<String> {
    rows.iter()
        .filter_map(|row| {
            let read = read_snippet(row.rst, true);
            let trace: Vec<Event> = row
                .trace
                .iter()
                .map(|&(phase, kind, id, attached)| event(phase, kind, id, attached))
                .collect();
            let same =
                read.body == row.body && read.warnings == row.warnings && read.trace == trace;
            (!same).then(|| {
                format!(
                    "{:?}\n--- sphinx 9.1.0 ---\n{}{:#?}\n{:#?}\n--- ours ---\n{}{:#?}\n{:#?}",
                    row.rst, row.body, row.warnings, trace, read.body, read.warnings, read.trace
                )
            })
        })
        .collect()
}
