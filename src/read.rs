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
//! differential compare its output with Sphinx's. [`resolve_standalone`]
//! then writes it as Sphinx's dummy builder does, for the resolved
//! comparison.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::builder::BuilderKind;
use crate::config::BuildConfig;
use crate::diagnostics::{Category, Diagnostic, OrderKey, Severity};
use crate::doctree::{Doctree, Node, Uid};
use crate::env::numbers as env_numbers;
use crate::env::resolve::{NitpickConfig, Resolver};
use crate::env::BuildEnvironment;
use crate::intersphinx::Intersphinx;
use crate::rst::{LogLocation, ParseOptions, ParseOutput, ReadConfig, RegistryExport};
use crate::transforms::post::{apply_post_transforms, PostTransformConfig};
use crate::transforms::reporter::{render_records, LogRecord};
use crate::transforms::state::DocumentState;
use crate::transforms::tags::Tags;
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
    /// `today` (`SP/sphinx/config.py:227`, default `''`): the text of the
    /// `|today|` default substitution when it is set
    /// ([`crate::transforms::sphinx::DefaultSubstitutions`]).
    pub today: String,
    /// `today_fmt` (`config.py:229`, default `None`): the strftime format
    /// `|today|` gives [`Self::build_date`] when `today` is empty; `None`
    /// is the language's own, `'%b %d, %Y'` in `en`.
    pub today_fmt: Option<String>,
    /// `version` and `release` (`config.py:225-226`, default `''`): the
    /// text of the `|version|` and `|release|` default substitutions.
    pub version: String,
    pub release: String,
    /// The date `|today|` formats: what Sphinx's `format_date` takes when
    /// given none (`SP/sphinx/util/i18n.py:268-282`), `SOURCE_DATE_EPOCH`
    /// when it is set and the current time otherwise, in UTC either way.
    /// It is resolved once, when the settings are made ([`build_date`]):
    /// every document of a build reads the same date, and no transform
    /// reads the environment.
    pub build_date: DateTime<Utc>,
    /// `language` (`SP/sphinx/config.py:230`, default `'en'`): the language
    /// whose variant of an image the image collector prefers
    /// ([`crate::env::collectors::images`]).
    pub language: String,
    /// `trim_doctest_flags` (`SP/sphinx/config.py:267`, default `True`):
    /// whether the resolve pass's `TrimDoctestFlagsTransform` strips doctest
    /// flags ([`crate::transforms::post`]), which [`resolve_standalone`]
    /// reads here.
    pub trim_doctest_flags: bool,
}

impl ReadSettings {
    /// Sphinx's defaults, dated as a build that starts now ([`build_date`]).
    pub fn sphinx_defaults() -> Self {
        ReadSettings {
            keep_warnings: false,
            trace_ids: false,
            today: String::new(),
            today_fmt: None,
            version: String::new(),
            release: String::new(),
            build_date: build_date(),
            language: "en".to_string(),
            trim_doctest_flags: true,
        }
    }

    /// The read settings of a build configured by `config`, dated as a
    /// build that starts now ([`build_date`]).
    pub fn from_config(config: &BuildConfig) -> Self {
        ReadSettings {
            keep_warnings: config.keep_warnings,
            today: config.today.clone(),
            today_fmt: config.today_fmt.clone(),
            version: config.version.clone().unwrap_or_default(),
            release: config.release.clone().unwrap_or_default(),
            // Sphinx reads a `language` of `None` as `'en'`
            // (`SP/sphinx/config.py:573-581`).
            language: config.language.clone().unwrap_or_else(|| "en".to_string()),
            trim_doctest_flags: config.trim_doctest_flags,
            ..Self::sphinx_defaults()
        }
    }
}

/// The date `|today|` formats in a build that starts now
/// ([`ReadSettings::build_date`]): `SOURCE_DATE_EPOCH`, read as Sphinx's
/// `format_date` reads it ([`date_of_epoch`]), when it is set, and the
/// current time otherwise. A value Sphinx cannot read crashes it where
/// `|today|` is used; that input is not modelled (decision 13), and the
/// build takes the current time instead.
fn build_date() -> DateTime<Utc> {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|epoch| date_of_epoch(&epoch))
        .unwrap_or_else(Utc::now)
}

/// `datetime.fromtimestamp(float(epoch), tz=UTC)`
/// (`SP/sphinx/util/i18n.py:274-276`): the POSIX timestamp `epoch`, which
/// may have a fraction and surrounding whitespace, its whole seconds taken
/// toward zero and the fraction rounded half to even to microseconds, a
/// rounded fraction of a whole second carried into the seconds (CPython's
/// `_fromtimestamp`). `None` for what `float()` rejects (Python's
/// underscore-separated digits are not read) and for a date outside
/// Python's years 1 to 9999.
fn date_of_epoch(epoch: &str) -> Option<DateTime<Utc>> {
    use chrono::Datelike;
    let timestamp: f64 = epoch.trim_matches(crate::utils::py_isspace).parse().ok()?;
    if !timestamp.is_finite() {
        return None;
    }
    let mut seconds = timestamp.trunc();
    let mut micros = ((timestamp - seconds) * 1e6).round_ties_even();
    if micros >= 1e6 {
        seconds += 1.0;
        micros -= 1e6;
    } else if micros < 0.0 {
        seconds -= 1.0;
        micros += 1e6;
    }
    // Both are whole numbers in range here: `seconds` is bounded below
    // (more than Python's years allow), and `micros` lies in 0..1_000_000.
    if seconds.abs() >= 1e15 {
        return None;
    }
    DateTime::from_timestamp(seconds as i64, micros as u32 * 1000)
        .filter(|date| (1..=9999).contains(&date.year()))
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
        crate::transforms::read_transforms_with(
            output,
            settings,
            &parse.docname,
            &source_dir_of(parse),
            transform,
        )
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
/// records, and those stage M's collectors and transforms log there) and
/// the domains' records stage M returns, rendered by [`render_records`] (a
/// record with no source of its own is located at
/// [`MergeInputs::source_path`]), stable-sorted by key, keys dropped. The
/// log itself is left in place.
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
    /// The document's name ([`ParseOptions::docname`]).
    pub docname: String,
    /// The document's source file ([`ParseOptions::source_path`]).
    pub source_path: PathBuf,
    /// The project's source directory, or the one the source path implies
    /// for a read with none ([`source_dir_of`]).
    pub source_dir: PathBuf,
    /// The configuration the read consumed ([`ParseOptions::read`]), which
    /// the resolve pass reads too ([`resolve_standalone`]).
    pub read_config: ReadConfig,
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
    let source_dir = source_dir_of(parse);
    let diagnostics = if parse.sphinx {
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
        diagnostics
    } else {
        let log = outcome.state().log.clone();
        in_key_order(log, &outcome.doctree().sources, source_path)
    };
    StandaloneRead {
        outcome,
        env,
        diagnostics,
        docname: parse.docname.clone(),
        source_path: source_path.to_path_buf(),
        source_dir,
        read_config: parse.read.clone(),
    }
}

/// The write of a standalone read as Sphinx's dummy build does it, the
/// way the doctree oracles take it (`env.get_and_resolve_doctree(docname,
/// builder)`, `tools/gen_sphinx_fixture.py`): first the numbering
/// `env-get-updated` runs once every document is read, section numbers
/// then figure numbers, over a copy of the read's environment (stage M is
/// never re-run); then the post-transforms over a copy of the post-read
/// doctree, in priority order ([`apply_post_transforms`]): the references
/// resolved against that environment with the dummy builder's `''` URIs,
/// the numbering settings the snippets' `conf.py` leaves at Sphinx's
/// defaults (`numfig = False`, `numfig_secnum_depth = 1`, the default
/// `numfig_format`) and no intersphinx, and the `only` nodes evaluated
/// against `kind`'s tags. The resolved doctree, and the records the
/// numbering and the post-transforms logged, in the order Sphinx prints
/// them.
pub fn resolve_standalone(
    read: &StandaloneRead,
    settings: &ReadSettings,
    kind: BuilderKind,
) -> (Doctree, Vec<Diagnostic>) {
    let mut env = read.env.clone();
    let post_read = read.outcome.doctree();
    let load_doctree =
        |docname: &str| (docname == read.docname).then_some(Cow::Borrowed(post_read));
    // `location=toctreenode`: the toctree's own source and line.
    let sections = env_numbers::assign_section_numbers(&mut env, &load_doctree);
    let mut records: Vec<Diagnostic> = sections
        .warnings
        .iter()
        .map(|warning| {
            let location = match read.outcome.output.toctrees.get(warning.toctree_index) {
                Some(toctree) => LogLocation::Node {
                    source: toctree.source,
                    line: toctree.line,
                },
                None => LogLocation::Doc,
            };
            Diagnostic {
                location: location.render(&post_read.sources, &read.source_dir, &read.docname),
                severity: Severity::Warning,
                message: warning.message.clone(),
                category: warning
                    .category
                    .as_deref()
                    .map_or(Category::None, Category::parse),
            }
        })
        .collect();
    env_numbers::assign_figure_numbers(&mut env, false, 1, &load_doctree);

    let numfig_format = crate::config::default_numfig_format();
    let intersphinx = Intersphinx::default();
    let resolver = Resolver {
        env: &env,
        numfig: false,
        numfig_format: &numfig_format,
        doctree: &load_doctree,
        relative_uri: &|_, _| String::new(),
        intersphinx: &intersphinx,
    };
    let nitpick = NitpickConfig {
        nitpicky: false,
        ignore: &[],
        ignore_regex: &[],
    };
    let tags = Tags::new(kind.tags().iter().map(|tag| tag.to_string()));
    let config = PostTransformConfig {
        tags: &tags,
        highlight_language: &read.read_config.highlight_language,
        trim_doctest_flags: settings.trim_doctest_flags,
        source_dir: &read.source_dir,
    };
    let mut doctree = post_read.clone();
    let post = apply_post_transforms(
        &resolver,
        &nitpick,
        &config,
        &read.docname,
        &read.source_path,
        &mut doctree,
    );
    records.extend(post.records);
    (doctree, records)
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
    /// otherwise (`config.py:225-230`, `:261`, `:267`); the trace is a test
    /// switch no configuration sets. The build date is the moment the settings
    /// were made, so the comparisons carry it over.
    #[test]
    fn read_settings_follow_the_configuration() {
        let defaults = ReadSettings::sphinx_defaults();
        assert_eq!(
            defaults,
            ReadSettings {
                keep_warnings: false,
                trace_ids: false,
                today: String::new(),
                today_fmt: None,
                version: String::new(),
                release: String::new(),
                build_date: defaults.build_date,
                language: "en".to_string(),
                trim_doctest_flags: true,
            }
        );
        assert_eq!(
            ReadSettings {
                build_date: defaults.build_date,
                ..ReadSettings::from_config(&BuildConfig::default())
            },
            defaults
        );
        let config = BuildConfig {
            keep_warnings: true,
            today: "April 1".to_string(),
            today_fmt: Some("%Y".to_string()),
            version: Some("1.2".to_string()),
            release: Some("1.2.3".to_string()),
            language: Some("de".to_string()),
            trim_doctest_flags: false,
            ..BuildConfig::default()
        };
        let settings = ReadSettings::from_config(&config);
        assert!(!settings.trim_doctest_flags);
        assert!(settings.keep_warnings);
        assert!(!settings.trace_ids);
        assert_eq!(settings.today, "April 1");
        assert_eq!(settings.today_fmt.as_deref(), Some("%Y"));
        assert_eq!(
            (settings.version.as_str(), settings.release.as_str()),
            ("1.2", "1.2.3")
        );
        assert_eq!(settings.language, "de");
        let unset = BuildConfig {
            language: None,
            ..BuildConfig::default()
        };
        assert_eq!(ReadSettings::from_config(&unset).language, "en");
    }

    /// `SOURCE_DATE_EPOCH` read as `datetime.fromtimestamp(float(value),
    /// tz=UTC)` (`SP/sphinx/util/i18n.py:274-276`): whole and fractional
    /// seconds, Python's surrounding whitespace, the fraction rounded half
    /// to even to microseconds and carried; what Sphinx cannot read is not
    /// a date.
    //
    // oracle (CPython 3.12): `datetime.fromtimestamp(float(v), tz=UTC)`
    //   for each value below.
    #[test]
    fn source_date_epoch_reads_as_python_does() {
        let date = |epoch: &str| date_of_epoch(epoch).map(|date| date.to_rfc3339());
        assert_eq!(date("0").as_deref(), Some("1970-01-01T00:00:00+00:00"));
        assert_eq!(
            date(" 1700000000\n").as_deref(),
            Some("2023-11-14T22:13:20+00:00")
        );
        assert_eq!(
            date("1700000000.25").as_deref(),
            Some("2023-11-14T22:13:20.250+00:00")
        );
        assert_eq!(date("1.7e9").as_deref(), Some("2023-11-14T22:13:20+00:00"));
        assert_eq!(
            date("1699999999.9999996").as_deref(),
            Some("2023-11-14T22:13:20+00:00")
        );
        assert_eq!(
            date("-1.5").as_deref(),
            Some("1969-12-31T23:59:58.500+00:00")
        );
        for unreadable in ["", "soon", "inf", "nan", "1e300", "1e12"] {
            assert_eq!(date(unreadable), None, "{unreadable:?}");
        }
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
    /// ([DS §4.3]) keeps its id in the detached forest only. Nor is it when
    /// the node the id names left the tree: Footnotes (620) numbers the
    /// discarded reference first, and the one in the text, the reference
    /// too many, becomes a problematic that carries its id after its own,
    /// while `document.ids` still names the reference
    /// (`state.rejected_subst_autofootnote`).
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
                ("parse", kinds::FOOTNOTE_REFERENCE, "id2", false),
                ("parse", kinds::FOOTNOTE, "id3", true),
                (
                    "docutils.transforms.references.Footnotes",
                    kinds::SYSTEM_MESSAGE,
                    "id4",
                    false
                ),
                (
                    "docutils.transforms.references.Footnotes",
                    kinds::PROBLEMATIC,
                    "id5",
                    true
                ),
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
        // The short underline's WARNING, then `SphinxDanglingReferences`'
        // (850-039) INFOs about the two targets nothing references, `dup`
        // and the index entry's, which never print.
        let raised: Vec<(u16, u16, u8)> = b
            .state()
            .log
            .iter()
            .map(|record| match record {
                LogRecord::Reporter { key, level, .. } => (key.priority, key.serial, *level),
                LogRecord::Logger { key, .. } => (key.priority, key.serial, 0),
            })
            .collect();
        assert_eq!(
            raised,
            [
                (0, 0, crate::doctree::messages::WARNING),
                (850, 39, crate::doctree::messages::INFO),
                (850, 39, crate::doctree::messages::INFO),
            ]
        );
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

    /// The environment's parse-time registrations warn about a duplicate
    /// as Sphinx's directive does, while the document is parsed: the
    /// replayed `duplicate … description` records are keyed with the
    /// parse's `(0, 0, seq)` and the registration's seq ([DS §6.4]; [TW
    /// §5.5] rule 1), so they print among the parse's records and before
    /// every transform's. Probed on Sphinx 9.1.0: the envvar duplicate on
    /// line 5 prints between the parse's WARNING on line 2 and its ERROR
    /// on line 9, and the py duplicate before `SphinxDanglingReferences`'
    /// (850-039) ERROR.
    #[test]
    fn parse_time_env_duplicates_keep_parse_order() {
        let rendered = |source: &str| -> Vec<String> {
            read_standalone(source, &sphinx_options(), &ReadSettings::sphinx_defaults())
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.render(true))
                .collect()
        };
        assert_eq!(
            rendered("Title\n====\n\n.. envvar:: X\n.. envvar:: X\n\nPara\nmore\n   indented.\n"),
            [
                "<SRCDIR>/index.rst:2: WARNING: Title underline too short.\n\nTitle\n==== \
                 [docutils]",
                "<SRCDIR>/index.rst:5: WARNING: duplicate envvar description of X, other \
                 instance in index",
                "<SRCDIR>/index.rst:9: ERROR: Unexpected indentation. [docutils]",
            ]
        );
        assert_eq!(
            rendered(".. py:function:: f()\n\n.. py:function:: f()\n\nSee x_.\n"),
            [
                "<SRCDIR>/index.rst:3: WARNING: duplicate object description of f, other \
                 instance in index, use :no-index: for one of them",
                "<SRCDIR>/index.rst:5: ERROR: Unknown target name: \"x\". [docutils]",
            ]
        );
    }

    /// A line-less record — AnonymousHyperlinks' (440) mismatch — of a
    /// document that ends in an include, followed by blank lines or not, is
    /// located at the parse's final position, the padding docutils inserts
    /// after the included lines (`n + 4` for the 3 lines of `inc.rst`).
    /// Probed on Sphinx 9.1.0 for both inputs; the oracle's cwd is the
    /// source directory, so the padding names `inc.rst` as the include
    /// wrote it.
    #[test]
    fn line_less_records_after_a_trailing_include_locate_at_its_padding() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join("inc.rst"), "A.\n\nB.\n").expect("write");
        let parse = ParseOptions {
            source_path: tmp.path().join("index.rst").display().to_string(),
            srcdir: Some(tmp.path().to_path_buf()),
            ..sphinx_options()
        };
        for source in [
            "`a`__\n\n.. include:: inc.rst\n",
            "`a`__\n\n.. include:: inc.rst\n\n",
        ] {
            let rendered: Vec<String> =
                read_standalone(source, &parse, &ReadSettings::sphinx_defaults())
                    .diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.render(true))
                    .collect();
            assert_eq!(
                rendered,
                [
                    "internal padding after inc.rst:7: ERROR: Anonymous hyperlink mismatch: 1 \
                     references but 0 targets.\nSee \"backrefs\" attribute for IDs. [docutils]"
                ],
                "{source:?}"
            );
        }
    }

    /// A document's records print in key order, not in the order they
    /// were raised: stage M raises the replayed parse-time duplicates with
    /// the parse's `(0, 0)` after the parse's own, and each slot's records
    /// sort by its key whatever their seqs. Records with equal keys keep
    /// their order.
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
                ..ReadSettings::sphinx_defaults()
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
