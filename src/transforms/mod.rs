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

pub(crate) mod misc;

use std::collections::BTreeMap;

use crate::config::{BuildConfig, SmartquotesExcludes};
use crate::doctree::ids::{fully_normalize_name, IdRegistry};
use crate::doctree::{kinds, AttrValue, Doctree, Node};
use crate::rst::diagnostics::{Diagnostic, Reporter};
use crate::rst::ParseOptions;

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
/// Paths address the tree as it was when the lists were collected. A
/// transform that inserts, removes or moves nodes invalidates them for
/// every transform after it, which must then re-collect
/// (`ctx.lists = DocumentLists::collect(&ctx.tree.root)`) before reading
/// them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentLists {
    /// `document.ids`: each id to the node carrying it — the first, like
    /// `set_id`'s `self.ids.setdefault(id, node)` (`nodes.py:1834-1835`).
    pub ids: BTreeMap<String, NodePath>,
    /// `document.refnames`: each `refname` to the nodes referencing it —
    /// references, footnote and citation references, and named indirect
    /// targets (`note_refname`, `nodes.py:2009-2018,2043-2054`).
    pub refnames: BTreeMap<String, Vec<NodePath>>,
    /// `document.refids`. The parse never calls `note_refid` — only the
    /// reference transforms do (`transforms/references.py:95,159,...`) —
    /// so the walk leaves it empty.
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
        lists.visit(root, &mut Vec::new());
        lists
    }

    fn visit(&mut self, node: &Node, path: &mut NodePath) {
        self.note(node, path);
        for (index, child) in node.children.iter().enumerate() {
            path.push(index);
            self.visit(child, path);
            path.pop();
        }
    }

    fn note(&mut self, node: &Node, path: &[usize]) {
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
            }
            kinds::TARGET => {
                if let Some(refname) = refname {
                    self.indirect_targets.push(path.to_vec());
                    if !node.attrs.names.is_empty() {
                        push(&mut self.refnames, refname);
                    }
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
    /// The `document`'s node lists, rebuilt by one walk when the pass
    /// starts (see [`DocumentLists`] for when to re-collect them).
    pub lists: DocumentLists,
    /// `self.config`.
    pub config: &'a TransformConfig,
    /// `self.env.docname`: the document being read.
    pub docname: &'a str,
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
    // 210 DefaultSubstitutions (Task 8), MoveModuleTargets (Task 7),
    //     HandleCodeBlocks (Task 12), AutoNumbering (Task 12),
    //     AutoIndexUpgrader (never fires for core directives).
    // 220 Substitutions (Task 8), ReorderConsecutiveTargetAndIndexNodes
    //     (Task 7).
    // 260 PropagateTargets, 261 SortIds (Task 7).
    // 320 DocTitle, 350 SectionSubTitle: disabled by Sphinx's settings
    //     (`doctitle_xform=False`, `sectsubtitle_xform=False`).
    // 340 DocInfo (Task 11).
    // 440 AnonymousHyperlinks, 460 IndirectHyperlinks (Task 9).
    // 500 DoctestTransform (Task 12); GlossarySorter: applied by the
    //     parser's `glossary` directive.
    // 619 CitationDefinitionTransform, CitationReferenceTransform;
    // 620 Footnotes; 622 UnreferencedFootnotesDetector (Task 10).
    // 640 ExternalTargets, 660 InternalTargets (Task 9).
    // 700 FootnoteDocnameUpdater (Task 10).
    // 740 StripComments: no-op (`strip_comments` unset).
    // 750 SphinxSmartQuotes (Task 14).
    // 820 Decorations: no-op (no generator/datestamp/source link).
    // 830 Transitions (Task 12).
    // 835 Validate, 840 ExposeInternals: no-ops.
    // 850 SphinxDanglingReferences (Task 9); SphinxDomains: the merge
    //     phase's domain hooks (`src/builder.rs`), after this pass.
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
        docname: &'a str,
        config: &'a TransformConfig,
    ) -> Self {
        let lists = DocumentLists::collect(&tree.root);
        TransformCtx {
            tree,
            ids,
            lists,
            config,
            docname,
            reporter: Reporter::continuing_from(next_seq),
        }
    }

    fn run(&mut self, table: &[ReadTransform]) {
        for (_, _, transform) in table {
            transform(self);
        }
    }

    /// The records the transforms made, in `seq` order.
    fn finish(self) -> Vec<Diagnostic> {
        self.reporter.take()
    }
}

/// Run the read transforms over one parsed document, in Sphinx's order:
/// `tree` is rewritten in place, `ids` is the parse's registry
/// ([`crate::rst::ParseOutput::ids`]) and `next_seq` its diagnostics
/// counter ([`crate::rst::ParseOutput::next_seq`]), both continued; every
/// record a transform makes is appended to `diagnostics` — the document's
/// stream ([`crate::rst::RegistryExport::diagnostics`]) — numbered from
/// `next_seq` on.
///
/// `next_seq` is handed over separately because the recorded diagnostics
/// alone cannot say where the parse's numbering stopped: a registration
/// (`py_objects`, `std_objects`, `glossary_terms`) can spend the last
/// number without recording anything.
pub fn apply_read_transforms(
    tree: &mut Doctree,
    ids: IdRegistry,
    next_seq: u32,
    docname: &str,
    config: &TransformConfig,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut ctx = TransformCtx::new(tree, ids, next_seq, docname, config);
    ctx.run(READ_TRANSFORMS);
    diagnostics.extend(ctx.finish());
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
    let mut diagnostics = std::mem::take(&mut out.registry.diagnostics);
    apply_read_transforms(
        &mut out.doctree,
        out.ids,
        out.next_seq,
        &opts.docname,
        config,
        &mut diagnostics,
    );
    (out.doctree, diagnostics)
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
        let mut ctx = TransformCtx::new(&mut tree, out.ids, out.next_seq, "index", &config);
        ctx.run(&[
            (100, "ReportOne", report_one as fn(&mut TransformCtx)),
            (200, "LogOne", log_one),
        ]);
        let records: Vec<(u32, DiagnosticChannel, String)> = ctx
            .finish()
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
