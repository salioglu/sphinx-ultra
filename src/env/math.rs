//! The math domain (`SP/sphinx/domains/math.py`): its data in the build
//! environment ([`MathDomainData`], Sphinx's `env.domaindata['math']`),
//! the replay of the equation labels a document's parse registered
//! ([`process_doc`]) and what a reference to an equation resolves to
//! ([`resolve_xref`]), which the reference resolver builds its node from
//! ([`crate::env::resolve`]).
//!
//! Sphinx registers an equation's label with the domain while the math
//! directive runs, in the middle of the parse (`MathDirective.add_target`,
//! `SP/sphinx/directives/patches.py:177-200`). The parse here runs in
//! parallel, without the environment, so it records each registration with
//! the record seq it drew then ([`EquationRecord`]), and stage M replays the
//! records in sorted docname order: a label another equation already took
//! warns as Sphinx's parse warns, among the document's parse records.
//!
//! Three configuration values the domain reads are taken at Sphinx's
//! defaults, which no oracle corpus changes and the build configuration
//! does not carry: `math_numfig` (`True`), `math_numsep` (`'.'`) and
//! `math_eqref_format` (`None`, which formats `({number})`)
//! (`SP/sphinx/config.py:283-285`). So is the directive's `math_number_all`
//! (`False`, `:282`).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::diagnostics::{Category, Diagnostic, Severity};
use crate::doctree::ids::make_id;
use crate::env::BuildEnvironment;
use crate::rst::{EquationRecord, LogLocation};

/// `env.domaindata['math']['objects']`
/// (`SP/sphinx/domains/math.py:50-54`, `equations`): an equation's label
/// -> the document it is in and its number there. Only looked up, never
/// walked, so its order is not kept.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MathDomainData {
    pub equations: BTreeMap<String, (String, u32)>,
}

impl MathDomainData {
    /// The assignment half of `note_equation` (`:69-81`): the label names
    /// `number` in `docname`. Returns the document of the equation that
    /// held the label before, the one the caller's duplicate warning
    /// names.
    pub fn note_equation(&mut self, docname: &str, label: &str, number: u32) -> Option<String> {
        self.equations
            .insert(label.to_string(), (docname.to_string(), number))
            .map(|(other, _)| other)
    }

    /// `MathDomain.clear_doc` (`:89-92`): the document's equations leave.
    pub fn clear_doc(&mut self, docname: &str) {
        self.equations.retain(|_, (doc, _)| doc != docname);
    }
}

/// Stage M's replay of the equation labels the parse of `docname`
/// registered (`records`, in parse order) with the math domain
/// (`note_equation`, `SP/sphinx/domains/math.py:69-81`): each label takes
/// the equation's document and number, and one the domain already held,
/// from this document or one merged before it, warns `duplicate label of
/// equation %s, other instance in %s`, naming the other document by its
/// name, `location=node`, the equation's own (`set_source_info` stamped it
/// with the directive's line before the registration), with
/// `type='ref', subtype='equation'`. Each warning is returned with the seq
/// the parse drew at the registration, which keys it among the parse's
/// records. Locations render against the document's source table
/// (`sources`) and the project's source directory ([`LogLocation::render`]).
pub fn process_doc(
    env: &mut BuildEnvironment,
    docname: &str,
    records: &[EquationRecord],
    sources: &[String],
    srcdir: &Path,
) -> Vec<(u32, Diagnostic)> {
    let mut warnings = Vec::new();
    for record in records {
        if let Some(other) = env
            .math
            .note_equation(docname, &record.label, record.number)
        {
            let location = LogLocation::Node {
                source: record.source,
                line: record.line,
            };
            warnings.push((
                record.seq,
                Diagnostic {
                    location: location.render(sources, srcdir, docname),
                    severity: Severity::Warning,
                    message: format!(
                        "duplicate label of equation {}, other instance in {other}",
                        record.label
                    ),
                    category: Category::from_parts(Some("ref"), Some("equation")),
                },
            ));
        }
    }
    warnings
}

/// What a reference to an equation resolves to
/// ([`resolve_xref`]).
#[derive(Debug, PartialEq, Eq)]
pub struct EquationTarget<'e> {
    /// The document the equation is in.
    pub docname: &'e str,
    /// The id of the equation's target, `equation-<label>` as docutils'
    /// `make_id` spells it.
    pub node_id: String,
    /// The reference's text: the equation's number in `({number})`.
    pub title: String,
}

/// `MathDomain.resolve_xref` (`SP/sphinx/domains/math.py:99-134`), for
/// both of the domain's roles, `eq` and `numref`: the equation `target`
/// labels, if any. Its number is the document's own count when `numfig`
/// is off, and otherwise the number the toctree's figure numbering gave
/// its `displaymath` (`env.toc_fignumbers`), dotted, or none for a
/// document the numbering did not reach. A numbered document without a
/// `displaymath` table, which raises `KeyError` in Sphinx, is read as one
/// without the equation's number (decision 13: a crash is not modelled).
pub fn resolve_xref<'e>(
    env: &'e BuildEnvironment,
    numfig: bool,
    target: &str,
) -> Option<EquationTarget<'e>> {
    let (docname, number) = env.math.equations.get(target)?;
    let node_id = make_id(&format!("equation-{target}"));
    // `math_numfig` is on by default; `math_numsep.join(eqno.rsplit('.',
    // 1))` with the default `'.'` leaves the dotted number as it is.
    let eqno = if numfig {
        env.toc_fignumbers
            .get(docname)
            .map(|figtypes| {
                figtypes
                    .get("displaymath")
                    .and_then(|numbers| numbers.get(&node_id))
                    .map(|numbers| {
                        numbers
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(".")
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    } else {
        number.to_string()
    };
    Some(EquationTarget {
        docname,
        node_id,
        // `(math_eqref_format or '({number})').format(number=eqno)`.
        title: format!("({eqno})"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transforms::test_support::{merge_project, resolved};

    const EMPHASIS: &str = "WARNING: Inline emphasis start-string without end-string. [docutils]";

    /// A math directive's label registers the equation with the math
    /// domain while the document is parsed (`MathDirective.add_target`,
    /// `SP/sphinx/directives/patches.py:177-200`), and `:eq:` resolves to
    /// it through the domain (`MathDomain.resolve_xref`,
    /// `SP/sphinx/domains/math.py:99-134`): a reference to the equation's
    /// `equation-<label>` target whose text is its number, `(N)`, from
    /// this document or another. `docutils-0514` labels its equation with
    /// `:name:`, which also makes `my eq` an anonymous std label; a target
    /// no directive labelled is `equation not found` (`dangling_warnings`,
    /// `:55-57`), `[ref.eq]`, at the reference's line, for each of
    /// `sphinx_read-0335`'s three. Probed on Sphinx 9.1.0 (each project's
    /// resolved doctree and warning stream).
    #[test]
    fn math_labels_register_and_eq_resolves() {
        let project = merge_project(&[(
            "index",
            ".. math::\n   :class: eq\n   :name: my eq\n\n   x^2\n\nSee :eq:`my eq`.\n",
        )]);
        assert_eq!(
            project.env.std.anonlabels.get("my eq"),
            Some(&("index".to_string(), "my-eq".to_string()))
        );
        let index = resolved(&project, "index");
        assert_eq!(
            (index.body, index.warnings),
            (
                "    <target refid=\"equation-my-eq\">\n    \
                 <math_block classes=\"eq\" docname=\"index\" ids=\"my-eq equation-my-eq\" \
                 label=\"my eq\" names=\"my\\ eq\" no-wrap=\"0\" nowrap=\"0\" number=\"1\" \
                 xml:space=\"preserve\">\n        x^2\n    <paragraph>\n        See \n        \
                 <reference internal=\"1\" refid=\"equation-my-eq\">\n            (1)\n        .\n"
                    .to_string(),
                Vec::new()
            )
        );

        let project = merge_project(&[
            ("a", "A\n=\n\n.. math:: x\n   :label: ea\n"),
            ("index", "See :eq:`ea`.\n"),
        ]);
        assert_eq!(
            resolved(&project, "index").body,
            "    <paragraph>\n        See \n        \
             <reference internal=\"1\" refuri=\"#equation-ea\">\n            (1)\n        .\n"
        );

        let project = merge_project(&[(
            "index",
            "See :eq:`euler` and :eq:`the one <euler>` and :eq:`a  b` here.\n",
        )]);
        let index = resolved(&project, "index");
        // A math reference is the math domain's, not a cross-domain one.
        assert_eq!(index.unresolvable, 0);
        assert_eq!(
            index.warnings,
            [
                "<SRCDIR>/index.rst:1: WARNING: equation not found: euler [ref.eq]",
                "<SRCDIR>/index.rst:1: WARNING: equation not found: euler [ref.eq]",
                "<SRCDIR>/index.rst:1: WARNING: equation not found: a b [ref.eq]",
            ]
        );
        assert_eq!(
            index.body,
            "    <paragraph>\n        See \n        <literal classes=\"xref eq\">\n            \
             euler\n         and \n        <literal classes=\"xref eq\">\n            the one\n         \
             and \n        <literal classes=\"xref eq\">\n            a  b\n         here.\n"
        );
    }

    /// The math domain registers an equation label while the directive
    /// runs (`note_equation`, `SP/sphinx/domains/math.py:69-81`), so a
    /// label that an earlier document, or an earlier equation of this one,
    /// already took warns as the parse goes, `location=node`, naming the
    /// other document by its name: the warning is a parse record and
    /// prints among the document's parse records, keyed `(0, 0, seq)`. In
    /// one document, docutils then reports the target's `Duplicate ID`, at
    /// the top-level state machine's line (`document.set_id`,
    /// `SP/docutils/nodes.py:1832-1843`). Probed on Sphinx 9.1.0 (both
    /// projects' warning streams).
    #[test]
    fn duplicate_equation_label_keeps_parse_order() {
        let project = merge_project(&[
            ("a", "A\n=\n\n.. math:: x\n   :label: e1\n"),
            (
                "b",
                "B\n=\n\nPara *b.\n\n.. math:: y\n   :label: e1\n\nMore *b.\n",
            ),
        ]);
        assert_eq!(
            project.records,
            [
                format!("<SRCDIR>/b.rst:4: {EMPHASIS}"),
                "<SRCDIR>/b.rst:6: WARNING: duplicate label of equation e1, other instance in \
                 a [ref.equation]"
                    .to_string(),
                format!("<SRCDIR>/b.rst:9: {EMPHASIS}"),
            ]
        );

        let project = merge_project(&[(
            "index",
            "Title\n=====\n\nPara *a.\n\n.. math:: x\n   :label: e1\n\n.. math:: y\n   \
             :label: e1\n\nPara *b.\n",
        )]);
        assert_eq!(
            project.records,
            [
                format!("<SRCDIR>/index.rst:4: {EMPHASIS}"),
                "<SRCDIR>/index.rst:9: WARNING: duplicate label of equation e1, other \
                 instance in index [ref.equation]"
                    .to_string(),
                "<SRCDIR>/index.rst:8: ERROR: Duplicate ID: \"equation-e1\" used by <target \
                 ids=\"equation-e1\"> and <target ids=\"equation-e1\"> [docutils]"
                    .to_string(),
                format!("<SRCDIR>/index.rst:12: {EMPHASIS}"),
            ]
        );
    }

    /// An empty `:label:` names the equation `<docname>:<n>`, from a serial
    /// of its own, while its number counts every labelled equation of the
    /// document (`MathDirective.add_target`,
    /// `SP/sphinx/directives/patches.py:177-194`). `:any:` reaches the
    /// citation and math domains after the std one, in `domains.sorted()`
    /// order (`ReferencesResolver._resolve_pending_any_xref`), each under
    /// the role its `resolve_any_xref` reports: `'ref'` for a citation,
    /// whose winning content node takes `ref ref` as classes, and `'eq'`
    /// for an equation, so a target both name warns and takes the
    /// citation. Probed on Sphinx 9.1.0 (this document's resolved doctree
    /// and warning stream).
    #[test]
    fn empty_labels_number_and_any_reaches_both_domains() {
        let project = merge_project(&[(
            "index",
            "See :any:`CIT`, :any:`e1` and :any:`both`.\n\n.. [CIT] A citation.\n\n\
             .. [both] Another.\n\n.. math:: x\n   :label: e1\n\n.. math::\n   :label: both\n\n   \
             y\n\n.. math::\n   :label:\n\n   z\n\nSee :eq:`index:0`.\n",
        )]);
        let index = resolved(&project, "index");
        assert_eq!(
            index.body,
            "    <paragraph>\n        See \n        <reference internal=\"1\" refid=\"cit\">\n            \
             <literal classes=\"xref any ref ref\">\n                CIT\n        , \n        \
             <reference internal=\"1\" refid=\"equation-e1\">\n            (1)\n         and \n        \
             <reference internal=\"1\" refid=\"both\">\n            \
             <literal classes=\"xref any ref ref\">\n                both\n        .\n    \
             <citation docname=\"index\" ids=\"cit\" names=\"cit\">\n        \
             <label support_smartquotes=\"0\">\n            CIT\n        <paragraph>\n            \
             A citation.\n    <citation docname=\"index\" ids=\"both\" names=\"both\">\n        \
             <label support_smartquotes=\"0\">\n            both\n        <paragraph>\n            \
             Another.\n    <target refid=\"equation-e1\">\n    <math_block docname=\"index\" \
             ids=\"equation-e1\" label=\"e1\" no-wrap=\"0\" nowrap=\"0\" number=\"1\" \
             xml:space=\"preserve\">\n        x\n        \n    <target refid=\"equation-both\">\n    \
             <math_block docname=\"index\" ids=\"equation-both\" label=\"both\" no-wrap=\"0\" \
             nowrap=\"0\" number=\"2\" xml:space=\"preserve\">\n        y\n    \
             <target refid=\"equation-index-0\">\n    <math_block docname=\"index\" \
             ids=\"equation-index-0\" label=\"index:0\" no-wrap=\"0\" nowrap=\"0\" number=\"3\" \
             xml:space=\"preserve\">\n        z\n    <paragraph>\n        See \n        \
             <reference internal=\"1\" refid=\"equation-index-0\">\n            (3)\n        .\n"
        );
        assert_eq!(
            index.warnings,
            [
                "<SRCDIR>/index.rst:1: WARNING: more than one target found for 'any' \
                 cross-reference 'both': could be :ref:`both` or :eq:`(2)` [ref.any]"
            ]
        );
    }

    /// The domain's data: a label names the last equation noted under it,
    /// and a document's equations leave with it
    /// (`SP/sphinx/domains/math.py:69-92`).
    #[test]
    fn equations_follow_the_last_registration_and_leave_with_their_document() {
        let mut math = MathDomainData::default();
        assert_eq!(math.note_equation("a", "e1", 1), None);
        assert_eq!(math.note_equation("a", "e2", 2), None);
        assert_eq!(math.note_equation("b", "e1", 1), Some("a".to_string()));
        assert_eq!(math.equations.get("e1"), Some(&("b".to_string(), 1)));
        math.clear_doc("a");
        assert_eq!(
            math.equations.into_iter().collect::<Vec<_>>(),
            [("e1".to_string(), ("b".to_string(), 1))]
        );
    }
}
