//! Sphinx's read transforms over a parsed document (wave 5a).
//!
//! The parser stays transform-free (decision 1); it hands its docutils
//! document state on beside the doctree, and this stage reads both.
//!
//! Sphinx parses a document into one docutils `document` and then applies
//! every read transform to that same object, in docutils' priority order
//! (`SP/docutils/transforms/__init__.py:142-149`, `:181-196`): the 42
//! transforms a default build registers ([`READ_TRANSFORMS`]) and the ones
//! the parse left pending ([`schedule`]). The read runs them in two stages
//! ([DS §9]):
//!
//! - stage R ([`run_read_transforms`]): every transform before
//!   `SphinxDomains` (850-040), the ones that need nothing but the
//!   document, over its [`TxTree`], in parallel with the other documents'
//!   reads;
//! - stage M ([`run_merge_transforms`]): `SphinxDomains` and the rest,
//!   which merge the document into the build environment, one document at
//!   a time in sorted docname order.
//!
//! A row nothing ports yet runs as an explicit no-op
//! ([`TransformId::transform`]).

pub mod footnotes;
pub mod location;
pub mod misc;
pub mod references;
pub mod reporter;
pub mod sphinx;
pub mod state;
#[cfg(test)]
pub(crate) mod test_support;
pub mod tree;

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use crate::diagnostics::{Category, Diagnostic, Location, OrderKey, Severity};
use crate::doctree::messages;
use crate::doctree::pending::PendingTransform;
use crate::doctree::{kinds, Doctree, Node, Span};
use crate::env::dependencies as env_dependencies;
use crate::env::genindex as env_genindex;
use crate::env::metadata as env_metadata;
use crate::env::std_domain as env_std;
use crate::env::toctree::{self as env_toctree, ToctreeWarningKind};
use crate::env::BuildEnvironment;
use crate::read::{MergeInputs, ReadOutcome, ReadSettings};
use crate::rst::{LogLocation, ParseOutput};
use reporter::LogRecord;
use state::{DocumentState, SourceRef};
use tree::{TxId, TxTree};

/// A transform's place in the read: docutils' priority string
/// `'%03d-%03d' % (priority, serial)`, which orders as the pair does.
/// `priority` is the transform's `default_priority`; `serial` numbers the
/// transforms in the order the transformer was given them, the statics in
/// registration order and then the pending ones in parse order
/// ([`schedule`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransformKey {
    pub priority: u16,
    pub serial: u16,
}

impl TransformKey {
    const fn at(priority: u16, serial: u16) -> TransformKey {
        TransformKey { priority, serial }
    }

    /// The key of a record the transform logs: its own `(priority,
    /// serial)`, and `seq` from the document's record counter
    /// ([`DocumentState::draw_seq`]), which orders the transform's records
    /// as it raised them ([DS §6.4]).
    pub fn order_key(self, seq: u32) -> OrderKey {
        OrderKey {
            priority: self.priority,
            serial: self.serial,
            seq,
        }
    }
}

/// The transforms of a read: one per row of the read-transform table
/// ([`READ_TRANSFORMS`]), and the four a `pending` node can defer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransformId {
    ApplySourceWorkaround,
    ExtraTranslatableNodes,
    PreserveTranslatableMessages,
    Locale,
    TranslationProgressTotaliser,
    RefOnlyBulletListTransform,
    DefaultSubstitutions,
    MoveModuleTargets,
    HandleCodeBlocks,
    AutoNumbering,
    AutoIndexUpgrader,
    Substitutions,
    ReorderConsecutiveTargetAndIndexNodes,
    PropagateTargets,
    SortIds,
    DocTitle,
    DocInfo,
    SectionSubTitle,
    AnonymousHyperlinks,
    IndirectHyperlinks,
    DoctestTransform,
    GlossarySorter,
    CitationDefinitionTransform,
    CitationReferenceTransform,
    Footnotes,
    UnreferencedFootnotesDetector,
    ExternalTargets,
    InternalTargets,
    FootnoteDocnameUpdater,
    StripComments,
    SphinxSmartQuotes,
    Decorations,
    Transitions,
    Validate,
    ExposeInternals,
    SphinxDanglingReferences,
    SphinxDomains,
    DoctreeReadEvent,
    UIDTransform,
    AddTranslationClasses,
    FilterSystemMessages,
    RemoveTranslatableInline,
    /// Pending: `.. class::` / `.. rst-class::` without content.
    ClassAttribute,
    /// Pending: `.. target-notes::`.
    TargetNotes,
    /// Pending: `.. sectnum::`.
    SectNum,
    /// Pending: `.. contents::`.
    Contents,
}

impl TransformId {
    /// The transform class's dotted Python name,
    /// `f"{cls.__module__}.{cls.__qualname__}"`: what the read-state
    /// fixture's id trace records as the phase of an id the transform
    /// allocates.
    pub const fn python_name(self) -> &'static str {
        match self {
            TransformId::ApplySourceWorkaround => "sphinx.transforms.ApplySourceWorkaround",
            TransformId::ExtraTranslatableNodes => "sphinx.transforms.ExtraTranslatableNodes",
            TransformId::PreserveTranslatableMessages => {
                "sphinx.transforms.i18n.PreserveTranslatableMessages"
            }
            TransformId::Locale => "sphinx.transforms.i18n.Locale",
            TransformId::TranslationProgressTotaliser => {
                "sphinx.transforms.i18n.TranslationProgressTotaliser"
            }
            TransformId::RefOnlyBulletListTransform => {
                "sphinx.transforms.compact_bullet_list.RefOnlyBulletListTransform"
            }
            TransformId::DefaultSubstitutions => "sphinx.transforms.DefaultSubstitutions",
            TransformId::MoveModuleTargets => "sphinx.transforms.MoveModuleTargets",
            TransformId::HandleCodeBlocks => "sphinx.transforms.HandleCodeBlocks",
            TransformId::AutoNumbering => "sphinx.transforms.AutoNumbering",
            TransformId::AutoIndexUpgrader => "sphinx.transforms.AutoIndexUpgrader",
            TransformId::Substitutions => "docutils.transforms.references.Substitutions",
            TransformId::ReorderConsecutiveTargetAndIndexNodes => {
                "sphinx.transforms.ReorderConsecutiveTargetAndIndexNodes"
            }
            TransformId::PropagateTargets => "docutils.transforms.references.PropagateTargets",
            TransformId::SortIds => "sphinx.transforms.SortIds",
            TransformId::DocTitle => "docutils.transforms.frontmatter.DocTitle",
            TransformId::DocInfo => "docutils.transforms.frontmatter.DocInfo",
            TransformId::SectionSubTitle => "docutils.transforms.frontmatter.SectionSubTitle",
            TransformId::AnonymousHyperlinks => {
                "docutils.transforms.references.AnonymousHyperlinks"
            }
            TransformId::IndirectHyperlinks => "docutils.transforms.references.IndirectHyperlinks",
            TransformId::DoctestTransform => "sphinx.transforms.DoctestTransform",
            TransformId::GlossarySorter => "sphinx.transforms.GlossarySorter",
            TransformId::CitationDefinitionTransform => {
                "sphinx.domains.citation.CitationDefinitionTransform"
            }
            TransformId::CitationReferenceTransform => {
                "sphinx.domains.citation.CitationReferenceTransform"
            }
            TransformId::Footnotes => "docutils.transforms.references.Footnotes",
            TransformId::UnreferencedFootnotesDetector => {
                "sphinx.transforms.UnreferencedFootnotesDetector"
            }
            TransformId::ExternalTargets => "docutils.transforms.references.ExternalTargets",
            TransformId::InternalTargets => "docutils.transforms.references.InternalTargets",
            TransformId::FootnoteDocnameUpdater => {
                "sphinx.builders.latex.transforms.FootnoteDocnameUpdater"
            }
            TransformId::StripComments => "docutils.transforms.universal.StripComments",
            TransformId::SphinxSmartQuotes => "sphinx.transforms.SphinxSmartQuotes",
            TransformId::Decorations => "docutils.transforms.universal.Decorations",
            TransformId::Transitions => "docutils.transforms.misc.Transitions",
            TransformId::Validate => "docutils.transforms.universal.Validate",
            TransformId::ExposeInternals => "docutils.transforms.universal.ExposeInternals",
            TransformId::SphinxDanglingReferences => {
                "sphinx.transforms.references.SphinxDanglingReferences"
            }
            TransformId::SphinxDomains => "sphinx.transforms.references.SphinxDomains",
            TransformId::DoctreeReadEvent => "sphinx.transforms.DoctreeReadEvent",
            TransformId::UIDTransform => "sphinx.versioning.UIDTransform",
            TransformId::AddTranslationClasses => "sphinx.transforms.i18n.AddTranslationClasses",
            TransformId::FilterSystemMessages => "sphinx.transforms.FilterSystemMessages",
            TransformId::RemoveTranslatableInline => {
                "sphinx.transforms.i18n.RemoveTranslatableInline"
            }
            TransformId::ClassAttribute => PendingTransform::ClassAttribute.python_path(),
            TransformId::TargetNotes => PendingTransform::TargetNotes.python_path(),
            TransformId::SectNum => PendingTransform::SectNum.python_path(),
            TransformId::Contents => PendingTransform::Contents.python_path(),
        }
    }

    /// The transform a `pending` node defers.
    const fn pending(transform: PendingTransform) -> TransformId {
        match transform {
            PendingTransform::ClassAttribute => TransformId::ClassAttribute,
            PendingTransform::TargetNotes => TransformId::TargetNotes,
            PendingTransform::SectNum => TransformId::SectNum,
            PendingTransform::Contents => TransformId::Contents,
        }
    }

    /// The transform the read runs for this row: its port, or an explicit
    /// no-op for a row that is inert in a default build, not ported yet
    /// (the comment names the task that ports it), or run against the
    /// environment by stage M itself ([`run_merge_transforms`]).
    pub(crate) fn transform(self) -> &'static dyn Transform {
        match self {
            TransformId::ApplySourceWorkaround => &sphinx::ApplySourceWorkaround,
            TransformId::TranslationProgressTotaliser => &sphinx::TranslationProgressTotaliser,
            TransformId::DefaultSubstitutions => &sphinx::DefaultSubstitutions,
            TransformId::MoveModuleTargets => &sphinx::MoveModuleTargets,
            TransformId::HandleCodeBlocks => &sphinx::HandleCodeBlocks,
            TransformId::Substitutions => &references::Substitutions,
            TransformId::ReorderConsecutiveTargetAndIndexNodes => {
                &sphinx::ReorderConsecutiveTargetAndIndexNodes
            }
            TransformId::PropagateTargets => &references::PropagateTargets,
            TransformId::SortIds => &sphinx::SortIds,
            TransformId::AnonymousHyperlinks => &references::AnonymousHyperlinks,
            TransformId::IndirectHyperlinks => &references::IndirectHyperlinks,
            TransformId::DoctestTransform => &sphinx::DoctestTransform,
            TransformId::GlossarySorter => &sphinx::GlossarySorter,
            // The 619 rows' tree halves; T13b their environment half.
            TransformId::CitationDefinitionTransform => &footnotes::CitationDefinitionTransform,
            TransformId::CitationReferenceTransform => &footnotes::CitationReferenceTransform,
            TransformId::Footnotes => &footnotes::Footnotes,
            TransformId::UnreferencedFootnotesDetector => &footnotes::UnreferencedFootnotesDetector,
            TransformId::ExternalTargets => &references::ExternalTargets,
            TransformId::InternalTargets => &references::InternalTargets,
            TransformId::FootnoteDocnameUpdater => &footnotes::FootnoteDocnameUpdater,
            TransformId::Transitions => &misc::Transitions,
            TransformId::SphinxDanglingReferences => &references::SphinxDanglingReferences,
            TransformId::FilterSystemMessages => &sphinx::FilterSystemMessages,

            // Inert with `extensions = []` and Sphinx's defaults ([TW §2.2]):
            // no message catalogs or gettext builder (010-019, 010-034,
            // 020, 880-041, 950, 999-038), `html_compact_lists` on (100),
            // no 4-tuple index entries (210-026), `doctitle_xform` and
            // `sectsubtitle_xform` off (320, 350), no `strip_comments`
            // (740), no header or footer (820), `validate` off (835), and
            // nothing to expose (840).
            TransformId::ExtraTranslatableNodes
            | TransformId::PreserveTranslatableMessages
            | TransformId::Locale
            | TransformId::RefOnlyBulletListTransform
            | TransformId::AutoIndexUpgrader
            | TransformId::DocTitle
            | TransformId::SectionSubTitle
            | TransformId::StripComments
            | TransformId::Decorations
            | TransformId::Validate
            | TransformId::ExposeInternals
            | TransformId::UIDTransform
            | TransformId::AddTranslationClasses
            | TransformId::RemoveTranslatableInline => &NoOp,

            // T12.
            TransformId::AutoNumbering | TransformId::DocInfo | TransformId::ClassAttribute => {
                &NoOp
            }
            // T16b.
            TransformId::SphinxSmartQuotes => &NoOp,
            // T18b.
            TransformId::TargetNotes | TransformId::SectNum | TransformId::Contents => &NoOp,

            // Stage M runs these two against the environment itself.
            TransformId::SphinxDomains | TransformId::DoctreeReadEvent => &NoOp,
        }
    }
}

/// The 42 transforms Sphinx 9.1.0 applies to every document a default
/// build reads (`extensions = []`), in the order it applies them, each with
/// its key ([TW Appendix A §E]). The serials are the registration order:
/// docutils' standalone reader's transforms less `DanglingReferences`
/// (1-14), Sphinx's registry (15-41) and the rst parser's `Validate` (42)
/// (`SP/sphinx/util/docutils.py:879-882`).
pub static READ_TRANSFORMS: &[(TransformKey, TransformId)] = &[
    (TransformKey::at(10, 18), TransformId::ApplySourceWorkaround),
    (
        TransformKey::at(10, 19),
        TransformId::ExtraTranslatableNodes,
    ),
    (
        TransformKey::at(10, 34),
        TransformId::PreserveTranslatableMessages,
    ),
    (TransformKey::at(20, 35), TransformId::Locale),
    (
        TransformKey::at(25, 36),
        TransformId::TranslationProgressTotaliser,
    ),
    (
        TransformKey::at(100, 33),
        TransformId::RefOnlyBulletListTransform,
    ),
    (TransformKey::at(210, 20), TransformId::DefaultSubstitutions),
    (TransformKey::at(210, 21), TransformId::MoveModuleTargets),
    (TransformKey::at(210, 22), TransformId::HandleCodeBlocks),
    (TransformKey::at(210, 25), TransformId::AutoNumbering),
    (TransformKey::at(210, 26), TransformId::AutoIndexUpgrader),
    (TransformKey::at(220, 4), TransformId::Substitutions),
    (
        TransformKey::at(220, 32),
        TransformId::ReorderConsecutiveTargetAndIndexNodes,
    ),
    (TransformKey::at(260, 5), TransformId::PropagateTargets),
    (TransformKey::at(261, 23), TransformId::SortIds),
    (TransformKey::at(320, 6), TransformId::DocTitle),
    (TransformKey::at(340, 8), TransformId::DocInfo),
    (TransformKey::at(350, 7), TransformId::SectionSubTitle),
    (TransformKey::at(440, 9), TransformId::AnonymousHyperlinks),
    (TransformKey::at(460, 10), TransformId::IndirectHyperlinks),
    (TransformKey::at(500, 24), TransformId::DoctestTransform),
    (TransformKey::at(500, 31), TransformId::GlossarySorter),
    (
        TransformKey::at(619, 16),
        TransformId::CitationDefinitionTransform,
    ),
    (
        TransformKey::at(619, 17),
        TransformId::CitationReferenceTransform,
    ),
    (TransformKey::at(620, 11), TransformId::Footnotes),
    (
        TransformKey::at(622, 28),
        TransformId::UnreferencedFootnotesDetector,
    ),
    (TransformKey::at(640, 12), TransformId::ExternalTargets),
    (TransformKey::at(660, 13), TransformId::InternalTargets),
    (
        TransformKey::at(700, 15),
        TransformId::FootnoteDocnameUpdater,
    ),
    (TransformKey::at(740, 3), TransformId::StripComments),
    (TransformKey::at(750, 29), TransformId::SphinxSmartQuotes),
    (TransformKey::at(820, 1), TransformId::Decorations),
    (TransformKey::at(830, 14), TransformId::Transitions),
    (TransformKey::at(835, 42), TransformId::Validate),
    (TransformKey::at(840, 2), TransformId::ExposeInternals),
    (
        TransformKey::at(850, 39),
        TransformId::SphinxDanglingReferences,
    ),
    (SPHINX_DOMAINS, TransformId::SphinxDomains),
    (DOCTREE_READ_EVENT, TransformId::DoctreeReadEvent),
    (TransformKey::at(880, 41), TransformId::UIDTransform),
    (
        TransformKey::at(950, 37),
        TransformId::AddTranslationClasses,
    ),
    (TransformKey::at(999, 27), TransformId::FilterSystemMessages),
    (
        TransformKey::at(999, 38),
        TransformId::RemoveTranslatableInline,
    ),
];

/// `SphinxDomains`' key, where stage M starts.
const SPHINX_DOMAINS: TransformKey = TransformKey::at(850, 40);

/// `DoctreeReadEvent`'s key: the `doctree-read` collectors' slot.
const DOCTREE_READ_EVENT: TransformKey = TransformKey::at(880, 30);

/// The read's transforms in the order docutils applies them ([DS §2.2]):
/// the table's, and each pending transform the parse queued
/// ([`DocumentState::pending`]). The transformer numbers the pending
/// transforms after the 42 statics, in `note_pending` order
/// (`Transformer.add_pending`, `SP/docutils/transforms/__init__.py:132-141`),
/// so a pending transform runs after every static of its priority, and
/// pending transforms of one priority run in parse order. Past serial
/// `u16::MAX` the serials saturate; the sort is stable, so the order holds.
pub fn schedule(state: &DocumentState) -> Vec<(TransformKey, TransformId)> {
    let first_pending = READ_TRANSFORMS.len() + 1;
    let mut schedule = READ_TRANSFORMS.to_vec();
    schedule.extend(state.pending.iter().map(|record| {
        let serial = usize::try_from(record.order)
            .ok()
            .and_then(|order| u16::try_from(first_pending + order).ok())
            .unwrap_or(u16::MAX);
        (
            TransformKey::at(record.transform.priority(), serial),
            TransformId::pending(record.transform),
        )
    }));
    schedule.sort_by_key(|(key, _)| *key);
    schedule
}

/// One transform's view of the document while it applies.
pub struct TransformCx<'a> {
    /// The document's nodes, the detached forest included in stage R.
    pub tree: &'a mut TxTree,
    /// The docutils document state beside the tree: ids, names, registries,
    /// pending records and the record log.
    pub state: &'a mut DocumentState,
    pub settings: &'a ReadSettings,
    /// The applying row's key: what its records are keyed by
    /// ([`TransformKey::order_key`]), and, for a pending transform, which
    /// `note_pending` record it applies to ([`schedule`]).
    pub key: TransformKey,
    pub docname: &'a str,
    /// The project's source directory (`env.srcdir`), which the logger
    /// records' locations render against ([`Self::warn`]).
    pub srcdir: &'a Path,
}

impl TransformCx<'_> {
    /// docutils `document.reporter.system_message(level, text,
    /// base_node=node)` while this transform applies
    /// (`SP/docutils/utils/__init__.py:171-221`). The `system_message` is
    /// located where docutils locates `node` ([`location::get_source_line`])
    /// or, when that has neither a source nor a line, at the parse's final
    /// position ([`DocumentState::final_position`], [DS §6.2]); its
    /// `source` is the location's, the document's own when it names none
    /// (`reporter.source`). Docutils writes the message to the warning
    /// stream as it makes it, so it is logged here, with its text as it is
    /// now: a reporter record keyed by this transform's row and the next
    /// seq ([`TransformKey::order_key`]). The node is returned in no
    /// parent, for the transform to place or to leave out of the tree.
    pub fn report(&mut self, level: u8, text: &str, node: TxId) -> TxId {
        self.system_message(level, text, Some(node), Vec::new())
    }

    /// [`Self::report`] with docutils' optional parts: `base_node` may be
    /// missing (`reporter.error(text)`), which locates the message at the
    /// parse's final position, and `children` follow the message's
    /// paragraph, in its text too.
    pub fn system_message(
        &mut self,
        level: u8,
        text: &str,
        base_node: Option<TxId>,
        children: Vec<Node>,
    ) -> TxId {
        let mut loc = base_node.map_or_else(Default::default, |node| {
            location::get_source_line(self.tree, node, self.state)
        });
        if loc.source.is_none() && loc.line.is_none() {
            loc = self.state.final_position;
        }
        let message = messages::system_message(
            level,
            text,
            self.tree.source_path(loc.source),
            loc.line,
            children,
        );
        let seq = self.state.draw_seq();
        self.state.log.push(LogRecord::Reporter {
            key: self.key.order_key(seq),
            level,
            loc,
            text: message.element_astext(),
        });
        self.tree.new_subtree(message)
    }

    /// Sphinx's `logger.warning(message, type=…, subtype=…, location=node)`
    /// while this transform applies: a WARNING record with `category`,
    /// keyed by this transform's row and the next seq
    /// ([`TransformKey::order_key`]), and located where Sphinx's
    /// `get_node_location` (`SP/sphinx/util/logging.py:554-562`) locates
    /// `node`: docutils' location for it ([`location::get_source_line`]),
    /// printed `abspath(source):line` ([`LogLocation::Node`]), or
    /// `abspath(source):` when the line is missing or 0, which is falsy
    /// there (not the reporter's `source:0`); `<unknown>:line` with a line
    /// and no source; and nowhere, a bare record, with neither.
    pub fn warn(&mut self, node: TxId, message: String, category: Category) {
        let loc = location::get_source_line(self.tree, node, self.state);
        let line = loc.line.unwrap_or(0);
        let location = match loc.source {
            Some(SourceRef::Table(source)) => LogLocation::Node { source, line }.render(
                self.tree.sources(),
                self.srcdir,
                self.docname,
            ),
            None if line != 0 => Location::Verbatim(format!("<unknown>:{line}")),
            None => Location::None,
        };
        let seq = self.state.draw_seq();
        self.state.log.push(LogRecord::Logger {
            key: self.key.order_key(seq),
            diagnostic: Diagnostic {
                location,
                severity: Severity::Warning,
                message,
                category,
            },
        });
    }

    /// docutils `document.set_id(node)` for a node without ids, as every
    /// node a transform makes is (`SP/docutils/nodes.py:1844-1875`): the
    /// next id the allocator hands out ([`state::IdAllocator::allocate`]),
    /// which the node takes and docutils' `document.ids` maps to it.
    pub fn set_id(&mut self, node: TxId) -> String {
        let fresh = self.tree.node(node);
        debug_assert!(fresh.attrs.ids.is_empty(), "set_id on a node with ids");
        let id = self
            .state
            .ids
            .allocate(fresh.kind, &fresh.attrs.names, fresh.uid());
        self.tree.node_mut(node).attrs.ids.push(id.clone());
        self.tree.note_id(&id, node);
        id
    }

    /// docutils `document.note_refid(node)` (`nodes.py:2012-2013`): the
    /// node joins `document.refids` under its `refid`.
    pub fn note_refid(&mut self, node: TxId) {
        if let Some(refid) = self.tree.node(node).str_attr("refid").map(str::to_string) {
            self.tree.refids_mut().push(&refid, node);
        }
    }
}

/// One read transform: docutils' `Transform.apply` over the document's
/// tree and state.
pub trait Transform {
    fn apply(&self, cx: &mut TransformCx<'_>);
}

/// The transform of a row that does nothing in this read
/// ([`TransformId::transform`]).
struct NoOp;

impl Transform for NoOp {
    fn apply(&self, _cx: &mut TransformCx<'_>) {}
}

/// Applies row `(key, id)`'s transform, `transform`, to the document
/// `docname` of the project at `srcdir`, after setting the id allocator's
/// phase to the row's [`TransformId::python_name`], so that every id it
/// allocates is traced as its own.
fn apply(
    tree: &mut TxTree,
    state: &mut DocumentState,
    settings: &ReadSettings,
    (docname, srcdir): (&str, &Path),
    (key, id): (TransformKey, TransformId),
    transform: &dyn Transform,
) {
    state.ids.set_phase(id.python_name().to_string());
    transform.apply(&mut TransformCx {
        tree,
        state,
        settings,
        key,
        docname,
        srcdir,
    });
}

/// Stage R: every row of the read's [`schedule`] before `SphinxDomains`
/// (850-040), the transforms that need nothing but the document, applied
/// in order to its [`TxTree`] ([`apply`]). The arena takes the parse's
/// detached forest over ([`TxTree::from_parse`]) and the stage hands on
/// the root's tree alone, so the outcome's state keeps no detached
/// subtree: whatever checks the parse's registrations against the tree
/// ([`DocumentState::registries_dump`],
/// [`DocumentState::misplaced_registrations`]) reads the parse's state,
/// before this stage.
///
/// With the id trace on, the stage ends by tagging the trace's events
/// ([`tag_trace`]): stage M can still take a node out of the tree, so
/// whether an event's node stays is known only when the read is over.
///
/// `srcdir` is the project's source directory, which the stage's logger
/// records render their locations against ([`TransformCx::warn`]).
pub fn run_read_transforms(
    parsed: ParseOutput,
    settings: &ReadSettings,
    docname: &str,
    srcdir: &Path,
) -> ReadOutcome {
    read_transforms_with(parsed, settings, docname, srcdir, &TransformId::transform)
}

/// [`run_read_transforms`], with each row run by the transform `transform`
/// gives it: the read's own ([`TransformId::transform`]), or, in a test,
/// one swapped in.
pub(crate) fn read_transforms_with(
    parsed: ParseOutput,
    settings: &ReadSettings,
    docname: &str,
    srcdir: &Path,
    transform: &dyn Fn(TransformId) -> &'static dyn Transform,
) -> ReadOutcome {
    let ParseOutput {
        doctree,
        directive_records,
        role_records,
        toctrees,
        registry,
        mut state,
    } = parsed;
    let mut tree = TxTree::from_parse(doctree, &mut state);
    for (key, id) in schedule(&state) {
        if key >= SPHINX_DOMAINS {
            break;
        }
        apply(
            &mut tree,
            &mut state,
            settings,
            (docname, srcdir),
            (key, id),
            transform(id),
        );
    }
    tag_trace(&mut tree, &mut state);
    ReadOutcome {
        output: ParseOutput {
            doctree: tree.into_doctree(),
            directive_records,
            role_records,
            toctrees,
            registry,
            state,
        },
    }
}

/// Tags each id-trace event whose id names, through docutils'
/// `document.ids` ([`TxTree::by_id`]), a node in the root's tree with that
/// node's identity, minted for a node that has none
/// ([`TxTree::assign_uid`]; only a traced read mints these). An event whose
/// node is detached, out of the tree or nowhere stays untagged.
fn tag_trace(tree: &mut TxTree, state: &mut DocumentState) {
    if state.ids.trace().is_empty() {
        return;
    }
    let in_tree: HashSet<TxId> = tree.findall(tree.root(), |_| true).into_iter().collect();
    for event in 0..state.ids.trace().len() {
        let node = tree.by_id(&state.ids.trace()[event].id);
        if let Some(node) = node.filter(|node| in_tree.contains(node)) {
            let uid = tree.assign_uid(node, state);
            state.ids.tag_event(event, uid);
        }
    }
}

/// Stage M for the document `docname`: the rows of the read's
/// [`schedule`] from `SphinxDomains` (850-040) on, which merge the document
/// into `env`. It returns their records, each keyed by the slot that
/// raised it ([`TransformKey::order_key`]): the domains' `(850, 40, seq)`,
/// and a `doctree-read` collector's `(880, 30, seq)`.
///
/// The environment first forgets what an earlier read of the document
/// left (`env.clear_doc`, which `_read_serial` runs before every read) and
/// records the read's time. The two environment slots then run in the
/// order the merge has always run them, the collectors
/// ([`doctree_read_event`]) before the domains ([`sphinx_domains`]), until
/// T13a puts them in Sphinx's. The rows after them run over the
/// document's [`TxTree`], re-entered with no detached forest (stage R's
/// arena kept it): `FilterSystemMessages` (999-027) and the inert rest.
///
/// The parse's registry export and toctree records are read from `out`
/// ([`ReadOutcome`]).
pub fn run_merge_transforms(
    out: &mut ReadOutcome,
    env: &mut BuildEnvironment,
    docname: &str,
    inputs: &MergeInputs<'_>,
    settings: &ReadSettings,
) -> Vec<LogRecord> {
    // A re-read replaces the document's state wholesale: without the
    // clear, the `extend`-shaped fields (`toctree_includes`) would
    // accumulate duplicates across rebuilds.
    env.clear_doc(docname);
    env.all_docs
        .insert(docname.to_string(), inputs.read_time_us);

    let mut records = Vec::new();
    let phase = |out: &mut ReadOutcome, id: TransformId| {
        out.state_mut().ids.set_phase(id.python_name().to_string());
    };
    phase(out, TransformId::DoctreeReadEvent);
    doctree_read_event(out, env, docname, inputs);
    phase(out, TransformId::SphinxDomains);
    sphinx_domains(out, env, docname, inputs, &mut records);

    let ParseOutput { doctree, state, .. } = &mut out.output;
    let parsed = std::mem::replace(
        doctree,
        Doctree {
            root: Node::elem(kinds::DOCUMENT, Span::ZERO),
            sources: Vec::new(),
        },
    );
    let mut tree = TxTree::from_parse(parsed, state);
    for (key, id) in schedule(state) {
        if key > DOCTREE_READ_EVENT {
            apply(
                &mut tree,
                state,
                settings,
                (docname, inputs.source_dir),
                (key, id),
                id.transform(),
            );
        }
    }
    *doctree = tree.into_doctree();
    records
}

/// 880-030 `DoctreeReadEvent`: the `doctree-read` collectors as the merge
/// has run them since wave 4 — Title, Metadata, Dependencies, TocTree —
/// with two parse-time environment calls replayed among them where the
/// merge has always replayed them: the include directive's
/// `env.note_included` and the toctree directive's `env.note_reread`.
/// They raise no records.
fn doctree_read_event(
    out: &ReadOutcome,
    env: &mut BuildEnvironment,
    docname: &str,
    inputs: &MergeInputs<'_>,
) {
    let doctree = out.doctree();
    let parse = &out.output;

    let title = env_toctree::document_title(doctree);
    // Sphinx's longtitle differs from the title only for documents carrying
    // an explicit `title` attribute, which nothing produces yet
    // (`collectors/title.py:27`).
    env.longtitles.insert(docname.to_string(), title.clone());
    env.titles.insert(docname.to_string(), title);

    env.metadata.insert(
        docname.to_string(),
        env_metadata::document_metadata(doctree),
    );

    // The files this document pulls in, which is what makes it outdated
    // when one of *them* changes: image uris from the doctree walk plus the
    // parse-recorded include targets.
    env_dependencies::process_doc(
        env,
        docname,
        doctree,
        inputs.source_dir,
        &parse.registry.dependencies,
    );

    // The docnames it textually includes (`env.note_included` replayed from
    // the parse records): the orphan check in `check_consistency` is the
    // one consumer.
    let included: BTreeSet<String> = parse.registry.included.iter().cloned().collect();
    if !included.is_empty() {
        env.included.insert(docname.to_string(), included);
    }

    let (toc, num_entries) = env_toctree::build_toc(doctree, docname);
    // Each toctree node copied into the toc is noted, in the order it was
    // copied (which is the order Sphinx notes them in).
    for toctree in env_toctree::toctree_copies(&toc) {
        env_toctree::note_toctree(env, docname, toctree);
    }
    env.tocs.insert(docname.to_string(), toc);
    env.toc_num_entries.insert(docname.to_string(), num_entries);

    // `TocTree.parse_content` calls `env.note_reread()` for every entry
    // that names a document the project does not have
    // (`directives/other.py`): such a document is re-read on every build,
    // so that the day the missing target appears its toctree takes it up —
    // and stops warning about it. `clear_doc` dropped the previous read's
    // claim, so a document that no longer has a dangling entry is no longer
    // re-read either.
    if parse.toctrees.iter().any(|toctree| {
        toctree
            .warnings
            .iter()
            .any(|warning| warning.kind == ToctreeWarningKind::MissingDocument)
    }) {
        env.reread_always.insert(docname.to_string());
    }
}

/// 850-040 `SphinxDomains`: the domains' `process_doc`, in the order
/// `_DomainsContainer._process_doc` walks them — `c, changeset, citation,
/// cpp, index, js, math, py, rst, std`, so `index` before `std` with `py`
/// in between. `PythonDomain` defines no `process_doc` hook, so its slot in
/// that walk is a no-op: the py registrations (and their duplicate
/// warnings, which are parse-time in Sphinx and interleave with std's in
/// document order) replay inside `env_std::process_doc`'s parse-time pass,
/// keyed with the domains' records until T13a keys the replays as the
/// parse's. Warning locations come from node spans and the doctree's
/// source table, not the document text.
fn sphinx_domains(
    out: &mut ReadOutcome,
    env: &mut BuildEnvironment,
    docname: &str,
    inputs: &MergeInputs<'_>,
    records: &mut Vec<LogRecord>,
) {
    let mut index_warnings = Vec::new();
    // The index domain *removes* an `index` node whose entries do not
    // validate.
    env_genindex::process_doc(
        env,
        docname,
        out.doctree_mut(),
        inputs.source_path,
        &mut index_warnings,
    );
    let mut std_warnings = Vec::new();
    env_std::process_doc(
        env,
        &env_std::DocumentSource {
            docname,
            doctree: out.doctree(),
            registry: &out.output.registry,
            path: inputs.source_path,
        },
        inputs.doc2path,
        &mut std_warnings,
    );
    for warning in index_warnings.into_iter().chain(std_warnings) {
        records.push(LogRecord::Logger {
            key: SPHINX_DOMAINS.order_key(out.state_mut().draw_seq()),
            diagnostic: Diagnostic::from(warning),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctree::pending::PendingTransform;
    use state::PendingRecord;

    /// `"<priority>-<serial> <python name>"`, as docutils spells a
    /// transform's priority string and the probe printed each transform.
    fn spelled(schedule: &[(TransformKey, TransformId)]) -> Vec<String> {
        schedule
            .iter()
            .map(|(key, id)| format!("{:03}-{:03} {}", key.priority, key.serial, id.python_name()))
            .collect()
    }

    /// A transform's logger warning about a node ([`TransformCx::warn`]) is
    /// located the way Sphinx's `get_node_location` locates the node
    /// (`SP/sphinx/util/logging.py:554-562`), not the way docutils' reporter
    /// does: `path:line`; `path:` for a node at line 0, which is falsy
    /// there and which the reporter prints `path:0`; the path of a source
    /// the table spells relative to the project (an included file) made
    /// absolute against it; `<unknown>:line` for a line without a source;
    /// and a bare record for neither. Each record is keyed by the
    /// transform's row and the next seq.
    #[test]
    fn logger_warnings_locate_as_get_node_location() {
        let locations = [
            state::DocLoc::at(0, 3),
            state::DocLoc::at(0, 0),
            state::DocLoc::at(1, 2),
            state::DocLoc {
                source: None,
                line: Some(5),
            },
            state::DocLoc::default(),
        ];
        let mut state = DocumentState::default();
        let mut root = Node::elem(kinds::DOCUMENT, Span::ZERO);
        for loc in locations {
            let mut footnote = Node::elem(kinds::FOOTNOTE, Span::ZERO);
            let uid = state.mint_uid();
            footnote.set_uid(uid);
            state.loc_overrides.insert(uid, loc);
            root.children.push(footnote);
        }
        let doctree = Doctree {
            root,
            sources: vec!["<SRCDIR>/index.rst".to_string(), "part.rst".to_string()],
        };
        let mut tree = TxTree::from_parse(doctree, &mut state);
        let footnotes = tree.node(tree.root()).children.clone();
        let settings = ReadSettings::sphinx_defaults();
        let mut cx = TransformCx {
            tree: &mut tree,
            state: &mut state,
            settings: &settings,
            key: TransformKey::at(622, 28),
            docname: "index",
            srcdir: Path::new("<SRCDIR>"),
        };
        for footnote in footnotes {
            cx.warn(
                footnote,
                "Footnote [1] is not referenced.".to_string(),
                Category::from_parts(Some("ref"), Some("footnote")),
            );
        }
        let printed: Vec<(OrderKey, String)> = state
            .log
            .iter()
            .map(|record| match record {
                LogRecord::Logger { key, diagnostic } => (*key, diagnostic.render(true)),
                LogRecord::Reporter { .. } => panic!("a logger record was expected"),
            })
            .collect();
        let message = "WARNING: Footnote [1] is not referenced. [ref.footnote]";
        assert_eq!(
            printed,
            [
                format!("<SRCDIR>/index.rst:3: {message}"),
                format!("<SRCDIR>/index.rst:: {message}"),
                format!("<SRCDIR>/part.rst:2: {message}"),
                format!("<unknown>:5: {message}"),
                message.to_string(),
            ]
            .into_iter()
            .enumerate()
            .map(|(seq, line)| (TransformKey::at(622, 28).order_key(seq as u32), line))
            .collect::<Vec<_>>()
        );
    }

    /// The table is the 42 transforms Sphinx 9.1.0 applies to every
    /// document a default build reads, in the order it applies them
    /// ([TW Appendix A §E]; re-probed for this table by replacing docutils'
    /// `Transformer.apply_transforms` with a copy that prints each
    /// transform's priority string and
    /// `f"{cls.__module__}.{cls.__qualname__}"` before applying it). The
    /// serials are the registration order, 1 to 42, each once.
    #[test]
    fn read_transform_table_is_in_sphinx_order() {
        const SPHINX: [&str; 42] = [
            "010-018 sphinx.transforms.ApplySourceWorkaround",
            "010-019 sphinx.transforms.ExtraTranslatableNodes",
            "010-034 sphinx.transforms.i18n.PreserveTranslatableMessages",
            "020-035 sphinx.transforms.i18n.Locale",
            "025-036 sphinx.transforms.i18n.TranslationProgressTotaliser",
            "100-033 sphinx.transforms.compact_bullet_list.RefOnlyBulletListTransform",
            "210-020 sphinx.transforms.DefaultSubstitutions",
            "210-021 sphinx.transforms.MoveModuleTargets",
            "210-022 sphinx.transforms.HandleCodeBlocks",
            "210-025 sphinx.transforms.AutoNumbering",
            "210-026 sphinx.transforms.AutoIndexUpgrader",
            "220-004 docutils.transforms.references.Substitutions",
            "220-032 sphinx.transforms.ReorderConsecutiveTargetAndIndexNodes",
            "260-005 docutils.transforms.references.PropagateTargets",
            "261-023 sphinx.transforms.SortIds",
            "320-006 docutils.transforms.frontmatter.DocTitle",
            "340-008 docutils.transforms.frontmatter.DocInfo",
            "350-007 docutils.transforms.frontmatter.SectionSubTitle",
            "440-009 docutils.transforms.references.AnonymousHyperlinks",
            "460-010 docutils.transforms.references.IndirectHyperlinks",
            "500-024 sphinx.transforms.DoctestTransform",
            "500-031 sphinx.transforms.GlossarySorter",
            "619-016 sphinx.domains.citation.CitationDefinitionTransform",
            "619-017 sphinx.domains.citation.CitationReferenceTransform",
            "620-011 docutils.transforms.references.Footnotes",
            "622-028 sphinx.transforms.UnreferencedFootnotesDetector",
            "640-012 docutils.transforms.references.ExternalTargets",
            "660-013 docutils.transforms.references.InternalTargets",
            "700-015 sphinx.builders.latex.transforms.FootnoteDocnameUpdater",
            "740-003 docutils.transforms.universal.StripComments",
            "750-029 sphinx.transforms.SphinxSmartQuotes",
            "820-001 docutils.transforms.universal.Decorations",
            "830-014 docutils.transforms.misc.Transitions",
            "835-042 docutils.transforms.universal.Validate",
            "840-002 docutils.transforms.universal.ExposeInternals",
            "850-039 sphinx.transforms.references.SphinxDanglingReferences",
            "850-040 sphinx.transforms.references.SphinxDomains",
            "880-030 sphinx.transforms.DoctreeReadEvent",
            "880-041 sphinx.versioning.UIDTransform",
            "950-037 sphinx.transforms.i18n.AddTranslationClasses",
            "999-027 sphinx.transforms.FilterSystemMessages",
            "999-038 sphinx.transforms.i18n.RemoveTranslatableInline",
        ];
        assert_eq!(spelled(READ_TRANSFORMS), SPHINX);
        let mut serials: Vec<u16> = READ_TRANSFORMS.iter().map(|(key, _)| key.serial).collect();
        serials.sort_unstable();
        assert_eq!(serials, (1..=42).collect::<Vec<u16>>());
    }

    /// A pending transform runs after every static transform of its
    /// priority, and pending transforms of one priority run in parse order
    /// ([DS §2.2]): their serials follow the 42 statics', in `note_pending`
    /// order. Probed on Sphinx 9.1.0 with a document holding, in this
    /// order, `.. contents::`, `.. sectnum::`, `.. rst-class:: special`,
    /// `.. rst-class:: two` and `.. target-notes::`: the 47 transforms it
    /// applied, verbatim. A document with nothing pending runs the table.
    #[test]
    fn pending_runs_after_statics_of_equal_priority() {
        assert_eq!(schedule(&DocumentState::default()), READ_TRANSFORMS);

        let mut state = DocumentState::default();
        let parse_order = [
            PendingTransform::Contents,
            PendingTransform::SectNum,
            PendingTransform::ClassAttribute,
            PendingTransform::ClassAttribute,
            PendingTransform::TargetNotes,
        ];
        for (order, transform) in (0..).zip(parse_order) {
            let node = state.mint_uid();
            state.pending.push(PendingRecord {
                transform,
                order,
                node,
            });
        }
        assert_eq!(
            spelled(&schedule(&state)),
            [
                "010-018 sphinx.transforms.ApplySourceWorkaround",
                "010-019 sphinx.transforms.ExtraTranslatableNodes",
                "010-034 sphinx.transforms.i18n.PreserveTranslatableMessages",
                "020-035 sphinx.transforms.i18n.Locale",
                "025-036 sphinx.transforms.i18n.TranslationProgressTotaliser",
                "100-033 sphinx.transforms.compact_bullet_list.RefOnlyBulletListTransform",
                "210-020 sphinx.transforms.DefaultSubstitutions",
                "210-021 sphinx.transforms.MoveModuleTargets",
                "210-022 sphinx.transforms.HandleCodeBlocks",
                "210-025 sphinx.transforms.AutoNumbering",
                "210-026 sphinx.transforms.AutoIndexUpgrader",
                "210-045 docutils.transforms.misc.ClassAttribute",
                "210-046 docutils.transforms.misc.ClassAttribute",
                "220-004 docutils.transforms.references.Substitutions",
                "220-032 sphinx.transforms.ReorderConsecutiveTargetAndIndexNodes",
                "260-005 docutils.transforms.references.PropagateTargets",
                "261-023 sphinx.transforms.SortIds",
                "320-006 docutils.transforms.frontmatter.DocTitle",
                "340-008 docutils.transforms.frontmatter.DocInfo",
                "350-007 docutils.transforms.frontmatter.SectionSubTitle",
                "440-009 docutils.transforms.references.AnonymousHyperlinks",
                "460-010 docutils.transforms.references.IndirectHyperlinks",
                "500-024 sphinx.transforms.DoctestTransform",
                "500-031 sphinx.transforms.GlossarySorter",
                "540-047 docutils.transforms.references.TargetNotes",
                "619-016 sphinx.domains.citation.CitationDefinitionTransform",
                "619-017 sphinx.domains.citation.CitationReferenceTransform",
                "620-011 docutils.transforms.references.Footnotes",
                "622-028 sphinx.transforms.UnreferencedFootnotesDetector",
                "640-012 docutils.transforms.references.ExternalTargets",
                "660-013 docutils.transforms.references.InternalTargets",
                "700-015 sphinx.builders.latex.transforms.FootnoteDocnameUpdater",
                "710-044 docutils.transforms.parts.SectNum",
                "720-043 docutils.transforms.parts.Contents",
                "740-003 docutils.transforms.universal.StripComments",
                "750-029 sphinx.transforms.SphinxSmartQuotes",
                "820-001 docutils.transforms.universal.Decorations",
                "830-014 docutils.transforms.misc.Transitions",
                "835-042 docutils.transforms.universal.Validate",
                "840-002 docutils.transforms.universal.ExposeInternals",
                "850-039 sphinx.transforms.references.SphinxDanglingReferences",
                "850-040 sphinx.transforms.references.SphinxDomains",
                "880-030 sphinx.transforms.DoctreeReadEvent",
                "880-041 sphinx.versioning.UIDTransform",
                "950-037 sphinx.transforms.i18n.AddTranslationClasses",
                "999-027 sphinx.transforms.FilterSystemMessages",
                "999-038 sphinx.transforms.i18n.RemoveTranslatableInline",
            ]
        );
    }
}
