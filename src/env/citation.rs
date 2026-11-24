//! The citation domain (`SP/sphinx/domains/citation.py`): its data in the
//! build environment ([`CitationDomainData`], Sphinx's
//! `env.domaindata['citation']`), the environment half of the two 619
//! transforms that fill it, which stage M runs in their place
//! ([`note_citations`], [`note_citation_references`]), and its consistency
//! check ([`check_consistency`]). Its resolution is the reference
//! resolver's ([`crate::env::resolve`]).
//!
//! The 619 transforms' tree halves run in stage R
//! ([`crate::transforms::footnotes`]): each citation of the tree takes the
//! document's name, each citation reference becomes a `pending_xref` into
//! this domain, and what they would register with the domain is recorded
//! on the document's state ([`DocumentState::citation_notes`],
//! [`DocumentState::citation_reference_targets`]). Stage M, which merges
//! the documents into the environment one at a time in sorted docname
//! order, registers it here, so a document's citations meet those of every
//! document merged before it, as in Sphinx's serial read.
//!
//! [`DocumentState::citation_notes`]: crate::transforms::state::DocumentState::citation_notes
//! [`DocumentState::citation_reference_targets`]: crate::transforms::state::DocumentState::citation_reference_targets

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::diagnostics::Category;
use crate::env::BuildEnvironment;
use crate::error::{BuildWarning, WarningType};
use crate::transforms::{location, TransformCx};

/// One citation the domain knows: the value of Sphinx's
/// `citations[label]`, `(docname, labelid, lineno)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationEntry {
    /// The document the citation is in.
    pub docname: String,
    /// The citation's first id (`node['ids'][0]`), which a reference to it
    /// links to.
    pub labelid: String,
    /// The citation's `node.line`, its line in the source that holds it
    /// (an included file's own line for a citation an included file
    /// holds); 0 for none, which is as falsy as Python's `None` where the
    /// consistency check reads it.
    pub lineno: u32,
}

/// `env.domaindata['citation']` (`SP/sphinx/domains/citation.py:41-47`).
///
/// `citations` is a Python dict, whose order the consistency check's
/// warnings follow: the order the labels were first noted, the documents'
/// merge order and tree order within a document. A label noted again
/// keeps its slot (dict assignment), and [`Self::clear_doc`] keeps the
/// order of what stays, so it is kept as a list with an index beside it,
/// as [`crate::env::PyDomainData`] keeps its objects.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CitationDomainData {
    /// label -> citation, in the order the labels were first noted.
    pub citations: Vec<(String, CitationEntry)>,
    /// label -> index into [`Self::citations`].
    pub citations_index: BTreeMap<String, usize>,
    /// `citation_refs`: a reference target -> the documents whose citation
    /// references name it, whether or not a citation has that label.
    pub citation_refs: BTreeMap<String, BTreeSet<String>>,
}

impl CitationDomainData {
    /// The assignment half of `note_citation`
    /// (`SP/sphinx/domains/citation.py:70-82`): `citations[label] =
    /// entry`. Returns the document of the citation that held the label
    /// before, the one the caller's duplicate warning names.
    pub fn note_citation(&mut self, label: &str, entry: CitationEntry) -> Option<String> {
        match self.citations_index.get(label) {
            Some(&index) => Some(std::mem::replace(&mut self.citations[index].1, entry).docname),
            None => {
                self.citations_index
                    .insert(label.to_string(), self.citations.len());
                self.citations.push((label.to_string(), entry));
                None
            }
        }
    }

    /// `note_citation_reference` (`:84-86`): the document refers to
    /// `target`.
    pub fn note_citation_reference(&mut self, target: &str, docname: &str) {
        self.citation_refs
            .entry(target.to_string())
            .or_default()
            .insert(docname.to_string());
    }

    /// `CitationDomain.clear_doc` (`:49-57`): the document's citations
    /// leave, the rest keeping their order, and the document no longer
    /// refers to any target; a target no other document refers to leaves
    /// with it.
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

    /// `citations.get(label)`: the citation `label` names, which a
    /// reference to it resolves to (`resolve_xref`, `:99-113`).
    pub fn get(&self, label: &str) -> Option<&CitationEntry> {
        self.citations_index
            .get(label)
            .map(|&index| &self.citations[index].1)
    }
}

/// 619-016 `CitationDefinitionTransform`'s environment half
/// (`SP/sphinx/domains/citation.py:138-143`): `note_citation` for each
/// citation its tree half found, in tree order
/// ([`DocumentState::citation_notes`]). A label the domain already holds,
/// from this document or one merged before it, warns, `location=node`,
/// naming the other citation's document by its path (`env.doc2path`,
/// here `doc2path`), with `type='ref', subtype='citation'`: a record of
/// the transform's row, `cx.key` ([`TransformCx::warn`]). The citation
/// takes the label either way, with its first id and its own line.
///
/// [`DocumentState::citation_notes`]: crate::transforms::state::DocumentState::citation_notes
pub fn note_citations(
    cx: &mut TransformCx<'_>,
    env: &mut BuildEnvironment,
    doc2path: &dyn Fn(&str) -> PathBuf,
) {
    for note in cx.state.citation_notes.clone() {
        let Some(node) = cx.tree.by_uid(note.node) else {
            debug_assert!(false, "a noted citation is in the document's arena");
            continue;
        };
        let lineno = location::own_location(cx.tree, node, cx.state)
            .line
            .unwrap_or(0);
        let entry = CitationEntry {
            docname: cx.docname.to_string(),
            labelid: note.ids.first().cloned().unwrap_or_default(),
            lineno,
        };
        if let Some(other) = env.citation.note_citation(&note.label, entry) {
            let message = format!(
                "duplicate citation {}, other instance in {}",
                note.label,
                doc2path(&other).display()
            );
            cx.warn(
                node,
                message,
                Category::from_parts(Some("ref"), Some("citation")),
            );
        }
    }
}

/// 619-017 `CitationReferenceTransform`'s environment half
/// (`SP/sphinx/domains/citation.py:175-176`): `note_citation_reference`
/// for the target of each `pending_xref` its tree half made, in tree order
/// ([`DocumentState::citation_reference_targets`]). It logs nothing.
///
/// [`DocumentState::citation_reference_targets`]: crate::transforms::state::DocumentState::citation_reference_targets
pub fn note_citation_references(env: &mut BuildEnvironment, docname: &str, targets: &[String]) {
    for target in targets {
        env.citation.note_citation_reference(target, docname);
    }
}

/// `CitationDomain.check_consistency` (`SP/sphinx/domains/citation.py:88-97`),
/// which `env.check_consistency` runs once every document has been read,
/// after the toctree's own checks
/// (`SP/sphinx/environment/__init__.py:797-823`): a warning for each
/// citation no document refers to, in the order the labels were noted,
/// `type='ref', subtype='citation'`. The location is `(docname, lineno)`,
/// which Sphinx renders `doc2path(docname)` (here `doc2path`), then
/// `:lineno` when the line is not 0: the citation's own line, read against
/// its document's path even when an included file holds it.
pub fn check_consistency(
    env: &BuildEnvironment,
    doc2path: &dyn Fn(&str) -> PathBuf,
) -> Vec<BuildWarning> {
    env.citation
        .citations
        .iter()
        .filter(|(label, _)| !env.citation.citation_refs.contains_key(label))
        .map(|(label, entry)| {
            BuildWarning::new(
                doc2path(&entry.docname),
                (entry.lineno != 0).then_some(entry.lineno as usize),
                format!("Citation [{label}] is not referenced."),
                WarningType::UnusedLabel,
            )
            .with_category(Some("ref.citation".to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transforms::test_support::{merge_project, resolved};

    const EMPHASIS: &str = "WARNING: Inline emphasis start-string without end-string. [docutils]";

    /// `CitationDefinitionTransform` (619-016) notes every citation of the
    /// document with the citation domain, which warns about a label a
    /// citation already took, in this document or in one merged before it,
    /// `location=node`, naming the other's document by its path
    /// (`note_citation`, `SP/sphinx/domains/citation.py:70-82`). The warning
    /// is the transform's record, keyed `(619, 16, seq)`: `b`'s parse
    /// records print first, then the duplicate, then 622's unreferenced
    /// footnote. In one document, docutils' duplicate-name warning comes
    /// first, from the parse. Probed on Sphinx 9.1.0 (both projects'
    /// warning streams).
    #[test]
    fn duplicate_citation_warns_in_stage_m() {
        let project = merge_project(&[
            ("a", "A\n=\n\n.. [X] in a\n\nPara *a.\n"),
            (
                "b",
                "B\n=\n\nPara *b.\n\n.. [X] in b\n\nMore *b.\n\n.. [#f] foot\n",
            ),
        ]);
        assert_eq!(
            project.records,
            [
                format!("<SRCDIR>/a.rst:6: {EMPHASIS}"),
                format!("<SRCDIR>/b.rst:4: {EMPHASIS}"),
                format!("<SRCDIR>/b.rst:8: {EMPHASIS}"),
                "<SRCDIR>/b.rst:6: WARNING: duplicate citation X, other instance in \
                 <SRCDIR>/a.rst [ref.citation]"
                    .to_string(),
                "<SRCDIR>/b.rst:10: WARNING: Footnote [#] is not referenced. [ref.footnote]"
                    .to_string(),
            ]
        );

        let project = merge_project(&[(
            "index",
            "Title\n=====\n\nSee [X]_.\n\n.. [X] one\n.. [X] two\n",
        )]);
        assert_eq!(
            project.records,
            [
                "<SRCDIR>/index.rst:7: WARNING: Duplicate explicit target name: \"x\". \
                 [docutils]",
                "<SRCDIR>/index.rst:7: WARNING: duplicate citation X, other instance in \
                 <SRCDIR>/index.rst [ref.citation]",
            ]
        );
    }

    /// A citation reference resolves through the citation domain to the
    /// citation its target labels, in any document (`resolve_xref`,
    /// `SP/sphinx/domains/citation.py:99-113`): a reference to the
    /// citation's id around the content node, which, like the content node
    /// left in place of a reference that names no citation, takes the
    /// `pending_xref`'s ids (`replace_self`'s `update_basic_atts`), the ids
    /// the citations' backrefs name. A citation reference is the domain's,
    /// not a cross-domain one. Probed on Sphinx 9.1.0 (this project's
    /// resolved doctree and warning stream).
    #[test]
    fn citation_references_resolve_through_the_domain() {
        let project = merge_project(&[
            ("a", "A\n=\n\n.. [Other] there\n"),
            (
                "index",
                "Title\n=====\n\nSee [CIT]_ and [Other]_ and [Nope]_.\n\n.. [CIT] here\n",
            ),
        ]);
        let index = resolved(&project, "index");
        assert_eq!(
            index.body,
            "    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n        \
             <paragraph>\n            See \n            \
             <reference ids=\"id1\" internal=\"1\" refid=\"cit\">\n                <inline>\n\
             \x20                   [CIT]\n             and \n            \
             <reference ids=\"id2\" internal=\"1\" refuri=\"#other\">\n                \
             <inline>\n                    [Other]\n             and \n            \
             <inline ids=\"id3\">\n                [Nope]\n            .\n        \
             <citation backrefs=\"id1\" docname=\"index\" ids=\"cit\" names=\"cit\">\n            \
             <label support_smartquotes=\"0\">\n                CIT\n            <paragraph>\n\
             \x20               here\n"
        );
        assert_eq!(
            index.warnings,
            ["<SRCDIR>/index.rst:4: WARNING: citation not found: Nope [ref.ref]"]
        );
        assert_eq!(index.unresolvable, 0);
    }

    /// The domain's citations keep the order their labels were first
    /// noted: a label noted again keeps its slot and takes the new
    /// citation, and a document's citations leave with it, the rest in
    /// order. The consistency check walks them in that order, warning
    /// about each no document refers to at `(docname, lineno)`, the line
    /// left off when it is 0 (`SP/sphinx/domains/citation.py:49-97`); a
    /// target stays referred to while any document refers to it.
    #[test]
    fn citations_keep_their_noted_order() {
        let entry = |docname: &str, labelid: &str, lineno: u32| CitationEntry {
            docname: docname.to_string(),
            labelid: labelid.to_string(),
            lineno,
        };
        let mut env = BuildEnvironment::default();
        let citations = &mut env.citation;
        assert_eq!(citations.note_citation("Z", entry("b", "z", 4)), None);
        assert_eq!(citations.note_citation("A", entry("a", "a", 0)), None);
        assert_eq!(citations.note_citation("M", entry("a", "m", 7)), None);
        assert_eq!(
            citations.note_citation("Z", entry("c", "z", 2)),
            Some("b".to_string())
        );
        citations.note_citation_reference("M", "b");
        citations.note_citation_reference("M", "c");
        let doc2path = |docname: &str| PathBuf::from(format!("<SRCDIR>/{docname}.rst"));
        let unreferenced = |env: &BuildEnvironment| -> Vec<String> {
            check_consistency(env, &doc2path)
                .into_iter()
                .map(|warning| crate::diagnostics::Diagnostic::from(warning).render(true))
                .collect()
        };
        assert_eq!(
            unreferenced(&env),
            [
                "<SRCDIR>/c.rst:2: WARNING: Citation [Z] is not referenced. [ref.citation]",
                "<SRCDIR>/a.rst: WARNING: Citation [A] is not referenced. [ref.citation]",
            ]
        );
        env.citation.clear_doc("b");
        env.citation.clear_doc("a");
        assert_eq!(
            env.citation.citations,
            [("Z".to_string(), entry("c", "z", 2))]
        );
        assert_eq!(env.citation.get("Z"), Some(&entry("c", "z", 2)));
        assert_eq!(env.citation.get("M"), None);
        assert_eq!(
            env.citation.citation_refs,
            BTreeMap::from([("M".to_string(), BTreeSet::from(["c".to_string()]))])
        );
        env.citation.clear_doc("c");
        assert_eq!(env.citation, CitationDomainData::default());
    }
}
