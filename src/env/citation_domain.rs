//! The `citation` domain's registries — Sphinx's `CitationDomain`
//! (`sphinx/domains/citation.py:31-130`) — and the read-phase calls that
//! fill them: `note_citation` (`:70-82`) from CitationDefinitionTransform
//! and `note_citation_reference` (`:84-86`) from CitationReferenceTransform
//! (both priority 619, `:133-177`), which this crate's read pass makes in
//! the parallel read ([`crate::transforms`]) and the serial merge phase
//! replays here, against the environment — the duplicate-citation warning
//! needs every document read before this one. `check_consistency`
//! (`:88-97`) runs after the read. Resolving the `pending_xref`s
//! (`resolve_xref`, `:99-113`) is the write phase's, not yet ported.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::doctree::{kinds, AttrValue, Node};
use crate::env::std_domain::{source_path_of, DocumentSource};
use crate::env::BuildEnvironment;
use crate::error::{BuildWarning, WarningType};

/// One `citations` entry: Sphinx's `(docname, labelid, lineno)` tuple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationEntry {
    pub docname: String,
    /// The citation's first id, which `resolve_xref` links to.
    pub labelid: String,
    /// The citation's line, which `Citation [%s] is not referenced.` prints
    /// beside the document's path.
    pub lineno: u32,
}

/// `domaindata['citation']`: `citations` and `citation_refs`
/// (`citation.py:41-47`).
///
/// `citations` is INSERTION-ORDERED, like [`crate::env::PyDomainData`]:
/// `check_consistency` warns in the dict's order, and Python dict
/// assignment on an existing key keeps the original slot, so a duplicate
/// registration overwrites **in place**; `clear_doc`'s `del` and a later
/// re-registration move a label to the end. `citations_index` names each
/// label's position and carries nothing else.
///
/// Both are keyed by the label as written — `CIT` for `.. [CIT]` and
/// `cit` for a `[cit]_` reference — so a citation referenced in another
/// case is, to the domain, not referenced (probed with
/// `tx_footnotes.citation_definition_and_reference`'s spelling).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CitationDomainData {
    /// label -> entry, in registration order.
    pub citations: Vec<(String, CitationEntry)>,
    /// label -> index into [`Self::citations`].
    pub citations_index: BTreeMap<String, usize>,
    /// `reftarget` -> the documents that reference it.
    pub citation_refs: BTreeMap<String, BTreeSet<String>>,
}

impl CitationDomainData {
    /// `note_citation` (`citation.py:70-82`): registers `label`, returning
    /// the docname of the registration it replaces — `Some` exactly when
    /// Sphinx warns `duplicate citation`.
    pub fn note_citation(&mut self, label: &str, entry: CitationEntry) -> Option<String> {
        if let Some(&index) = self.citations_index.get(label) {
            let previous = std::mem::replace(&mut self.citations[index].1, entry);
            return Some(previous.docname);
        }
        self.citations_index
            .insert(label.to_string(), self.citations.len());
        self.citations.push((label.to_string(), entry));
        None
    }

    /// `note_citation_reference` (`citation.py:84-86`).
    pub fn note_citation_reference(&mut self, reftarget: &str, docname: &str) {
        self.citation_refs
            .entry(reftarget.to_string())
            .or_default()
            .insert(docname.to_string());
    }

    /// `CitationDomain.clear_doc` (`citation.py:49-57`): the document's
    /// citations go, and it stops referencing anything (a label nobody
    /// references any more goes too).
    pub fn clear_doc(&mut self, docname: &str) {
        self.citations.retain(|(_, entry)| entry.docname != docname);
        self.citations_index = self
            .citations
            .iter()
            .enumerate()
            .map(|(index, (label, _))| (label.clone(), index))
            .collect();
        self.citation_refs.retain(|_, docnames| {
            docnames.remove(docname);
            !docnames.is_empty()
        });
    }

    /// `check_consistency` (`citation.py:88-97`): every citation no
    /// document references, in registration order, as `(docname, lineno,
    /// label)` — Sphinx warns `Citation [%s] is not referenced.` at
    /// `location=(docname, lineno)`, the document's path and the
    /// citation's line (`type='ref', subtype='citation'`).
    pub fn unreferenced(&self) -> Vec<(&str, u32, &str)> {
        self.citations
            .iter()
            .filter(|(label, _)| !self.citation_refs.contains_key(label))
            .map(|(label, entry)| (entry.docname.as_str(), entry.lineno, label.as_str()))
            .collect()
    }
}

/// The citation domain's `check_consistency` warning for one citation
/// ([`CitationDomainData::unreferenced`]), located at `path`, the
/// document's source.
pub fn unreferenced_warning(path: PathBuf, lineno: u32, label: &str) -> BuildWarning {
    BuildWarning::new(
        path,
        Some(lineno as usize),
        format!("Citation [{label}] is not referenced."),
        WarningType::UnusedLabel,
    )
    .with_category(Some("ref.citation".to_string()))
}

/// The citation domain's registrations for one re-read document, replayed
/// in the merge phase:
///
/// * each `note_citation` call the read pass recorded
///   ([`crate::rst::RegistryExport::citations`]), whose duplicate warning —
///   `duplicate citation %s, other instance in %s` with `doc2path` of the
///   registration it replaces, at the citation, `[ref.citation]` — comes
///   back with the `seq` the transform spent, so the merge phase prints it
///   among the document's records where Sphinx did (probed, env project
///   `citations`: after the parse's records and before Footnotes');
/// * each `note_citation_reference` call, one per `pending_xref` the pass
///   left with `refdomain="citation"` (CitationReferenceTransform made
///   them all and nothing removes them before the doctree is stored), read
///   off the tree — the domain records only which documents reference a
///   label.
pub(crate) fn collect_registrations(
    env: &mut BuildEnvironment,
    doc: &DocumentSource<'_>,
    doc2path: &dyn Fn(&str) -> PathBuf,
    warnings: &mut Vec<(u32, BuildWarning)>,
) {
    for record in &doc.registry.citations {
        let entry = CitationEntry {
            docname: doc.docname.to_string(),
            labelid: record.node_id.clone(),
            lineno: record.line,
        };
        let Some(other) = env.citation.note_citation(&record.label, entry) else {
            continue;
        };
        warnings.push((
            record.seq,
            BuildWarning::new(
                source_path_of(doc, record.source),
                Some(record.line as usize),
                format!(
                    "duplicate citation {}, other instance in {}",
                    record.label,
                    doc2path(&other).display()
                ),
                WarningType::DuplicateLabel,
            )
            .with_category(Some("ref.citation".to_string())),
        ));
    }
    let mut stack: Vec<&Node> = vec![&doc.doctree.root];
    while let Some(node) = stack.pop() {
        if node.kind == kinds::PENDING_XREF
            && matches!(node.get("refdomain"), Some(AttrValue::Str(domain)) if domain == "citation")
        {
            if let Some(AttrValue::Str(target)) = node.get("reftarget") {
                env.citation.note_citation_reference(target, doc.docname);
            }
        }
        stack.extend(node.children.iter());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::std_domain::replay_registrations;
    use crate::rst::ParseOptions;
    use crate::transforms::parse_full_and_transform;
    use std::path::Path;

    fn entry(docname: &str, labelid: &str, lineno: u32) -> CitationEntry {
        CitationEntry {
            docname: docname.to_string(),
            labelid: labelid.to_string(),
            lineno,
        }
    }

    fn rows(data: &CitationDomainData) -> Vec<(&str, &str, u32)> {
        data.citations
            .iter()
            .map(|(label, e)| (label.as_str(), e.docname.as_str(), e.lineno))
            .collect()
    }

    /// `self.citations[label] = (...)` after the warning: the newer
    /// registration overwrites the older in place (dict order), and the
    /// caller learns whose it replaced.
    #[test]
    fn a_label_registered_again_names_the_earlier_document_and_takes_its_slot() {
        let mut data = CitationDomainData::default();
        assert_eq!(data.note_citation("Dup", entry("a", "dup", 4)), None);
        assert_eq!(data.note_citation("Lone", entry("a", "lone", 5)), None);
        assert_eq!(
            data.note_citation("Dup", entry("b", "dup", 8)),
            Some("a".to_string())
        );
        assert_eq!(rows(&data), [("Dup", "b", 8), ("Lone", "a", 5)]);
    }

    /// `clear_doc` (`citation.py:49-57`) drops the document's citations —
    /// a label re-registered afterwards goes to the end — and its
    /// references, a label nobody references any more with them.
    #[test]
    fn clearing_a_document_drops_its_citations_and_references() {
        let mut data = CitationDomainData::default();
        data.note_citation("A", entry("a", "a", 1));
        data.note_citation("B", entry("b", "b", 1));
        data.note_citation_reference("B", "a");
        data.note_citation_reference("B", "c");
        data.note_citation_reference("A", "a");
        data.clear_doc("a");
        assert_eq!(rows(&data), [("B", "b", 1)]);
        assert_eq!(
            data.citation_refs,
            BTreeMap::from([("B".to_string(), BTreeSet::from(["c".to_string()]))])
        );
        assert_eq!(data.note_citation("A", entry("a", "a", 2)), None);
        assert_eq!(rows(&data), [("B", "b", 1), ("A", "a", 2)]);
        assert_eq!(data.citations_index["A"], 1);
    }

    /// `check_consistency` (`citation.py:88-97`): unreferenced citations
    /// in registration order; references count by the label as written.
    #[test]
    fn unreferenced_citations_are_listed_in_registration_order() {
        let mut data = CitationDomainData::default();
        data.note_citation("Zed", entry("a", "zed", 3));
        data.note_citation("CIT", entry("a", "cit", 4));
        data.note_citation("Used", entry("b", "used", 5));
        data.note_citation_reference("Used", "c");
        data.note_citation_reference("cit", "c");
        assert_eq!(data.unreferenced(), [("a", 3, "Zed"), ("a", 4, "CIT")]);
        assert_eq!(
            unreferenced_warning(PathBuf::from("/src/a.rst"), 3, "Zed").render(),
            "/src/a.rst:3: WARNING: Citation [Zed] is not referenced. [ref.citation]"
        );
    }

    /// The merge-phase replay of two documents read in order: the second
    /// registration of `Dup` warns at its own line and source with the `seq`
    /// the read pass spent on it — between the parse's inline WARNING and
    /// Footnotes' overflow ERROR — naming the first document's path; every
    /// `pending_xref` the pass made is a reference to its target.
    #[test]
    fn the_replay_warns_at_the_seq_the_read_pass_spent() {
        let opts = |docname: &str| ParseOptions {
            source_path: format!("/src/{docname}.rst"),
            sphinx: true,
            docname: docname.to_string(),
            ..Default::default()
        };
        let a = parse_full_and_transform("See [Lone]_.\n\n.. [Dup] In a.\n", &opts("a"));
        let b = parse_full_and_transform(
            "Para *bad.\n\nSee [#]_ and [#]_.\n\n.. [Dup] In b.\n\n.. [#] One.\n",
            &opts("b"),
        );
        let seqs: Vec<(u32, &str)> = b
            .registry
            .diagnostics
            .iter()
            .map(|d| (d.seq, d.text.as_str()))
            .collect();
        assert_eq!(
            seqs,
            [
                (0, "Inline emphasis start-string without end-string."),
                (
                    2,
                    "Too many autonumbered footnote references: only 1 corresponding \
                     footnote available."
                ),
            ]
        );
        assert_eq!(b.registry.citations.len(), 1);
        assert_eq!(
            (
                b.registry.citations[0].label.as_str(),
                b.registry.citations[0].node_id.as_str(),
                b.registry.citations[0].line,
                b.registry.citations[0].seq,
            ),
            ("Dup", "dup", 5, 1)
        );

        let mut env = BuildEnvironment::default();
        let doc2path = |docname: &str| PathBuf::from(format!("/src/{docname}.rst"));
        let mut replay = |docname: &str, out: &crate::rst::ParseOutput| {
            let path = doc2path(docname);
            replay_registrations(
                &mut env,
                &DocumentSource {
                    docname,
                    doctree: &out.doctree,
                    registry: &out.registry,
                    path: Path::new(&path),
                },
                &doc2path,
            )
        };
        assert!(replay("a", &a).is_empty());
        let warnings: Vec<(u32, String)> = replay("b", &b)
            .into_iter()
            .map(|(seq, warning)| (seq, warning.render()))
            .collect();
        assert_eq!(
            warnings,
            [(
                1,
                "/src/b.rst:5: WARNING: duplicate citation Dup, other instance in \
                 /src/a.rst [ref.citation]"
                    .to_string()
            )]
        );
        assert_eq!(rows(&env.citation), [("Dup", "b", 5)]);
        assert_eq!(
            env.citation.citation_refs,
            BTreeMap::from([("Lone".to_string(), BTreeSet::from(["a".to_string()]))])
        );
    }
}
