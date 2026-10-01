//! The read-transform pass: what Sphinx's read phase does to a document
//! between parsing it and pickling it.
//!
//! Sphinx builds a document's doctree in `_parse_str_to_doctree`
//! (`sphinx/util/docutils.py:847-908`): it queues docutils' standalone
//! reader transforms (minus `DanglingReferences`, `:80-84`), the registry's
//! transforms and the RST parser's, parses, and then runs
//! `transformer.apply_transforms()` (`:906`) — every queued transform in
//! priority order, equal priorities in the order they were queued
//! (`docutils/transforms/__init__.py:141-150,177-193`). What Sphinx pickles,
//! and every later phase reads, is the tree those transforms leave.
//! `READ_TRANSFORMS` is that order, as probed from
//! `document.transformer.applied` (research
//! `docs/superpowers/research/2026-09-30-m2-wave5-transforms.md` §1.2), and
//! [`apply_read_transforms`] runs it over one parsed document.
//!
//! The pass is kept apart from the parser on purpose: [`crate::rst`] stays
//! the transform-free docutils parse layer the docutils oracle
//! (`tests/doctree_differential.rs`) pins, and only sphinx mode runs this
//! pass after it — in the build's parallel read phase
//! ([`crate::parser::Parser::parse_full`], before the merge phase's domain
//! hooks and before the doctree is persisted) and in the Sphinx oracle's
//! harness ([`parse_and_transform`]).
//!
//! Adding a transform is registering a `fn(&mut TransformCtx)` in
//! `READ_TRANSFORMS` at its Sphinx priority, in the slot the probed order
//! gives it. It reads and rewrites [`TransformCtx::tree`], allocates ids
//! from [`TransformCtx::ids`], and records what it prints through
//! [`TransformCtx::reporter`].

mod dates;
pub(crate) mod footnotes;
pub(crate) mod misc;
pub(crate) mod references;

use std::collections::BTreeMap;

use crate::config::{BuildConfig, SmartquotesExcludes};
use crate::doctree::ids::{fully_normalize_name, IdRegistry};
use crate::doctree::{kinds, messages, AttrValue, Doctree, Node};
use crate::rst::diagnostics::{Diagnostic, Reporter};
use crate::rst::{CitationRecord, ParseOptions, RegistryExport};

/// The configuration the read transforms consult — the slice of Sphinx's
/// `self.config` they read, with Sphinx 9.1's defaults (`config.py`).
#[derive(Debug, Clone, PartialEq)]
pub struct TransformConfig {
    /// `smartquotes` (`config.py:289`): SmartQuotes' master switch.
    pub smartquotes: bool,
    /// `smartquotes_action` (`config.py:290`): which substitutions it makes.
    pub smartquotes_action: String,
    /// `smartquotes_excludes` (`config.py:291-295`).
    pub smartquotes_excludes: SmartquotesExcludes,
    /// `keep_warnings` (`config.py:261`): the level FilterSystemMessages
    /// filters below — 2 when on, 5 (every message) when off
    /// (`transforms/__init__.py:343`).
    pub keep_warnings: bool,
    /// `language` (`config.py:230`), which picks SmartQuotes' quote set.
    pub language: String,
    /// `version` (`config.py:225`), the `|version|` default substitution.
    pub version: String,
    /// `release` (`config.py:226`), the `|release|` default substitution.
    pub release: String,
    /// `today` (`config.py:227`): `|today|`'s literal text; empty means
    /// "format the build date with [`Self::today_fmt`]".
    pub today: String,
    /// `today_fmt` (`config.py:228-229`); `None` means `'%b %d, %Y'`
    /// (`transforms/__init__.py:134`).
    pub today_fmt: Option<String>,
    /// The date `|today|` formats when `today` is empty.
    pub build_date: BuildDate,
}

/// Where `|today|` takes the build date from when `today` is empty — the
/// `date=None` branch of Sphinx's `format_date` (`sphinx/util/i18n.py:
/// 270-280`), which DefaultSubstitutions calls with no date
/// (`sphinx/transforms/__init__.py:135`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BuildDate {
    /// What a build uses, and what Sphinx does: `$SOURCE_DATE_EPOCH` when it
    /// is set, else the current time, read when a document substitutes
    /// `|today|`. Always UTC — Sphinx's `local_time` stays `False` here.
    #[default]
    Environment,
    /// A fixed instant, in whole seconds since the Unix epoch (UTC): the
    /// `SOURCE_DATE_EPOCH` a test or an oracle harness pins, without the
    /// process environment.
    Epoch(i64),
}

impl Default for TransformConfig {
    /// Sphinx 9.1's defaults (`config.py:225-295`).
    fn default() -> Self {
        TransformConfig {
            smartquotes: true,
            smartquotes_action: "qDe".to_string(),
            smartquotes_excludes: SmartquotesExcludes::default(),
            keep_warnings: false,
            language: "en".to_string(),
            version: String::new(),
            release: String::new(),
            today: String::new(),
            today_fmt: None,
            build_date: BuildDate::Environment,
        }
    }
}

impl From<&BuildConfig> for TransformConfig {
    /// The build configuration's values. An unset `version`/`release` is
    /// upstream's `''` default, and an unset `language` its `'en'` — which
    /// is also what Sphinx makes of a conf.py `language = None`
    /// (`config.py:569-581`).
    fn from(config: &BuildConfig) -> Self {
        TransformConfig {
            smartquotes: config.smartquotes,
            smartquotes_action: config.smartquotes_action.clone(),
            smartquotes_excludes: config.smartquotes_excludes.clone(),
            keep_warnings: config.keep_warnings,
            language: config.language.clone().unwrap_or_else(|| "en".to_string()),
            version: config.version.clone().unwrap_or_default(),
            release: config.release.clone().unwrap_or_default(),
            today: config.today.clone(),
            today_fmt: config.today_fmt.clone(),
            build_date: BuildDate::Environment,
        }
    }
}

/// A node's address in the doctree: the child index at each step down from
/// the root (`[]` is the root itself). Paths compare in document order —
/// a parent sorts before its descendants, siblings by position.
pub type NodePath = Vec<usize>;

/// The node at `path` below `root`, if the tree still has one there.
pub fn node_at<'n>(root: &'n Node, path: &[usize]) -> Option<&'n Node> {
    path.iter()
        .try_fold(root, |node, &index| node.children.get(index))
}

/// [`node_at`], for a transform that rewrites the node it finds.
pub(crate) fn node_at_mut<'n>(root: &'n mut Node, path: &[usize]) -> Option<&'n mut Node> {
    path.iter()
        .try_fold(root, |node, &index| node.children.get_mut(index))
}

/// Hand every node of the tree under `root`, `root` included, to `visit` —
/// a node before its children, which are visited as `visit` left them —
/// by an explicit stack, so no depth of nesting overflows the call stack.
/// Siblings are visited last first: for the transforms that only rewrite
/// each node in place (or its children), order is no matter.
pub(crate) fn for_each_node_mut(root: &mut Node, mut visit: impl FnMut(&mut Node)) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        visit(node);
        stack.extend(node.children.iter_mut());
    }
}

/// The docutils `document`'s node lists (`docutils/nodes.py:1734-1786`) as
/// the parse leaves them — filled there by the `note_*` calls the RST
/// parser makes as it creates each node (`parsers/rst/states.py`,
/// `nodes.py:1996-2078`), rebuilt here by one walk of the parsed tree.
///
/// Every node is held by its [`NodePath`], and every list — each dict
/// value included — is in document order. A dict iterates its keys in
/// [`BTreeMap`] order, not docutils' insertion order; a port that needs the
/// latter sorts the keys by their first path. The `document.nameids`/
/// `nametypes` tables and the id set live in the continued [`IdRegistry`]
/// instead.
///
/// Paths address the tree as it was when the lists were collected, and a
/// stale one does not go dead: after a node before it is removed it names
/// whatever moved into its slot. A transform therefore reads them through
/// [`TransformCtx::lists`], which never hands it lists collected before an
/// earlier transform ran.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentLists {
    /// `document.ids`: each id to the node carrying it — the first, like
    /// `set_id`'s `self.ids.setdefault(id, node)` (`nodes.py:1834-1835`).
    pub ids: BTreeMap<String, NodePath>,
    /// `document.refnames`: each `refname` to the nodes referencing it —
    /// references, footnote and citation references, and named indirect
    /// targets (`note_refname`, `nodes.py:2009-2018,2043-2054`).
    pub refnames: BTreeMap<String, Vec<NodePath>>,
    /// `document.refids`: each `refid` to the nodes pointing at it. The
    /// parse never calls `note_refid`; PropagateTargets (260) does, for
    /// every target it points at its next node (`transforms/
    /// references.py:95`), and AnonymousHyperlinks (440), for every
    /// anonymous reference it gives a `refid` (`:159`) — the two lists
    /// IndirectHyperlinks (460) reads (`:285,325`). So the walk notes each
    /// target with a `refid` (before 260 only the crate's parse-time `math`
    /// label targets, already in their post-propagation shape), then each
    /// `reference` with one: under every id, the targets in document order
    /// first, then the references — the order docutils noted them in.
    /// (IndirectHyperlinks notes the targets and references it resolves
    /// itself, `:255,259,319`, on its own lists; the transforms after it
    /// read no `refids`.)
    pub refids: BTreeMap<String, Vec<NodePath>>,
    /// `document.indirect_targets`: every target with a `refname`
    /// (`note_indirect_target`, `states.py:977,2086`).
    pub indirect_targets: Vec<NodePath>,
    /// `document.substitution_defs`: each definition name to its node; a
    /// duplicate definition has been dupnamed away by the parse, so the
    /// last named one is kept, as `note_substitution_def` keeps it
    /// (`nodes.py:2056-2073`).
    pub substitution_defs: BTreeMap<String, NodePath>,
    /// `document.substitution_names`: the case-insensitive
    /// (`fully_normalize_name`) spelling of each definition name to the
    /// name.
    pub substitution_names: BTreeMap<String, String>,
    /// `document.footnote_refs`: each `refname` to its footnote references
    /// (`note_footnote_ref`, `nodes.py:2043-2046`).
    pub footnote_refs: BTreeMap<String, Vec<NodePath>>,
    /// `document.citation_refs` (`note_citation_ref`, `nodes.py:2051-2054`).
    /// Empty from 619 on, which replaces every one in the tree; docutils'
    /// list, which keeps them, is [`TransformCtx::replaced_citation_refs`].
    pub citation_refs: BTreeMap<String, Vec<NodePath>>,
    /// `document.autofootnotes`: the `auto=1` footnotes (`[#]`, `[#label]`).
    pub autofootnotes: Vec<NodePath>,
    /// `document.autofootnote_refs`: the `auto=1` footnote references.
    pub autofootnote_refs: Vec<NodePath>,
    /// `document.symbol_footnotes`: the `auto='*'` footnotes (`[*]`).
    pub symbol_footnotes: Vec<NodePath>,
    /// `document.symbol_footnote_refs`.
    pub symbol_footnote_refs: Vec<NodePath>,
    /// `document.footnotes`: the manually numbered footnotes.
    pub footnotes: Vec<NodePath>,
    /// `document.citations`.
    pub citations: Vec<NodePath>,
}

impl DocumentLists {
    /// One pre-order walk of `root`, noting each node the way the parse's
    /// `note_*` call for it did (`states.py:1061-1077` for footnote and
    /// citation references, `:2013-2047` for footnotes and citations,
    /// `:2179-2215` for substitution definitions).
    pub fn collect(root: &Node) -> DocumentLists {
        let mut lists = DocumentLists::default();
        let mut reference_refids = BTreeMap::new();
        lists.visit(root, &mut Vec::new(), &mut reference_refids);
        for (refid, paths) in reference_refids {
            lists.refids.entry(refid).or_default().extend(paths);
        }
        lists
    }

    fn visit(
        &mut self,
        node: &Node,
        path: &mut NodePath,
        reference_refids: &mut BTreeMap<String, Vec<NodePath>>,
    ) {
        self.note(node, path, reference_refids);
        for (index, child) in node.children.iter().enumerate() {
            path.push(index);
            self.visit(child, path, reference_refids);
            path.pop();
        }
    }

    fn note(
        &mut self,
        node: &Node,
        path: &[usize],
        reference_refids: &mut BTreeMap<String, Vec<NodePath>>,
    ) {
        for id in &node.attrs.ids {
            self.ids.entry(id.clone()).or_insert_with(|| path.to_vec());
        }
        let refname = match node.get("refname") {
            Some(AttrValue::Str(refname)) => Some(refname),
            _ => None,
        };
        let auto = node.get("auto");
        let push = |map: &mut BTreeMap<String, Vec<NodePath>>, key: &str| {
            map.entry(key.to_string()).or_default().push(path.to_vec());
        };
        match node.kind {
            kinds::REFERENCE => {
                if let Some(refname) = refname {
                    push(&mut self.refnames, refname);
                }
                if let Some(AttrValue::Str(refid)) = node.get("refid") {
                    push(reference_refids, refid);
                }
            }
            kinds::TARGET => {
                if let Some(refname) = refname {
                    self.indirect_targets.push(path.to_vec());
                    if !node.attrs.names.is_empty() {
                        push(&mut self.refnames, refname);
                    }
                }
                if let Some(AttrValue::Str(refid)) = node.get("refid") {
                    push(&mut self.refids, refid);
                }
            }
            kinds::FOOTNOTE_REFERENCE => {
                match auto {
                    Some(AttrValue::Int(1)) => self.autofootnote_refs.push(path.to_vec()),
                    Some(AttrValue::Str(symbol)) if symbol == "*" => {
                        self.symbol_footnote_refs.push(path.to_vec())
                    }
                    _ => {}
                }
                if let Some(refname) = refname {
                    push(&mut self.footnote_refs, refname);
                    push(&mut self.refnames, refname);
                }
            }
            kinds::CITATION_REFERENCE => {
                if let Some(refname) = refname {
                    push(&mut self.citation_refs, refname);
                    push(&mut self.refnames, refname);
                }
            }
            kinds::FOOTNOTE => match auto {
                Some(AttrValue::Int(1)) => self.autofootnotes.push(path.to_vec()),
                Some(AttrValue::Str(symbol)) if symbol == "*" => {
                    self.symbol_footnotes.push(path.to_vec())
                }
                _ => self.footnotes.push(path.to_vec()),
            },
            kinds::CITATION => self.citations.push(path.to_vec()),
            "substitution_definition" => {
                if let Some(name) = node.attrs.names.first() {
                    self.substitution_defs.insert(name.clone(), path.to_vec());
                    self.substitution_names
                        .insert(fully_normalize_name(name), name.clone());
                }
            }
            _ => {}
        }
    }
}

/// Everything a read transform works with — the analogue of the
/// `self.document`, `self.config` and `self.env` a Sphinx transform reads.
pub struct TransformCtx<'a> {
    /// `self.document`: the doctree, rewritten in place.
    pub tree: &'a mut Doctree,
    /// The parser's id/name registry (`document.ids`/`nameids`/
    /// `nametypes`/`id_counter`), continued rather than re-derived: an id
    /// a transform allocates (`document.set_id`, AutoNumbering's
    /// `note_implicit_target`) must come after every id the parse handed
    /// out, exactly as it does on the one `document` docutils keeps.
    pub ids: IdRegistry,
    /// The `document`'s node lists for the tree as the running transform
    /// found it, collected on first use; read through [`Self::lists`].
    /// `None` until then, and again after every transform.
    lists: Option<DocumentLists>,
    /// `self.config`.
    pub config: &'a TransformConfig,
    /// `self.env.docname`: the document being read.
    pub docname: &'a str,
    /// Where a message with no node is located
    /// ([`crate::rst::ParseOutput::end_of_input`];
    /// [`Self::end_of_parse_message`]).
    end_of_input: Option<(u16, u32)>,
    /// The diagnostics sink — docutils' `document.reporter` and Sphinx's
    /// logger in one stream, continuing the parse's numbering
    /// ([`Reporter::continuing_from`]): every transform in Sphinx runs
    /// after the parse, so its records follow the parse's. The records
    /// reach the build's printed stream through the document's own
    /// [`crate::rst::RegistryExport::diagnostics`], which the merge phase
    /// prints in `seq` order ahead of the domains' `process_doc` records —
    /// the place of every read transform that prints: each one runs ahead
    /// of `SphinxDomains` in the probed order (850-040; the last printer,
    /// `SphinxDanglingReferences`, is 850-039 — research §1.2).
    pub reporter: Reporter,
    /// The registrations the transforms make with the environment, which
    /// leave the pass beside its records
    /// ([`crate::rst::RegistryExport::citations`]): CitationDefinitionTransform's
    /// `note_citation` calls (619), each holding the `seq` it spent — where
    /// its duplicate warning, if the merge phase's replay against the
    /// environment finds one ([`crate::env::citation_domain`]), prints.
    pub(crate) citations: Vec<CitationRecord>,
    /// `document.citation_refs` from 619 on: the `citation_reference`s
    /// CitationReferenceTransform took out of the tree (replaced by
    /// `pending_xref`s, [`footnotes::citation_references`]), which
    /// docutils' list still holds and Footnotes (620) still links to their
    /// citations — each `refname` to the first id of each reference no
    /// earlier transform resolved, in document order. The walk-built lists
    /// ([`DocumentLists`]) cannot see nodes that have left the tree.
    pub(crate) replaced_citation_refs: BTreeMap<String, Vec<String>>,
}

/// One [`READ_TRANSFORMS`] entry: Sphinx priority, upstream class name,
/// and the port.
type ReadTransform = (u16, &'static str, fn(&mut TransformCtx));

/// The read transforms, in the order Sphinx 9.1 applies them to an HTML
/// build's document (probed, `extensions=[]`; research §1.2 lists each
/// with its `priority-serial` and effect). Only the transforms this crate
/// runs are entries; the rest are named where they run, with why they are
/// not (yet) here.
static READ_TRANSFORMS: &[ReadTransform] = &[
    // 010 ApplySourceWorkaround: source/line patches only — the parser
    //     stamps its spans directly.
    // 010 ExtraTranslatableNodes: no-op without `gettext_additional_targets`.
    // 010 PreserveTranslatableMessages: toctree `rawentries`/`rawcaption` —
    //     a parse-time stamp (Task 12).
    // 020 Locale, 025 TranslationProgressTotaliser: no-op without message
    //     catalogs / the `translation_progress` attribute no oracle compares.
    // 100 RefOnlyBulletListTransform: no-op under `html_compact_lists=True`.
    (
        210,
        "DefaultSubstitutions",
        references::default_substitutions,
    ),
    (210, "MoveModuleTargets", misc::move_module_targets),
    // 210 HandleCodeBlocks (Task 12), AutoNumbering (Task 12),
    //     AutoIndexUpgrader (never fires for core directives).
    (220, "Substitutions", references::substitutions),
    (
        220,
        "ReorderConsecutiveTargetAndIndexNodes",
        misc::reorder_consecutive_target_and_index_nodes,
    ),
    (260, "PropagateTargets", references::propagate_targets),
    (261, "SortIds", misc::sort_ids),
    // 320 DocTitle, 350 SectionSubTitle: disabled by Sphinx's settings
    //     (`doctitle_xform=False`, `sectsubtitle_xform=False`).
    // 340 DocInfo (Task 11).
    (440, "AnonymousHyperlinks", references::anonymous_hyperlinks),
    (460, "IndirectHyperlinks", references::indirect_hyperlinks),
    // 500 DoctestTransform (Task 12); GlossarySorter: applied by the
    //     parser's `glossary` directive.
    (
        619,
        "CitationDefinitionTransform",
        footnotes::citation_definitions,
    ),
    (
        619,
        "CitationReferenceTransform",
        footnotes::citation_references,
    ),
    (620, "Footnotes", footnotes::footnotes),
    (
        622,
        "UnreferencedFootnotesDetector",
        footnotes::unreferenced_footnotes,
    ),
    (640, "ExternalTargets", references::external_targets),
    (660, "InternalTargets", references::internal_targets),
    (700, "FootnoteDocnameUpdater", footnotes::footnote_docnames),
    // 740 StripComments: no-op (`strip_comments` unset).
    // 750 SphinxSmartQuotes (Task 14).
    // 820 Decorations: no-op (no generator/datestamp/source link).
    // 830 Transitions (Task 12).
    // 835 Validate, 840 ExposeInternals: no-ops.
    (
        850,
        "SphinxDanglingReferences",
        references::dangling_references,
    ),
    // 850 SphinxDomains: the merge phase's domain hooks
    //     (`src/builder.rs`), after this pass.
    // 880 DoctreeReadEvent: the merge phase's environment collectors;
    //     UIDTransform: no-op for these builders.
    // 950 AddTranslationClasses: no-op by default.
    (999, "FilterSystemMessages", misc::filter_system_messages),
    // 999 RemoveTranslatableInline: its effect is the parser's docfield and
    //     versionmodified output.
];

impl<'a> TransformCtx<'a> {
    fn new(
        tree: &'a mut Doctree,
        ids: IdRegistry,
        next_seq: u32,
        end_of_input: Option<(u16, u32)>,
        docname: &'a str,
        config: &'a TransformConfig,
    ) -> Self {
        TransformCtx {
            tree,
            ids,
            lists: None,
            config,
            docname,
            end_of_input,
            reporter: Reporter::continuing_from(next_seq),
            citations: Vec::new(),
            replaced_citation_refs: BTreeMap::new(),
        }
    }

    /// The `system_message` docutils' reporter makes at `line` of source-
    /// table entry `source`, its `source` attribute naming that entry's path
    /// — or, with no line, only the document's (`Reporter.system_message`,
    /// `docutils/utils/__init__.py:187-207`: `source` falls back to the
    /// reporter's own). Neither recorded nor placed: the transform does both.
    pub(crate) fn message(&self, level: u8, text: &str, source: u16, line: Option<u32>) -> Node {
        let source = if line.is_some() { source } else { 0 };
        let path = self
            .tree
            .sources
            .get(usize::from(source))
            .or_else(|| self.tree.sources.first())
            .map_or("<document>", String::as_str);
        let mut message =
            messages::system_message(level, text, source, line.unwrap_or_default(), path);
        if line.is_none() {
            message.attrs.extra.retain(|(key, _)| *key != "line");
        }
        message
    }

    /// [`Self::message`] for a message a transform raises with no node to
    /// locate it by: docutils asks the finished parse where it is
    /// ([`crate::rst::ParseOutput::end_of_input`], research §9.3).
    pub(crate) fn end_of_parse_message(&self, level: u8, text: &str) -> Node {
        match self.end_of_input {
            Some((source, line)) => self.message(level, text, source, Some(line)),
            None => self.message(level, text, 0, None),
        }
    }

    /// The `document`'s node lists ([`DocumentLists`]) for the tree as the
    /// running transform found it: one walk the first time the transform
    /// asks, shared by its later calls. Lists collected for an earlier
    /// transform are never handed on — the pass drops them after each
    /// transform, whatever it did to the tree — so no transform needs to
    /// know what the ones before it changed. A transform that itself
    /// restructures the tree and then needs paths into the result walks
    /// again ([`DocumentLists::collect`]).
    pub fn lists(&mut self) -> &DocumentLists {
        self.tree_and_lists().1
    }

    /// [`Self::lists`] beside the tree, for a transform that reads both at
    /// once — the lists still address the tree as the transform found it,
    /// so it changes the tree's structure only after its last read.
    pub(crate) fn tree_and_lists(&mut self) -> (&mut Doctree, &DocumentLists) {
        let tree = &mut *self.tree;
        let lists = self
            .lists
            .get_or_insert_with(|| DocumentLists::collect(&tree.root));
        (tree, lists)
    }

    fn run(&mut self, table: &[ReadTransform]) {
        for (_, _, transform) in table {
            transform(self);
            self.lists = None;
        }
    }

    /// The records the transforms made, in `seq` order, and the
    /// registrations they made, in the order they made them.
    fn finish(self) -> (Vec<Diagnostic>, Vec<CitationRecord>) {
        (self.reporter.take(), self.citations)
    }
}

/// Run the read transforms over one parsed document, in Sphinx's order:
/// `tree` is rewritten in place, `ids` is the parse's registry
/// ([`crate::rst::ParseOutput::ids`]) and `next_seq` its diagnostics
/// counter ([`crate::rst::ParseOutput::next_seq`]), both continued;
/// `end_of_input` is where the parse's input ended
/// ([`crate::rst::ParseOutput::end_of_input`]). `registry` is the parse's
/// export: every record a transform makes is appended to its
/// `diagnostics` — the document's stream — numbered from `next_seq` on,
/// and every registration a transform makes with the environment to its
/// own list ([`RegistryExport::citations`]), for the merge phase to replay.
///
/// The parse's `nameids` snapshot in `registry` is not updated: the one
/// name a transform registers is the number Footnotes (620) gives an
/// unlabelled auto-numbered footnote (`note_explicit_target`,
/// `docutils/transforms/references.py:530-532`), which the snapshot's only
/// reader — `StandardDomain.process_doc`, the labels — skips with every
/// other footnote name (`sphinx/domains/std/__init__.py:951-958`). The
/// continued `ids` registry, which the transforms after Footnotes resolve
/// against, does hold it.
///
/// `next_seq` is handed over separately because the recorded diagnostics
/// alone cannot say where the parse's numbering stopped: a registration
/// (`py_objects`, `std_objects`, `glossary_terms`) can spend the last
/// number without recording anything.
pub fn apply_read_transforms(
    tree: &mut Doctree,
    ids: IdRegistry,
    next_seq: u32,
    end_of_input: Option<(u16, u32)>,
    docname: &str,
    config: &TransformConfig,
    registry: &mut RegistryExport,
) {
    let mut ctx = TransformCtx::new(tree, ids, next_seq, end_of_input, docname, config);
    ctx.run(READ_TRANSFORMS);
    let (diagnostics, citations) = ctx.finish();
    registry.diagnostics.extend(diagnostics);
    registry.citations.extend(citations);
}

/// A standalone Sphinx read of `source`: the parse, then the read
/// transforms. Returns the transformed doctree and the document's whole
/// printed stream — the parse's records, then the transforms' — in `seq`
/// order. Registrations whose duplicate warnings the merge phase replays
/// are not in it (they need the environment).
pub fn parse_and_transform(
    source: &str,
    opts: &ParseOptions,
    config: &TransformConfig,
) -> (Doctree, Vec<Diagnostic>) {
    let mut out = crate::rst::parse_rst_full(source, opts);
    apply_read_transforms(
        &mut out.doctree,
        out.ids,
        out.next_seq,
        out.end_of_input,
        &opts.docname,
        config,
        &mut out.registry,
    );
    (out.doctree, out.registry.diagnostics)
}

/// Test support: [`crate::rst::parse_rst_full`] followed by the read pass
/// under Sphinx's default configuration, every other field of the parse
/// output kept — the doctree and registry the build's read phase
/// ([`crate::parser::Parser`]) hands the merge phase. The transforms'
/// records join `registry.diagnostics`, their registrations
/// `registry.citations`; the id registry the pass continued is spent, so
/// `ids` comes back empty.
#[cfg(test)]
pub(crate) fn parse_full_and_transform(
    source: &str,
    opts: &ParseOptions,
) -> crate::rst::ParseOutput {
    let mut out = crate::rst::parse_rst_full(source, opts);
    apply_read_transforms(
        &mut out.doctree,
        std::mem::take(&mut out.ids),
        out.next_seq,
        out.end_of_input,
        &opts.docname,
        &TransformConfig::default(),
        &mut out.registry,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BuildConfig, SmartquotesExcludes};
    use crate::doctree::{kinds, messages};
    use crate::rst::diagnostics::DiagnosticChannel;
    use crate::rst::ParseOptions;

    fn sphinx_opts() -> ParseOptions {
        ParseOptions {
            source_path: "<snippet>".to_string(),
            sphinx: true,
            ..Default::default()
        }
    }

    /// Sphinx 9.1's defaults (`config.py:225-295`, `language = 'en'` at
    /// `:230`), and what a default [`BuildConfig`] projects to.
    #[test]
    fn transform_config_defaults_are_sphinx_9_1s() {
        let expected = TransformConfig {
            smartquotes: true,
            smartquotes_action: "qDe".to_string(),
            smartquotes_excludes: SmartquotesExcludes::default(),
            keep_warnings: false,
            language: "en".to_string(),
            version: String::new(),
            release: String::new(),
            today: String::new(),
            today_fmt: None,
            build_date: BuildDate::Environment,
        };
        assert_eq!(TransformConfig::default(), expected);
        assert_eq!(TransformConfig::from(&BuildConfig::default()), expected);
    }

    /// Every key is read off the build configuration; an unset
    /// `version`/`release` reads as upstream's `''`, and `language = None`
    /// as `'en'` (Sphinx rewrites it, `config.py:569-581`).
    #[test]
    fn transform_config_reads_the_build_config() {
        let config = BuildConfig {
            smartquotes: false,
            smartquotes_action: "q".to_string(),
            smartquotes_excludes: SmartquotesExcludes {
                languages: vec!["de".to_string()],
                builders: Vec::new(),
            },
            keep_warnings: true,
            language: Some("fr".to_string()),
            version: Some("1.2".to_string()),
            release: Some("1.2.3".to_string()),
            today: "Sept 30".to_string(),
            today_fmt: Some("%Y".to_string()),
            ..BuildConfig::default()
        };
        assert_eq!(
            TransformConfig::from(&config),
            TransformConfig {
                smartquotes: false,
                smartquotes_action: "q".to_string(),
                smartquotes_excludes: SmartquotesExcludes {
                    languages: vec!["de".to_string()],
                    builders: Vec::new(),
                },
                keep_warnings: true,
                language: "fr".to_string(),
                version: "1.2".to_string(),
                release: "1.2.3".to_string(),
                today: "Sept 30".to_string(),
                today_fmt: Some("%Y".to_string()),
                build_date: BuildDate::Environment,
            }
        );
        let unset = BuildConfig {
            language: None,
            ..BuildConfig::default()
        };
        assert_eq!(TransformConfig::from(&unset).language, "en");
    }

    /// The table is Sphinx's applied order: sorted by priority, equal
    /// priorities in registration order (`docutils/transforms/
    /// __init__.py:141-150,177-193`), FilterSystemMessages last (999).
    #[test]
    fn the_table_runs_in_sphinx_priority_order() {
        let priorities: Vec<u16> = READ_TRANSFORMS.iter().map(|(p, _, _)| *p).collect();
        assert!(
            priorities.windows(2).all(|pair| pair[0] <= pair[1]),
            "out of order: {priorities:?}"
        );
        let (priority, name, _) = READ_TRANSFORMS.last().unwrap();
        assert_eq!((*priority, *name), (999, "FilterSystemMessages"));
    }

    /// The substitution and target families in their probed slots
    /// (research §1.2: 210-020, 210-021, 220-004, 220-032, 260-005,
    /// 261-023): each substitution transform ahead of its priority's
    /// neighbour, as Sphinx queues them; the reorder before PropagateTargets
    /// (`transforms/__init__.py:472`, "This transform MUST run before
    /// ``PropagateTargets``"), and SortIds after it, since it sorts the ids
    /// PropagateTargets appended.
    #[test]
    fn the_substitution_and_target_transforms_run_in_sphinx_order() {
        let order: Vec<(u16, &str)> = READ_TRANSFORMS
            .iter()
            .map(|(priority, name, _)| (*priority, *name))
            .filter(|(priority, _)| (210..=261).contains(priority))
            .collect();
        assert_eq!(
            order,
            [
                (210, "DefaultSubstitutions"),
                (210, "MoveModuleTargets"),
                (220, "Substitutions"),
                (220, "ReorderConsecutiveTargetAndIndexNodes"),
                (260, "PropagateTargets"),
                (261, "SortIds"),
            ]
        );
    }

    /// The hyperlink and footnote families in their probed slots (research
    /// §1.2: 440-009, 460-010, 619-016, 619-017, 620-011, 622-028, 640-012,
    /// 660-013, 700-015, 850-039): the anonymous pairing before the indirect
    /// targets (which rewrite the anonymous references it gave a `refid`);
    /// the citation definitions before the citation references (Sphinx
    /// registers them in that order, `sphinx/domains/citation.py:181-182`),
    /// both before Footnotes, which back-links the citations to the
    /// references they replaced, and the unreferenced-footnote check after
    /// it, which reads its backrefs; the external and internal targets after
    /// the footnotes' 620 (a reference Footnotes resolved is theirs no
    /// more), the docnames after every footnote reference is final but the
    /// dangling ones, and the dangling references last before
    /// FilterSystemMessages.
    #[test]
    fn the_hyperlink_and_footnote_transforms_run_in_sphinx_order() {
        let order: Vec<(u16, &str)> = READ_TRANSFORMS
            .iter()
            .map(|(priority, name, _)| (*priority, *name))
            .filter(|(priority, _)| *priority >= 340)
            .collect();
        assert_eq!(
            order,
            [
                (440, "AnonymousHyperlinks"),
                (460, "IndirectHyperlinks"),
                (619, "CitationDefinitionTransform"),
                (619, "CitationReferenceTransform"),
                (620, "Footnotes"),
                (622, "UnreferencedFootnotesDetector"),
                (640, "ExternalTargets"),
                (660, "InternalTargets"),
                (700, "FootnoteDocnameUpdater"),
                (850, "SphinxDanglingReferences"),
                (999, "FilterSystemMessages"),
            ]
        );
    }

    /// `document.refids` at IndirectHyperlinks (460), which reads it
    /// (`references.py:285,325`): the targets PropagateTargets (260) noted,
    /// then the anonymous references AnonymousHyperlinks (440) gave a
    /// `refid` (`note_refid(ref)`, `:159`) — the order docutils noted
    /// them in, whatever the document order: here the reference comes
    /// first in the document but last under `id1`.
    #[test]
    fn a_reference_given_a_refid_is_noted_after_the_targets() {
        let (tree, records) = parse_and_transform(
            "See `x`__.\n\n.. _t:\n.. __:\n\nPara.\n",
            &sphinx_opts(),
            &TransformConfig::default(),
        );
        assert_eq!(records, []);
        let lists = DocumentLists::collect(&tree.root);
        let expected: BTreeMap<String, Vec<NodePath>> = [
            ("id1".to_string(), vec![vec![2], vec![0, 1]]),
            ("t".to_string(), vec![vec![1]]),
        ]
        .into_iter()
        .collect();
        assert_eq!(lists.refids, expected);
        assert_eq!(
            node_at(&tree.root, &[0, 1]).and_then(|r| r.get("refid")),
            Some(&AttrValue::Str("id1".to_string()))
        );
    }

    /// `document.refids` after PropagateTargets: every target it pointed at
    /// its next node (`note_refid`, `references.py:95`) — which a transform
    /// after it reads (IndirectHyperlinks, `references.py:285,325`).
    #[test]
    fn a_propagated_target_is_noted_in_refids() {
        let (tree, _) = parse_and_transform(
            ".. _a:\n.. _b:\n\nPara.\n",
            &sphinx_opts(),
            &TransformConfig::default(),
        );
        let lists = DocumentLists::collect(&tree.root);
        let expected: BTreeMap<String, Vec<NodePath>> = [
            ("a".to_string(), vec![vec![0]]),
            ("b".to_string(), vec![vec![1]]),
        ]
        .into_iter()
        .collect();
        assert_eq!(lists.refids, expected);
        assert_eq!(lists.ids["a"], [2], "both ids now name the paragraph");
        assert_eq!(lists.ids["b"], [2]);
    }

    fn report_one(ctx: &mut TransformCtx) {
        let msg = messages::system_message(messages::ERROR, "From a transform.", 0, 1, "<snippet>");
        ctx.reporter.report(&msg);
    }

    fn log_one(ctx: &mut TransformCtx) {
        let docname = ctx.docname.to_string();
        ctx.reporter
            .log(2, Some("test.log".to_string()), docname, 0, Some(3), false);
    }

    /// A transform's record takes the document's next `seq` — past every
    /// number the parse spent, registrations included — so it prints after
    /// the whole parse stream. The py registration here takes seq 1 after
    /// the inline WARNING's 0, and is the parse's last spend: a counter
    /// restarted from the recorded diagnostics (one past 0) would collide
    /// with it.
    #[test]
    fn transform_records_continue_the_document_sequence() {
        let out = crate::rst::parse_rst_full("*x\n\n.. py:function:: f()\n", &sphinx_opts());
        let parse_seqs: Vec<u32> = out.registry.diagnostics.iter().map(|d| d.seq).collect();
        assert_eq!(parse_seqs, [0]);
        assert_eq!(out.registry.py_objects[0].seq, 1);
        assert_eq!(out.next_seq, 2);

        let mut tree = out.doctree;
        let config = TransformConfig::default();
        let mut ctx = TransformCtx::new(&mut tree, out.ids, out.next_seq, None, "index", &config);
        ctx.run(&[
            (100, "ReportOne", report_one as fn(&mut TransformCtx)),
            (200, "LogOne", log_one),
        ]);
        let records: Vec<(u32, DiagnosticChannel, String)> = ctx
            .finish()
            .0
            .into_iter()
            .map(|d| (d.seq, d.channel, d.text))
            .collect();
        assert_eq!(
            records,
            [
                (
                    2,
                    DiagnosticChannel::Reporter,
                    "From a transform.".to_string()
                ),
                (3, DiagnosticChannel::Logger, "index".to_string()),
            ]
        );
    }

    /// Reads the lists, then removes the document's first block — the shape
    /// of MoveModuleTargets (210) removing a module target
    /// (`sphinx/transforms/__init__.py:175`).
    fn note_then_remove_first_block(ctx: &mut TransformCtx) {
        assert_eq!(ctx.lists().substitution_defs["sub"], [1]);
        ctx.tree.root.children.remove(0);
    }

    /// Reads a substitution definition through the lists — the shape of
    /// Substitutions (220) after it.
    fn read_the_substitution_definition(ctx: &mut TransformCtx) {
        let path = ctx.lists().substitution_defs["sub"].clone();
        assert_eq!(
            node_at(&ctx.tree.root, &path).map(|node| node.kind),
            Some("substitution_definition"),
            "the lists handed this transform a path from before the removal: {path:?}"
        );
    }

    /// A path from before an earlier transform's structural change does
    /// not go dead, it silently names the node that moved into its slot —
    /// here the paragraph after the definition. Every transform must be
    /// handed lists of the tree as it finds it.
    #[test]
    fn the_lists_follow_the_tree_from_one_transform_to_the_next() {
        let mut tree = crate::rst::parse_rst(
            "Para.\n\n.. |sub| replace:: text\n\nAfter.\n",
            &sphinx_opts(),
        );
        let config = TransformConfig::default();
        let mut ctx = TransformCtx::new(&mut tree, IdRegistry::new(), 0, None, "index", &config);
        ctx.run(&[
            (
                210,
                "RemoveFirstBlock",
                note_then_remove_first_block as fn(&mut TransformCtx),
            ),
            (220, "ReadSubstitution", read_the_substitution_definition),
        ]);
    }

    /// The docutils `document` lists the parse left behind
    /// (`docutils/nodes.py:1734-1786`, filled by the `note_*` calls in
    /// `parsers/rst/states.py`), rebuilt by one walk of the finished tree:
    /// every node in document order, addressed by its path from the root.
    #[test]
    fn the_document_lists_are_rebuilt_by_one_walk() {
        let tree = crate::rst::parse_rst(
            "See `ref`_ and [#]_ and [*]_ and [1]_ and [#lbl]_ and [CIT]_.\n\n\
             .. _ind: ref_\n\n\
             .. [#] Auto.\n.. [*] Symbol.\n.. [1] Manual.\n.. [#lbl] Labelled.\n\
             .. [CIT] Citation.\n\n\
             .. |Sub  Name| replace:: text\n",
            &sphinx_opts(),
        );
        let lists = DocumentLists::collect(&tree.root);
        let path = |p: &[usize]| p.to_vec();
        let expected = DocumentLists {
            ids: lists.ids.clone(),
            refnames: [
                ("1".to_string(), vec![path(&[0, 7])]),
                ("cit".to_string(), vec![path(&[0, 11])]),
                ("lbl".to_string(), vec![path(&[0, 9])]),
                ("ref".to_string(), vec![path(&[0, 1]), path(&[1])]),
            ]
            .into_iter()
            .collect(),
            refids: Default::default(),
            indirect_targets: vec![path(&[1])],
            substitution_defs: [("Sub Name".to_string(), path(&[7]))].into_iter().collect(),
            substitution_names: [("sub name".to_string(), "Sub Name".to_string())]
                .into_iter()
                .collect(),
            footnote_refs: [
                ("1".to_string(), vec![path(&[0, 7])]),
                ("lbl".to_string(), vec![path(&[0, 9])]),
            ]
            .into_iter()
            .collect(),
            citation_refs: [("cit".to_string(), vec![path(&[0, 11])])]
                .into_iter()
                .collect(),
            autofootnotes: vec![path(&[2]), path(&[5])],
            autofootnote_refs: vec![path(&[0, 3]), path(&[0, 9])],
            symbol_footnotes: vec![path(&[3])],
            symbol_footnote_refs: vec![path(&[0, 5])],
            footnotes: vec![path(&[4])],
            citations: vec![path(&[6])],
        };
        assert_eq!(lists, expected);

        // `document.ids`: every id, mapped to the first node carrying it.
        let manual = node_at(&tree.root, &[4]).unwrap();
        assert_eq!(manual.kind, kinds::FOOTNOTE);
        assert_eq!(lists.ids.get(&manual.attrs.ids[0]), Some(&path(&[4])));
        assert_eq!(
            lists.ids.len(),
            count_ids(&tree.root),
            "every id is registered once"
        );
        assert_eq!(
            node_at(&tree.root, &[0, 11]).map(|n| n.kind),
            Some(kinds::CITATION_REFERENCE)
        );
        assert!(node_at(&tree.root, &[0, 99]).is_none());
    }

    fn count_ids(node: &crate::doctree::Node) -> usize {
        node.attrs.ids.len() + node.children.iter().map(count_ids).sum::<usize>()
    }
}
