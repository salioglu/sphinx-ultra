//! Reading one document: the parse, then the read transforms (wave 5a).
//!
//! Sphinx reads a document by parsing it into a docutils `document` and
//! applying every read transform to that same object; what it pickles is
//! the post-read doctree ([DS §2.1]). The read here has the same two halves
//! ([DS §9]): stage R ([`read_document`]) parses and runs the transforms
//! that need nothing but the document, in parallel with the other reads;
//! stage M merges the document into the build environment, one document at
//! a time in sorted docname order. Until the transform table lands (T9b),
//! stage R is the parse plus the one stamp every Sphinx document carries,
//! and stage M is the builder's existing merge.
//!
//! [`read_standalone`] reads one snippet the way the oracle harnesses do,
//! with no project around it: the doctree differentials and the read-state
//! differential compare its output with Sphinx's.

use std::collections::HashSet;

use crate::config::BuildConfig;
use crate::diagnostics::Diagnostic;
use crate::doctree::{AttrValue, Doctree, Node};
use crate::env::BuildEnvironment;
use crate::rst::{ParseOptions, ParseOutput, RegistryExport};
use crate::transforms::reporter::render_records;
use crate::transforms::state::DocumentState;

/// The value `TranslationProgressTotaliser` (025) stamps on every document
/// of a build with no translations (`SP/sphinx/transforms/i18n.py:631-651`).
const TRANSLATION_PROGRESS: &str = "{'total': 0, 'translated': 0}";

/// The configuration the read consumes. Every field holds Sphinx's default
/// unless the configuration says otherwise ([`Self::from_config`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSettings {
    /// `keep_warnings` (`SP/sphinx/config.py:261`, default `False`): whether
    /// `FilterSystemMessages` (999, T9b) keeps the reporter's
    /// `system_message`s of level WARNING and up in the post-read doctree,
    /// rather than removing every one.
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

/// Stage R: parse `source` and run the read transforms that need nothing
/// but the document. No transform is ported yet (T9b); a Sphinx-mode read
/// stamps `translation_progress` on the document, as Sphinx's 025
/// `TranslationProgressTotaliser` does on every document of a build with no
/// translations (decision 9).
pub fn read_document(source: &str, parse: &ParseOptions, settings: &ReadSettings) -> ReadOutcome {
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
    if parse.sphinx {
        output.doctree.root.set(
            "translation_progress",
            AttrValue::Str(TRANSLATION_PROGRESS.to_string()),
        );
    }
    ReadOutcome { output }
}

/// A document read on its own, as the oracle harnesses read a snippet.
pub struct StandaloneRead {
    pub outcome: ReadOutcome,
    /// The environment stage M merged the document into, fresh for this
    /// read. Stage M is empty until T9b, so it stays fresh.
    pub env: BuildEnvironment,
    /// The read's warning-stream records, in the order Sphinx prints them.
    pub diagnostics: Vec<Diagnostic>,
}

/// Stage R and a one-document stage M against a fresh environment, which
/// it returns, with the read's records: the document's log rendered by
/// [`render_records`] (a record with no source of its own is located at
/// `parse.source_path`, docutils' `reporter.source`), stable-sorted by key,
/// keys dropped. With [`ReadSettings::trace_ids`] on, every id-trace
/// event's `attached` says whether the read tree still holds the node the
/// event's id names.
pub fn read_standalone(
    source: &str,
    parse: &ParseOptions,
    settings: &ReadSettings,
) -> StandaloneRead {
    let mut outcome = read_document(source, parse, settings);
    let mut keyed = render_records(
        outcome.state().log.clone(),
        &outcome.doctree().sources,
        &parse.source_path,
    );
    keyed.sort_by_key(|(key, _)| *key);
    let diagnostics = keyed
        .into_iter()
        .map(|(_, diagnostic)| diagnostic)
        .collect();
    if settings.trace_ids {
        fill_attached(&mut outcome.output);
    }
    StandaloneRead {
        outcome,
        env: BuildEnvironment::default(),
        diagnostics,
    }
}

/// Sets each id-trace event's `attached` to whether the tree still holds
/// the node docutils' `document.ids` maps the event's id to. With no
/// transform run yet, that node is the id's first carrier in a pre-order
/// walk of the tree and then of the detached forest, so the event is
/// attached exactly when some node of the tree carries its id (T5a's rule;
/// T9b resolves the id through the transform tree instead).
fn fill_attached(output: &mut ParseOutput) {
    fn carried<'a>(node: &'a Node, ids: &mut HashSet<&'a str>) {
        ids.extend(node.attrs.ids.iter().map(String::as_str));
        for child in &node.children {
            carried(child, ids);
        }
    }
    let ParseOutput { doctree, state, .. } = output;
    let mut ids = HashSet::new();
    carried(&doctree.root, &mut ids);
    for event in state.ids.trace_mut() {
        event.attached = ids.contains(event.id.as_str());
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
    /// too-short underline of `Short` does not print) and in key order.
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
        assert_eq!(read.env, BuildEnvironment::default());
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
}
