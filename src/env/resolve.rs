//! Cross-reference resolution: the `std` half of Sphinx's
//! `ReferencesResolver` post-transform
//! (`transforms/post_transforms/__init__.py:60-160`) plus
//! `StandardDomain.resolve_xref` (`domains/std/__init__.py:1034-1293`) and
//! the dangling-reference warnings both of them can raise
//! [ENV §4, §8 #4-#13].
//!
//! Sphinx resolves references while *writing* each document, over a fresh
//! copy of its doctree; this port does the same at the end of the resolve
//! phase, once numbering has run (`:numref:` reads `env.toc_fignumbers`).

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::doctree::{kinds, AttrValue, Doctree, Node};
use crate::env::numbers::clean_astext;
use crate::env::std_domain::DocumentIds;
use crate::env::toctree::docname_join;
use crate::env::BuildEnvironment;
use crate::error::{BuildWarning, WarningType};
use crate::intersphinx::{self, Diagnostic, HookOutcome, Intersphinx, XrefQuery};
use crate::pyformat::{self, FormatError, Value};
use crate::rst::SphinxCrash;
use crate::utils::py_repr_str;

/// One `pending_xref` to resolve — the attributes Sphinx's resolvers read
/// off the node.
#[derive(Clone, Copy)]
pub struct XrefRequest<'a> {
    /// The document being resolved (Sphinx's `fromdocname`).
    pub fromdoc: &'a str,
    /// `pending_xref['refdoc']`: the document the reference was *written*
    /// in, which is what a relative `:doc:` target resolves against. Equal
    /// to `fromdoc` unless the node was copied in from an include.
    pub refdoc: &'a str,
    pub reftype: &'a str,
    pub reftarget: &'a str,
    pub refexplicit: bool,
    /// `pending_xref['std:program']`: the `.. program::` in scope where the
    /// `:option:` reference was written.
    pub program: Option<&'a str>,
    /// `contnode.astext()` — the text the parse layer put in the reference.
    pub contnode_text: &'a str,
}

/// What resolution did with a reference. Sphinx expresses these three as
/// "returned a node" / "returned the contnode" / "returned None": the
/// middle case still counts as *resolved* to the caller, which is why a
/// `:numref:` that gives up never also raises a dangling-reference warning.
#[derive(Debug, PartialEq)]
pub enum XrefOutcome {
    /// A reference node replaces the `pending_xref`.
    Resolved(ResolvedXref),
    /// The content node stays in place. `warning` is the diagnostic the
    /// resolver logged on its way out (numref's #10-#13), which carries no
    /// `type`/`subtype` and so renders with no `[category]` suffix.
    Kept { warning: Option<String> },
    /// Nothing found: the caller decides whether this warrants a
    /// dangling-reference warning.
    Missing,
    /// A reference node replaces the `pending_xref`, and the resolver
    /// logged `warning` on its way (`Invalid math_eqref_format`), which
    /// carries no `type`/`subtype`.
    ResolvedWarning {
        resolved: ResolvedXref,
        warning: String,
    },
    /// The resolver raised what Sphinx 9.1.0 crashes on (decision 13),
    /// which the caller locates at the reference.
    Crash(SphinxCrash),
}

/// The reference node a successful resolution builds.
#[derive(Debug, PartialEq)]
pub struct ResolvedXref {
    /// `reference`, or `number_reference` for a resolved `:numref:`.
    pub kind: &'static str,
    pub refid: Option<String>,
    pub refuri: Option<String>,
    /// `number_reference['title']`: the *format*, not the rendered text.
    pub title: Option<String>,
    /// `reference['reftitle']`: the hover title `make_refnode` stamps for
    /// py targets (the matched fullname, or the module title). std's
    /// resolvers never pass one.
    pub reftitle: Option<String>,
    pub inner: Inner,
}

/// The reference's child node(s).
#[derive(Debug, PartialEq)]
pub enum Inner {
    /// Sphinx's `contnode`: whatever the parse layer produced, reused
    /// verbatim (`make_refnode(..., contnode)`).
    Contnode,
    /// A fresh `inline` node (`build_reference_node`, and the `:doc:`
    /// caption).
    Inline { text: String, classes: Vec<String> },
    /// A fresh Text node: the number a reference to an equation shows
    /// (`MathDomain.resolve_xref`'s `title`).
    Text(String),
    /// Existing nodes moved under the reference: the
    /// `pending_xref_condition(condition='resolved')` children a resolved
    /// py xref adopts (`PythonDomain.resolve_xref`, `:986-992`).
    Children(Vec<Node>),
}

/// Everything resolution reads: the environment, the numbering
/// configuration, the other documents' doctrees, and the builder's URI
/// policy.
pub struct Resolver<'a> {
    pub env: &'a BuildEnvironment,
    pub numfig: bool,
    pub numfig_format: &'a BTreeMap<String, String>,
    /// The math domain's configuration values
    /// ([`crate::env::math::MathConfig`]).
    pub math: crate::env::math::MathConfig,
    /// A document's doctree, for `:numref:`'s target-node lookup
    /// (`env.get_doctree(docname).ids`).
    pub doctree: &'a dyn Fn(&str) -> Option<Cow<'a, Doctree>>,
    /// `builder.get_relative_uri(from, to)`.
    pub relative_uri: &'a dyn Fn(&str, &str) -> String,
    /// The loaded cross-project inventories. An [`Intersphinx::default`]
    /// (no mapping configured) makes every hook below inert, which is what
    /// keeps a project without `intersphinx_mapping` byte-identical to what
    /// it produced before this existed.
    pub intersphinx: &'a Intersphinx,
}

impl Resolver<'_> {
    /// `StandardDomain.resolve_xref` (`:1034-1059`) — the role → resolver
    /// dispatch table.
    pub fn resolve_xref(&self, req: &XrefRequest<'_>) -> XrefOutcome {
        match req.reftype {
            "ref" => self.resolve_ref(req),
            "numref" => self.resolve_numref(req),
            "keyword" => self.resolve_keyword(req),
            "doc" => self.resolve_doc(req),
            "option" => self.resolve_option(req),
            "term" => self.resolve_term(req),
            _ => self.resolve_obj(req),
        }
    }

    /// `domain.resolve_xref` for the domain `refdomain` names: the citation
    /// domain's ([`Self::resolve_citation`]), the math domain's
    /// ([`Self::resolve_math`]), or the standard domain's
    /// ([`Self::resolve_xref`]) for `std` and a role with no domain. The
    /// python domain has a path of its own ([`resolve_py`]).
    fn resolve_in_domain(&self, refdomain: &str, req: &XrefRequest<'_>) -> XrefOutcome {
        match refdomain {
            "citation" => self.resolve_citation(req),
            "math" => self.resolve_math(req),
            _ => self.resolve_xref(req),
        }
    }

    /// `CitationDomain.resolve_xref` (`SP/sphinx/domains/citation.py:99-113`):
    /// the citation the target labels, in any document, linked through its
    /// first id, the content node kept (`make_refnode`).
    fn resolve_citation(&self, req: &XrefRequest<'_>) -> XrefOutcome {
        match self.env.citation.get(req.reftarget) {
            Some(entry) => XrefOutcome::Resolved(self.make_refnode(
                req.fromdoc,
                &entry.docname,
                Some(entry.labelid.as_str()).filter(|labelid| !labelid.is_empty()),
            )),
            None => XrefOutcome::Missing,
        }
    }

    /// `MathDomain.resolve_xref` (`SP/sphinx/domains/math.py:99-134`), for
    /// both of the domain's roles, `eq` and `numref`: the equation the
    /// target labels ([`crate::env::math::resolve_xref`]), linked through
    /// its target's id, with its number, `(N)`, in place of the content
    /// node.
    fn resolve_math(&self, req: &XrefRequest<'_>) -> XrefOutcome {
        match crate::env::math::resolve_xref(self.env, self.numfig, &self.math, req.reftarget) {
            Err(crash) => XrefOutcome::Crash(crash),
            Ok(Some(target)) => {
                let mut node =
                    self.make_refnode(req.fromdoc, target.docname, Some(&target.node_id));
                node.inner = Inner::Text(target.title);
                match target.warning {
                    Some(warning) => XrefOutcome::ResolvedWarning {
                        resolved: node,
                        warning,
                    },
                    None => XrefOutcome::Resolved(node),
                }
            }
            Ok(None) => XrefOutcome::Missing,
        }
    }

    /// `_resolve_ref_xref` (`:1061-1085`).
    fn resolve_ref(&self, req: &XrefRequest<'_>) -> XrefOutcome {
        let (docname, labelid, sectname) = if req.refexplicit {
            // A reference to an anonymous label uses the supplied caption.
            match self.env.std.anonlabels.get(req.reftarget) {
                Some((docname, labelid)) => (
                    docname.clone(),
                    labelid.clone(),
                    req.contnode_text.to_string(),
                ),
                None => return XrefOutcome::Missing,
            }
        } else {
            match self.env.std.labels.get(req.reftarget) {
                Some((docname, labelid, sectname)) => {
                    (docname.clone(), labelid.clone(), sectname.clone())
                }
                None => return XrefOutcome::Missing,
            }
        };
        if docname.is_empty() {
            return XrefOutcome::Missing;
        }
        XrefOutcome::Resolved(self.build_reference_node(
            LabelTarget {
                fromdoc: req.fromdoc,
                docname: &docname,
                labelid: &labelid,
            },
            &sectname,
            "ref",
            kinds::REFERENCE,
            None,
        ))
    }

    /// `_resolve_numref_xref` (`:1087-1170`) — the whole algorithm,
    /// warnings [ENV §8 #10-#13] included.
    fn resolve_numref(&self, req: &XrefRequest<'_>) -> XrefOutcome {
        // `labels` first; an anonymous-only label resolves with no figname.
        let (docname, labelid, figname) = match self.env.std.labels.get(req.reftarget) {
            Some((docname, labelid, figname)) => {
                (docname.clone(), labelid.clone(), Some(figname.clone()))
            }
            None => match self.env.std.anonlabels.get(req.reftarget) {
                Some((docname, labelid)) => (docname.clone(), labelid.clone(), None),
                None => return XrefOutcome::Missing,
            },
        };
        if docname.is_empty() {
            return XrefOutcome::Missing;
        }

        // `env.get_doctree(docname).ids.get(labelid)`: the numbered node
        // itself, which decides the figtype and owns the number's key.
        let Some((figtype, target_ids)) = (self.doctree)(&docname).and_then(|doctree| {
            let ids = DocumentIds::of(&doctree);
            let node = ids.node(&labelid)?;
            Some((
                enumerable_node_type(node).map(str::to_string),
                // `target_node['ids']`, which for a `.. _label:` written
                // above the node holds the label's id (`PropagateTargets`,
                // 260) — the same key `assign_figure_numbers` filed the
                // number under.
                node.attrs.ids.clone(),
            ))
        }) else {
            return XrefOutcome::Missing;
        };
        let Some(figtype) = figtype else {
            return XrefOutcome::Missing;
        };

        if figtype != "section" && !self.numfig {
            return XrefOutcome::Kept {
                warning: Some("numfig is disabled. :numref: is ignored.".to_string()),
            };
        }

        let fignumber = match self.fignumber(&figtype, &docname, &target_ids) {
            Ok(Some(fignumber)) => fignumber,
            // `get_fignumber` returning None: the contnode stays, silently.
            Ok(None) => return XrefOutcome::Kept { warning: None },
            Err(NoNumber) => {
                return XrefOutcome::Kept {
                    warning: Some(format!(
                        "Failed to create a cross reference. Any number is not assigned: {labelid}"
                    )),
                }
            }
        };

        let title = if req.refexplicit {
            req.contnode_text.to_string()
        } else {
            self.numfig_format
                .get(&figtype)
                .cloned()
                .unwrap_or_default()
        };
        if figname.is_none() && title.contains("{name}") {
            return XrefOutcome::Kept {
                warning: Some(format!("the link has no caption: {title}")),
            };
        }
        let fignum: Vec<String> = fignumber.iter().map(u32::to_string).collect();
        let fignum = fignum.join(".");
        let number = ("number", Value::Str(&fignum));
        let formatted = if title.contains("{name}") || title.contains("number") {
            // New style (`Fig.{number}`). Sphinx passes `name` to `format`
            // only `if figname:` — a *truthiness* test, so an empty caption
            // is formatted without it, and a `{name}` in the title then
            // raises the KeyError below (the `figname is None` guard above
            // is the only None-ness test in this algorithm).
            match figname.as_deref().filter(|figname| !figname.is_empty()) {
                Some(figname) => pyformat::format(&title, &[("name", Value::Str(figname)), number]),
                None => pyformat::format(&title, &[number]),
            }
        } else {
            // Old style (`Fig.%s`): `title % fignum`.
            pyformat::percent(&title, &fignum)
        };
        // `except KeyError` and `except TypeError` only: anything else
        // either style raises crashes Sphinx. A value the port does not
        // model reads as the `KeyError` it always has (T19b's divergence
        // list).
        let newtitle = match formatted {
            Ok(newtitle) => newtitle,
            Err(FormatError::KeyError(key) | FormatError::NotModelled(key)) => {
                return XrefOutcome::Kept {
                    warning: Some(format!(
                        "invalid numfig_format: {title} (KeyError({}))",
                        py_repr_str(&key)
                    )),
                }
            }
            Err(FormatError::TypeError(_)) => {
                return XrefOutcome::Kept {
                    warning: Some(format!("invalid numfig_format: {title}")),
                }
            }
            Err(FormatError::Crash(crash)) => return XrefOutcome::Crash(crash),
        };

        XrefOutcome::Resolved(self.build_reference_node(
            LabelTarget {
                fromdoc: req.fromdoc,
                docname: &docname,
                labelid: &labelid,
            },
            &newtitle,
            "numref",
            "number_reference",
            Some(title),
        ))
    }

    /// `StandardDomain.get_fignumber` (`:1395-1422`). `Err(NoNumber)` is
    /// Sphinx's `ValueError`.
    fn fignumber(
        &self,
        figtype: &str,
        docname: &str,
        target_ids: &[String],
    ) -> Result<Option<Vec<u32>>, NoNumber> {
        if figtype == "section" {
            // (`builder.name == 'latex'` returns `()` — no latex builder here.)
            let secnumbers = self.env.toc_secnumbers.get(docname).ok_or(NoNumber)?;
            let anchorname = format!("#{}", target_ids.first().ok_or(NoNumber)?);
            return Ok(secnumbers
                .get(&anchorname)
                .or_else(|| secnumbers.get(""))
                .cloned());
        }
        // `target_node['ids'][0]` raises IndexError when there is none,
        // which the caller turns into the same ValueError.
        let figure_id = target_ids.first().ok_or(NoNumber)?;
        self.env
            .toc_fignumbers
            .get(docname)
            .and_then(|per_type| per_type.get(figtype))
            .and_then(|per_id| per_id.get(figure_id))
            .cloned()
            .map(Some)
            .ok_or(NoNumber)
    }

    /// `_resolve_keyword_xref` (`:1172-1186`): named labels only, and the
    /// content node is kept as-is.
    fn resolve_keyword(&self, req: &XrefRequest<'_>) -> XrefOutcome {
        match self.env.std.labels.get(req.reftarget) {
            Some((docname, labelid, _)) if !docname.is_empty() => {
                XrefOutcome::Resolved(self.make_refnode(req.fromdoc, docname, Some(labelid)))
            }
            _ => XrefOutcome::Missing,
        }
    }

    /// `_resolve_doc_xref` (`:1188-1210`).
    fn resolve_doc(&self, req: &XrefRequest<'_>) -> XrefOutcome {
        let docname = docname_join(req.refdoc, req.reftarget);
        if !self.env.all_docs.contains_key(&docname) {
            return XrefOutcome::Missing;
        }
        let caption = if req.refexplicit {
            req.contnode_text.to_string()
        } else {
            self.env
                .titles
                .get(&docname)
                .map(clean_astext)
                .unwrap_or_default()
        };
        let mut node = self.make_refnode(req.fromdoc, &docname, None);
        node.inner = Inner::Inline {
            text: caption,
            classes: vec!["doc".to_string()],
        };
        XrefOutcome::Resolved(node)
    }

    /// `_resolve_option_xref` (`:1212-1249`): the exact key first, then the
    /// option-value fallback, then folding leading words into the program
    /// name.
    fn resolve_option(&self, req: &XrefRequest<'_>) -> XrefOutcome {
        let program = req.program.map(str::to_string);
        let target = req.reftarget.trim();

        let mut found = self.progoption(program.as_deref(), target);
        if found.is_none() {
            // `:option:`-foo=bar`` / `-foo[=bar]` / `-foo bar`.
            for needle in ["=", "[=", " "] {
                if let Some((stem, _)) = target.split_once(needle) {
                    found = self.progoption(program.as_deref(), stem);
                    if found.is_some() {
                        break;
                    }
                }
            }
        }
        if found.is_none() {
            // `:option:`git add --patch`` -> program `git-add`, option
            // `--patch`; one word is folded in per round.
            let mut commands: Vec<&str> = Vec::new();
            let mut rest = target;
            while let Some((subcommand, tail)) = split_once_whitespace(rest) {
                commands.push(subcommand);
                rest = tail;
                let progname = commands.join("-");
                found = self.progoption(Some(&progname), rest);
                if found.is_some() {
                    break;
                }
            }
        }
        match found {
            Some((docname, labelid)) => {
                XrefOutcome::Resolved(self.make_refnode(req.fromdoc, &docname, Some(&labelid)))
            }
            None => XrefOutcome::Missing,
        }
    }

    fn progoption(&self, program: Option<&str>, name: &str) -> Option<(String, String)> {
        self.env
            .std
            .progoptions
            .get(&(program.map(str::to_string), name.to_string()))
            .filter(|(docname, _)| !docname.is_empty())
            .cloned()
    }

    /// `_resolve_term_xref` (`:1251-1272`): the exact object first, then a
    /// case-insensitive fallback through `terms`.
    fn resolve_term(&self, req: &XrefRequest<'_>) -> XrefOutcome {
        if let XrefOutcome::Resolved(node) = self.resolve_obj(req) {
            return XrefOutcome::Resolved(node);
        }
        match self.env.std.terms.get(&req.reftarget.to_lowercase()) {
            Some((docname, labelid)) => {
                XrefOutcome::Resolved(self.make_refnode(req.fromdoc, docname, Some(labelid)))
            }
            None => XrefOutcome::Missing,
        }
    }

    /// `_resolve_obj_xref` (`:1274-1293`): the first object type this role
    /// can name that has an entry wins.
    fn resolve_obj(&self, req: &XrefRequest<'_>) -> XrefOutcome {
        for objtype in objtypes_for_role(req.reftype) {
            let key = (objtype.to_string(), req.reftarget.to_string());
            if let Some((docname, labelid)) = self.env.std.objects.get(&key) {
                if docname.is_empty() {
                    break;
                }
                return XrefOutcome::Resolved(self.make_refnode(
                    req.fromdoc,
                    docname,
                    Some(labelid),
                ));
            }
        }
        XrefOutcome::Missing
    }

    /// `sphinx.util.nodes.make_refnode`, which keeps the content node.
    fn make_refnode(&self, fromdoc: &str, docname: &str, targetid: Option<&str>) -> ResolvedXref {
        let mut node = ResolvedXref {
            kind: kinds::REFERENCE,
            refid: None,
            refuri: None,
            title: None,
            reftitle: None,
            inner: Inner::Contnode,
        };
        match targetid {
            Some(targetid) if fromdoc == docname => node.refid = Some(targetid.to_string()),
            Some(targetid) => {
                node.refuri = Some(format!(
                    "{}#{targetid}",
                    (self.relative_uri)(fromdoc, docname)
                ));
            }
            None => node.refuri = Some((self.relative_uri)(fromdoc, docname)),
        }
        node
    }

    /// The reference node for a resolved py target: [`Self::make_refnode`]
    /// semantics (Sphinx routes both `_make_module_refnode` and the object
    /// branch through `sphinx.util.nodes.make_refnode`) plus the
    /// `reftitle` and, for non-module targets, the
    /// `pending_xref_condition(condition='resolved')` children when the
    /// node carries them (`PythonDomain.resolve_xref`, `:983-994`).
    fn py_refnode(
        &self,
        fromdoc: &str,
        target: crate::env::py_domain::PyXrefTarget<'_>,
        resolved_children: Option<Vec<Node>>,
    ) -> ResolvedXref {
        // `make_refnode`'s targetid test is truthiness, not presence.
        let targetid = Some(target.node_id).filter(|id| !id.is_empty());
        let mut node = self.make_refnode(fromdoc, target.docname, targetid);
        node.reftitle = Some(target.reftitle);
        if !target.is_module {
            if let Some(children) = resolved_children {
                node.inner = Inner::Children(children);
            }
        }
        node
    }

    /// The candidate walk behind `:any:` —
    /// `ReferencesResolver._resolve_pending_any_xref`
    /// (`post_transforms/__init__.py:180-233`) minus the winner-picking and
    /// warning, which [`resolve_any_ref`] owns. Order is load-bearing (the
    /// FIRST candidate wins): `:doc:` resolution first (role `'doc'`, no
    /// `std:` prefix), then `StandardDomain.resolve_any_xref`
    /// (`std/__init__.py`: `'ref'` with the LOWERCASED target, `'option'`
    /// with the target as written, then the `objects` walk over
    /// [`STD_OBJECT_TYPE_ROLES`]), then the other domains in
    /// `domains.sorted()` order, of those this build resolves:
    /// `CitationDomain.resolve_any_xref` (role `'ref'`),
    /// `MathDomain.resolve_any_xref` (role `'eq'`) and
    /// `PythonDomain.resolve_any_xref`.
    ///
    /// Also adds the warnings the walk's resolvers logged to `warnings`,
    /// in order ([`any_candidate`]): Sphinx logs them during the walk,
    /// whichever candidate wins, before any ambiguity warning. A resolver
    /// that crashes Sphinx (decision 13) ends the walk with its crash,
    /// after the warnings logged before it.
    fn resolve_any(
        &self,
        req: &XrefRequest<'_>,
        py_module: Option<&str>,
        py_class: Option<&str>,
        resolved_children: Option<&Vec<Node>>,
        warnings: &mut Vec<String>,
    ) -> Result<Vec<AnyCandidate>, SphinxCrash> {
        let mut results: Vec<AnyCandidate> = Vec::new();
        let mut push = |role: String, node: ResolvedXref| {
            // `_stringify`: `node.get('reftitle', node.astext())`.
            let label = node.reftitle.clone().unwrap_or_else(|| match &node.inner {
                Inner::Inline { text, .. } | Inner::Text(text) => text.clone(),
                Inner::Contnode => req.contnode_text.to_string(),
                Inner::Children(children) => children.iter().map(Node::astext).collect(),
            });
            results.push(AnyCandidate { role, node, label });
        };

        // "first, try resolving as :doc:".
        let doc = self.resolve_doc(&XrefRequest {
            reftype: "doc",
            ..*req
        });
        if let Some(node) = any_candidate(doc, warnings)? {
            push("doc".to_string(), node);
        }

        // "next, do the standard domain (makes this a priority)":
        // StandardDomain.resolve_any_xref. ":ref: lowercases its target
        // automatically", so the any walk hands it the lowercased form;
        // "do not try 'keyword'".
        let ltarget = req.reftarget.to_lowercase();
        let reference = self.resolve_ref(&XrefRequest {
            reftype: "ref",
            reftarget: &ltarget,
            ..*req
        });
        if let Some(node) = any_candidate(reference, warnings)? {
            push("std:ref".to_string(), node);
        }
        let option = self.resolve_option(&XrefRequest {
            reftype: "option",
            ..*req
        });
        if let Some(node) = any_candidate(option, warnings)? {
            push("std:option".to_string(), node);
        }
        for (objtype, role) in STD_OBJECT_TYPE_ROLES {
            let name = if *objtype == "term" {
                // Terms alone are looked up lowercased — which only hits
                // entries whose as-written form IS lowercase, since the
                // objects key keeps the term's case (probe: `:any:`Aterm``
                // and `:any:`aterm`` both dangle against a glossary term
                // `Aterm`).
                ltarget.clone()
            } else {
                req.reftarget.to_string()
            };
            let key = ((*objtype).to_string(), name);
            if let Some((docname, labelid)) = self.env.std.objects.get(&key) {
                push(
                    format!("std:{role}"),
                    self.make_refnode(req.fromdoc, docname, Some(labelid)),
                );
            }
        }

        // CitationDomain and MathDomain.resolve_any_xref: each its
        // `resolve_xref`, under the role it reports. The math domain's
        // warns about a `math_eqref_format` it cannot fill and still
        // resolves.
        if let Some(node) = any_candidate(self.resolve_citation(req), warnings)? {
            push("ref".to_string(), node);
        }
        if let Some(node) = any_candidate(self.resolve_math(req), warnings)? {
            push("eq".to_string(), node);
        }

        // PythonDomain.resolve_any_xref, non-module entries adopting the
        // `resolved`-condition children exactly like resolve_xref's path.
        for (role, target) in crate::env::py_domain::resolve_any_xref(
            &self.env.py,
            py_module,
            py_class,
            req.reftarget,
        ) {
            let node = self.py_refnode(req.fromdoc, target, resolved_children.cloned());
            push(role, node);
        }
        Ok(results)
    }

    /// `StandardDomain.build_reference_node` (`:1002-1032`), which replaces
    /// the content node with a fresh `inline` carrying the section name.
    fn build_reference_node(
        &self,
        target: LabelTarget<'_>,
        sectname: &str,
        rolename: &str,
        kind: &'static str,
        title: Option<String>,
    ) -> ResolvedXref {
        let LabelTarget {
            fromdoc,
            docname,
            labelid,
        } = target;
        let mut node = ResolvedXref {
            kind,
            refid: None,
            refuri: None,
            title,
            reftitle: None,
            inner: Inner::Inline {
                text: sectname.to_string(),
                classes: vec!["std".to_string(), format!("std-{rolename}")],
            },
        };
        // Note this arm does *not* require a non-empty labelid, unlike
        // `make_refnode`.
        if docname == fromdoc {
            node.refid = Some(labelid.to_string());
        } else {
            let mut refuri = (self.relative_uri)(fromdoc, docname);
            if !labelid.is_empty() {
                refuri.push('#');
                refuri.push_str(labelid);
            }
            node.refuri = Some(refuri);
        }
        node
    }
}

/// `StandardDomain.object_types` in declaration order (dict order is the
/// `resolve_any_xref` walk order), paired with each ObjType's first role
/// (`Domain.role_for_objtype`): term/token/label/confval/envvar/cmdoption/
/// doc → term/token/ref/confval/envvar/option/doc. Labels and documents
/// never live in `objects` (they have their own registries), so those two
/// keys are dead weight carried for fidelity.
const STD_OBJECT_TYPE_ROLES: &[(&str, &str)] = &[
    ("term", "term"),
    ("token", "token"),
    ("label", "ref"),
    ("confval", "confval"),
    ("envvar", "envvar"),
    ("cmdoption", "option"),
    ("doc", "doc"),
];

/// One `:any:` candidate: the role string Sphinx's resolvers hand back
/// (`'doc'`, `'std:ref'`, `'py:func'`, ...), the node it built, and the
/// text half of the ambiguity warning's ``:role:`label``` form.
struct AnyCandidate {
    role: String,
    node: ResolvedXref,
    label: String,
}

/// The node one resolver of the `:any:` walk ([`Resolver::resolve_any`])
/// built, if any. A warning it logged on the way is added to `warnings`,
/// because Sphinx logs it during the walk whether or not that candidate
/// wins: a [`XrefOutcome::ResolvedWarning`] still yields its node. No
/// resolver of the walk keeps its content node (only `:numref:` does, and
/// the walk never calls it), so a [`XrefOutcome::Kept`] adds no candidate.
/// A resolver that crashed ends the walk with its crash.
fn any_candidate(
    outcome: XrefOutcome,
    warnings: &mut Vec<String>,
) -> Result<Option<ResolvedXref>, SphinxCrash> {
    Ok(match outcome {
        XrefOutcome::Resolved(node) => Some(node),
        XrefOutcome::ResolvedWarning { resolved, warning } => {
            warnings.push(warning);
            Some(resolved)
        }
        XrefOutcome::Kept { warning } => {
            warnings.extend(warning);
            None
        }
        XrefOutcome::Missing => None,
        XrefOutcome::Crash(crash) => return Err(crash),
    })
}

/// The label a reference resolved to, as `build_reference_node` takes it.
struct LabelTarget<'a> {
    fromdoc: &'a str,
    docname: &'a str,
    labelid: &'a str,
}

/// Sphinx's `ValueError` out of `get_fignumber`.
#[derive(Debug)]
struct NoNumber;

/// `ws_re.split(target, maxsplit=1)`: the first whitespace run splits the
/// leading word off. `ws_re` is `\s+`, Python's `str.isspace` — so a
/// `\x1f` (which `OptionXRefRole` keeps in the reftarget) splits a
/// subcommand off exactly as a space would ([`crate::utils::py_isspace`]).
fn split_once_whitespace(target: &str) -> Option<(&str, &str)> {
    let start = target.find(crate::utils::py_isspace)?;
    let end = target[start..]
        .find(|c: char| !crate::utils::py_isspace(c))
        .map(|offset| start + offset)
        .unwrap_or(target.len());
    Some((&target[..start], &target[end..]))
}

/// `StandardDomain.objtypes_for_role` over `object_types` (`:729-737`).
fn objtypes_for_role(role: &str) -> &'static [&'static str] {
    match role {
        "term" => &["term"],
        "token" => &["token"],
        "ref" | "keyword" => &["label"],
        "confval" => &["confval"],
        "envvar" => &["envvar"],
        "option" => &["cmdoption"],
        "doc" => &["doc"],
        _ => &[],
    }
}

/// `StandardDomain.get_enumerable_node_type` (`:1380-1393`) — note this is
/// the std domain's own table, so a `math_block` is not enumerable here
/// even though the math domain numbers it.
fn enumerable_node_type(node: &Node) -> Option<&'static str> {
    match node.kind {
        kinds::SECTION => Some("section"),
        "figure" => Some("figure"),
        kinds::TABLE => Some("table"),
        "container" => Some("code-block"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The document walk
// ---------------------------------------------------------------------------

/// Nitpick configuration, as `warn_missing_reference` consults it
/// (`post_transforms/__init__.py:255-282`).
pub struct NitpickConfig<'a> {
    pub nitpicky: bool,
    pub ignore: &'a [(String, String)],
    pub ignore_regex: &'a [(String, String)],
}

/// What resolving one document produced.
#[derive(Default)]
pub struct DocumentResolution {
    pub warnings: Vec<BuildWarning>,
    /// References into a domain this build has no implementation for —
    /// every `refdomain` outside `{"", "std", "py", "citation", "math"}`
    /// (`c:`, `cpp:`, `js:`, ...) — counted rather than warned about.
    pub unresolvable_domain_refs: usize,
    /// The reference whose resolution crashes Sphinx 9.1.0 (decision 13),
    /// located at it: the walk stops there, after the warnings of the
    /// references before it.
    pub crash: Option<SphinxCrash>,
}

impl DocumentResolution {
    /// Keeps `crash` in [`Self::crash`], raised resolving the reference at
    /// `path` and `line`, located there as a warning about it is.
    fn raise_at(&mut self, crash: SphinxCrash, path: &Path, line: Option<usize>) {
        self.crash = Some(crash.rendered_at(crate::diagnostics::location_of(path, line)));
    }

    /// Whether a reference's resolution crashed ([`Self::raise_at`]).
    fn crashed(&self) -> bool {
        self.crash.is_some()
    }
}

/// Resolve every `pending_xref` in one document, rewriting the tree the way
/// `ReferencesResolver.run` does: the node is replaced by the reference
/// that resolution built, or by its own content node when it failed.
///
/// Every warning it logs is about a node (`location=node`), so it names
/// the node's source as `get_node_location` does, made absolute against
/// the project's source directory `source_dir`
/// ([`crate::rst::node_source_path`]): a node from an included file names
/// `<srcdir>/part.rst`, whatever the cwd. `path`, the document's own
/// source, stands for a source the table does not know.
pub fn resolve_document(
    resolver: &Resolver<'_>,
    nitpick: &NitpickConfig<'_>,
    docname: &str,
    doctree: &mut Doctree,
    path: &Path,
    source_dir: &Path,
) -> DocumentResolution {
    let mut out = DocumentResolution::default();
    // The walk mutates `root` while warnings read the source table for
    // each node's `(source, line)`; the table is tiny, so a copy of it,
    // its paths made absolute, is the simplest split.
    let sources: Vec<String> = doctree
        .sources
        .iter()
        .map(|source| crate::rst::node_source_path(source, source_dir))
        .collect();
    resolve_children(
        resolver,
        nitpick,
        docname,
        &mut doctree.root,
        &sources,
        path,
        None,
        &mut out,
    );
    out
}

/// The `(source, line)` a warning about a node reports — docutils'
/// `get_source_line`, which `sphinx.util.logging.get_node_location` runs
/// for every `logger.warning(..., location=node)`: the node's OWN
/// `(source, line)` when it has one, else the nearest ancestor's, else
/// nothing (the warning then prints with no location prefix at all).
///
/// Threaded down the resolution walk as the nearest stamped ancestor's
/// location, so an unstamped `pending_xref` — the doc-field xrefs
/// `DocFieldTransformer` synthesizes carry line 0 by design, see
/// `DocFieldEnv` in src/rst/block.rs — locates where sphinx locates it.
type Location = Option<(u16, u32)>;

/// Whether docutils' walk would stop at this node. Our parser stamps a span
/// on every node it builds; docutils stamps most containers too (sections,
/// paragraphs, list items, admonitions, ...) but NOT the `document` root
/// (its source lives in the attribute dict, not on `node.source`), nor
/// `desc`/`desc_content` (`ObjectDescription.run` builds both bare and
/// calls `set_source_info` on the signature only), so those three are
/// skipped regardless of the span they carry. A zero line is "unstamped"
/// for any kind.
fn contributes_location(node: &Node) -> bool {
    node.span.line != 0 && !matches!(node.kind, kinds::DOCUMENT | "desc" | "desc_content")
}

#[allow(clippy::too_many_arguments)]
fn resolve_children(
    resolver: &Resolver<'_>,
    nitpick: &NitpickConfig<'_>,
    docname: &str,
    node: &mut Node,
    sources: &[String],
    path: &Path,
    inherited: Location,
    out: &mut DocumentResolution,
) {
    if out.crashed() {
        return;
    }
    let location = if contributes_location(node) {
        Some((node.span.source, node.span.line))
    } else {
        inherited
    };
    for child in &mut node.children {
        resolve_children(
            resolver, nitpick, docname, child, sources, path, location, out,
        );
    }
    if out.crashed()
        || !node
            .children
            .iter()
            .any(|child| child.kind == kinds::PENDING_XREF)
    {
        return;
    }
    let children = std::mem::take(&mut node.children);
    for mut child in children {
        // Sphinx's resolution stops at a reference that crashes it
        // (decision 13): the ones after it stay as they are.
        if child.kind != kinds::PENDING_XREF || out.crashed() {
            node.children.push(child);
            continue;
        }
        // `node.replace_self(new_nodes)` (`post_transforms/__init__.py:92`,
        // `SP/docutils/nodes.py:1110-1130`): the first new node, when it is
        // an element, takes the pending_xref's basic attributes — a
        // citation reference's ids, which its citation's backlink names.
        let basic = take_basic_atts(&mut child);
        let mut replacement = resolve_one(
            resolver, nitpick, docname, child, sources, path, location, out,
        );
        if let Some(first) = replacement.first_mut() {
            if first.kind != kinds::TEXT {
                update_basic_atts(first, basic);
            }
        }
        node.children.extend(replacement);
    }
}

/// docutils' `Element.basic_attributes` taken out of `node`: its `ids`,
/// `classes`, `names` and `dupnames`.
pub(crate) fn take_basic_atts(node: &mut Node) -> [Vec<String>; 4] {
    [
        std::mem::take(&mut node.attrs.ids),
        std::mem::take(&mut node.attrs.classes),
        std::mem::take(&mut node.attrs.names),
        std::mem::take(&mut node.attrs.dupnames),
    ]
}

/// docutils `Element.update_basic_atts` (`SP/docutils/nodes.py:850-871`):
/// each basic attribute of `node` gains, in order, the values of `basic`'s
/// ([`take_basic_atts`]) it does not hold yet.
pub(crate) fn update_basic_atts(node: &mut Node, basic: [Vec<String>; 4]) {
    let [ids, classes, names, dupnames] = basic;
    for (list, values) in [
        (&mut node.attrs.ids, ids),
        (&mut node.attrs.classes, classes),
        (&mut node.attrs.names, names),
        (&mut node.attrs.dupnames, dupnames),
    ] {
        for value in values {
            if !list.contains(&value) {
                list.push(value);
            }
        }
    }
}

/// `ReferencesResolver._resolve_pending_xref` for a single node.
#[allow(clippy::too_many_arguments)]
fn resolve_one(
    resolver: &Resolver<'_>,
    nitpick: &NitpickConfig<'_>,
    docname: &str,
    node: Node,
    sources: &[String],
    doc_path: &Path,
    inherited: Location,
    out: &mut DocumentResolution,
) -> Vec<Node> {
    let span = node.span;
    // Warnings locate the way `get_source_line` does: at the node's own
    // `(source, line)` when it is stamped, else at the nearest stamped
    // ancestor's (an unstamped node under an unstamped tree — a doc-field
    // xref in a description directly under the document — has no location
    // at all, and sphinx prints the bare `WARNING:`). Never the enclosing
    // document's path for a node that came from an included file.
    let location = if span.line != 0 {
        Some((span.source, span.line))
    } else {
        inherited
    };
    let (source_path, line): (PathBuf, Option<usize>) = match location {
        Some((source, line)) => (
            sources
                .get(source as usize)
                .map(PathBuf::from)
                .unwrap_or_else(|| doc_path.to_path_buf()),
            Some(line as usize),
        ),
        None => (PathBuf::new(), None),
    };
    let path = source_path.as_path();
    let refdomain = attr_str(&node, "refdomain").unwrap_or_default().to_string();
    let reftype = attr_str(&node, "reftype").unwrap_or_default().to_string();
    let reftarget = attr_str(&node, "reftarget").unwrap_or_default().to_string();
    let refdoc = attr_str(&node, "refdoc").unwrap_or(docname).to_string();
    let refexplicit = matches!(node.get("refexplicit"), Some(AttrValue::Int(1)));
    let refwarn = matches!(node.get("refwarn"), Some(AttrValue::Int(1)));
    // `OptionXRefRole.process_link` stamps this on every `:option:`, using
    // Python None outside a `.. program::` scope — which pformat renders as
    // the "True" sentinel (see `std_domain::is_none_sentinel`).
    let program = attr_str(&node, "std:program")
        .filter(|program| !crate::env::std_domain::is_none_sentinel(program))
        .map(str::to_string);
    // `node['intersphinx']`: the stamp `:external:` leaves, which sends the
    // node through `IntersphinxRoleResolver` instead of ordinary resolution.
    let external = matches!(node.get("intersphinx"), Some(AttrValue::Int(1)));
    let inventory = attr_str(&node, "inventory").map(str::to_string);
    let role_error = attr_str(&node, "intersphinx_role_error").map(str::to_string);
    // `PyXRefRole.process_link` context stamps (Python `None` renders as
    // the "True" sentinel, and an empty ref_context value is falsy in
    // every place Sphinx reads these).
    let py_module = attr_str(&node, "py:module")
        .filter(|value| !crate::env::std_domain::is_none_sentinel(value) && !value.is_empty())
        .map(str::to_string);
    let py_class = attr_str(&node, "py:class")
        .filter(|value| !crate::env::std_domain::is_none_sentinel(value) && !value.is_empty())
        .map(str::to_string);
    // `searchmode = 1 if node.hasattr('refspecific') else 0` (`:942`) — a
    // PRESENCE test: annotation xrefs carry `refspecific="0"` and still
    // search in refspecific mode (probe: a bare `Cls` annotation resolves
    // `pkg.Cls` through the fuzzy pass).
    let searchmode: u8 = u8::from(node.get("refspecific").is_some());
    let children = XrefChildren::split(node.children);
    let contnode = children.contnode();
    let contnode_text = contnode.as_ref().map(Node::astext).unwrap_or_default();

    let query = XrefQuery {
        refdomain: &refdomain,
        reftype: &reftype,
        reftarget: &reftarget,
        refexplicit,
        refdoc: &refdoc,
        contnode_text: &contnode_text,
    };

    // `:external:` first, exactly like the post-transform that runs one
    // priority ahead of the reference resolver — except when the inventory
    // it names is this project, which Sphinx's role never stamps at all.
    let self_referential = inventory.as_deref().is_some_and(|inventory| {
        !resolver.intersphinx.resolve_self.is_empty()
            && resolver.intersphinx.resolve_self == inventory
    });
    if external && !self_referential {
        return resolve_external(
            resolver,
            &query,
            inventory.as_deref(),
            role_error.as_deref(),
            contnode,
            span,
            line,
            path,
            out,
        );
    }

    // `:any:` is the one role with no domain (`refdomain=""` routes
    // `_resolve_pending_xref_in_domain` to the "really hardwired reference
    // types" branch, `post_transforms/__init__.py:216-222`).
    if refdomain.is_empty() && reftype == "any" {
        return resolve_any_ref(
            resolver,
            nitpick,
            docname,
            &query,
            PyRefContext {
                module: py_module.as_deref(),
                class: py_class.as_deref(),
                searchmode: 1,
                refwarn,
            },
            program.as_deref(),
            children,
            span,
            line,
            path,
            out,
        );
    }

    // Domains this build has no resolver for (`c:`, `cpp:`, `js:`, ...)
    // are left alone: warning about them would report every such reference
    // in every project as broken. The count feeds the build's one-line
    // notice. Intersphinx still gets a look first — a reference into
    // another project's inventory is exactly what it is for.
    if !matches!(refdomain.as_str(), "" | "std" | "py" | "citation" | "math") {
        let mut diagnostics = Vec::new();
        let outcome = resolver
            .intersphinx
            .resolve_detect(&query, &mut diagnostics);
        report(out, diagnostics, line, path);
        if let HookOutcome::Resolved(resolution) = outcome {
            return vec![intersphinx_node(resolution, contnode, span)];
        }
        out.unresolvable_domain_refs += 1;
        return children.fallback(out, line, path);
    }
    if refdomain == "py" {
        return resolve_py(
            resolver,
            nitpick,
            docname,
            &query,
            PyRefContext {
                module: py_module.as_deref(),
                class: py_class.as_deref(),
                searchmode,
                refwarn,
            },
            children,
            span,
            line,
            path,
            out,
        );
    }
    let req = XrefRequest {
        fromdoc: docname,
        refdoc: &refdoc,
        reftype: &reftype,
        reftarget: &reftarget,
        refexplicit,
        program: program.as_deref(),
        contnode_text: &contnode_text,
    };
    let mut outcome = resolver.resolve_in_domain(&refdomain, &req);
    if outcome == XrefOutcome::Missing {
        // The `missing-reference` event, which is where intersphinx hooks
        // in: after the domain, before the warning.
        let mut diagnostics = Vec::new();
        let hook = resolver
            .intersphinx
            .resolve_detect(&query, &mut diagnostics);
        report(out, diagnostics, line, path);
        match hook {
            HookOutcome::Resolved(resolution) => {
                return vec![intersphinx_node(resolution, contnode, span)];
            }
            // The target named this project: the local domain again, with
            // the prefix stripped, and Sphinx keeps whatever it returns
            // (`post_transforms/__init__.py:140-155`), a warning or a kept
            // content node included. A miss still reports the target as
            // written, because Sphinx never rewrote it.
            HookOutcome::SelfReferential(stripped) => {
                outcome = resolver.resolve_in_domain(
                    &refdomain,
                    &XrefRequest {
                        reftarget: &stripped,
                        ..req
                    },
                );
            }
            HookOutcome::Missing => {}
        }
    }

    match outcome {
        XrefOutcome::Resolved(resolved) => {
            vec![reference_node(resolved, contnode, span)]
        }
        XrefOutcome::ResolvedWarning { resolved, warning } => {
            out.warnings.push(
                BuildWarning::new(
                    path.to_path_buf(),
                    line,
                    warning,
                    WarningType::BrokenCrossReference,
                )
                .with_category(None),
            );
            vec![reference_node(resolved, contnode, span)]
        }
        XrefOutcome::Kept { warning } => {
            if let Some(message) = warning {
                out.warnings.push(
                    BuildWarning::new(
                        path.to_path_buf(),
                        line,
                        message,
                        WarningType::BrokenCrossReference,
                    )
                    .with_category(None),
                );
            }
            contnode.into_iter().collect()
        }
        XrefOutcome::Crash(crash) => {
            out.raise_at(crash, path, line);
            Vec::new()
        }
        XrefOutcome::Missing => {
            if let Some(message) = missing_reference_warning(
                resolver.env,
                nitpick,
                &refdomain,
                &reftype,
                &reftarget,
                refwarn,
            ) {
                out.warnings.push(
                    BuildWarning::new(
                        path.to_path_buf(),
                        line,
                        message,
                        WarningType::BrokenCrossReference,
                    )
                    // `logger.warning(..., type='ref', subtype=typ)`.
                    .with_category(Some(format!("ref.{reftype}"))),
                );
            }
            children.fallback(out, line, path)
        }
    }
}

/// The py-role context [`resolve_py`] reads off the `pending_xref`.
struct PyRefContext<'a> {
    /// `node['py:module']` / `node['py:class']`, None-sentinel and
    /// empty-string (Python falsy) both read as absent.
    module: Option<&'a str>,
    class: Option<&'a str>,
    searchmode: u8,
    refwarn: bool,
}

/// `PythonDomain.resolve_xref` wired into the resolver's event order
/// (`ReferencesResolver._resolve_pending_xref`): the domain first, then the
/// `missing-reference` event — intersphinx at its default priority 500,
/// [`crate::env::py_domain::builtin_resolver`] at 900 — then the
/// self-referential retry, then the nitpicky warning. Probe-pinned
/// consequence of the priorities: a builtin name a loaded inventory carries
/// resolves EXTERNALLY; one it doesn't carry is silenced.
#[allow(clippy::too_many_arguments)]
fn resolve_py(
    resolver: &Resolver<'_>,
    nitpick: &NitpickConfig<'_>,
    docname: &str,
    query: &XrefQuery<'_>,
    ctx: PyRefContext<'_>,
    children: XrefChildren,
    span: crate::doctree::Span,
    line: Option<usize>,
    path: &Path,
    out: &mut DocumentResolution,
) -> Vec<Node> {
    use crate::env::py_domain;

    let reftype = query.reftype;
    let contnode = children.contnode();

    // The domain's own resolution. The ambiguity warning fires even when
    // the reference then resolves (to the first match).
    let resolve = |target: &str, out: &mut DocumentResolution| {
        let (found, ambiguity) = py_domain::resolve_xref(
            &resolver.env.py,
            ctx.module,
            ctx.class,
            reftype,
            target,
            ctx.searchmode,
        );
        if let Some(message) = ambiguity {
            out.warnings.push(
                BuildWarning::new(
                    path.to_path_buf(),
                    line,
                    message,
                    WarningType::BrokenCrossReference,
                )
                // `type='ref', subtype='python'` (`:977-978`).
                .with_category(Some("ref.python".to_string())),
            );
        }
        found
    };
    if let Some(target) = resolve(query.reftarget, out) {
        let resolved = resolver.py_refnode(docname, target, children.resolved.clone());
        return vec![reference_node(resolved, contnode, span)];
    }

    // The `missing-reference` event: intersphinx first (priority 500)...
    let mut diagnostics = Vec::new();
    let outcome = resolver.intersphinx.resolve_detect(query, &mut diagnostics);
    report(out, diagnostics, line, path);
    match outcome {
        HookOutcome::Resolved(resolution) => {
            return vec![intersphinx_node(resolution, contnode, span)];
        }
        HookOutcome::SelfReferential(stripped) => {
            // ...then builtin_resolver (900), which reads the reftarget
            // intersphinx just rewrote on the node...
            if py_domain::builtin_resolver(reftype, &stripped) {
                return children.contnode().into_iter().collect();
            }
            // ...and only then the domain retry with the stripped target.
            // The warning below still reports the target as written.
            if let Some(target) = resolve(&stripped, out) {
                let resolved = resolver.py_refnode(docname, target, children.resolved.clone());
                return vec![reference_node(resolved, contnode, span)];
            }
        }
        HookOutcome::Missing => {
            if py_domain::builtin_resolver(reftype, query.reftarget) {
                // "Do not emit nitpicky warnings for built-in types": the
                // event returns the contnode, so no `*`-condition fallback
                // either (probe: an unqualified-names annotation keeps the
                // SHORT name when builtin-silenced).
                return children.contnode().into_iter().collect();
            }
        }
    }

    if let Some(message) = missing_reference_warning(
        resolver.env,
        nitpick,
        "py",
        reftype,
        query.reftarget,
        ctx.refwarn,
    ) {
        out.warnings.push(
            BuildWarning::new(
                path.to_path_buf(),
                line,
                message,
                WarningType::BrokenCrossReference,
            )
            // `logger.warning(..., type='ref', subtype=typ)`.
            .with_category(Some(format!("ref.{reftype}"))),
        );
    }
    children.fallback(out, line, path)
}

/// `ReferencesResolver._resolve_pending_any_xref` wired into the event
/// order: the candidate walk ([`Resolver::resolve_any`]), the ambiguity
/// warning (`[ref.any]`, fired even though the first candidate still
/// wins), the winner's class extension, then — on no candidates — the
/// `missing-reference` event (intersphinx; `builtin_resolver` never fires
/// for `any`, its reftype gate is `{class, obj, exc}`), the
/// self-referential retry, and the dangling warning (`:any:` is
/// `warn_dangling=True`).
#[allow(clippy::too_many_arguments)]
fn resolve_any_ref(
    resolver: &Resolver<'_>,
    nitpick: &NitpickConfig<'_>,
    docname: &str,
    query: &XrefQuery<'_>,
    ctx: PyRefContext<'_>,
    program: Option<&str>,
    children: XrefChildren,
    span: crate::doctree::Span,
    line: Option<usize>,
    path: &Path,
    out: &mut DocumentResolution,
) -> Vec<Node> {
    let contnode = children.contnode();
    let contnode_text = contnode.as_ref().map(Node::astext).unwrap_or_default();
    let req = XrefRequest {
        fromdoc: docname,
        refdoc: query.refdoc,
        reftype: "any",
        reftarget: query.reftarget,
        refexplicit: query.refexplicit,
        program,
        contnode_text: &contnode_text,
    };

    // One resolution attempt over a target (the self-referential retry runs
    // the same code over the stripped spelling, ambiguity warning included).
    let attempt = |target: &str, out: &mut DocumentResolution| -> Option<Node> {
        let mut warnings = Vec::new();
        let walk = resolver.resolve_any(
            &XrefRequest {
                reftarget: target,
                ..req
            },
            ctx.module,
            ctx.class,
            children.resolved.as_ref(),
            &mut warnings,
        );
        // What the walk's resolvers logged, at the reference: an `Invalid
        // math_eqref_format`, which has no `type`/`subtype`.
        for warning in warnings {
            out.warnings.push(
                BuildWarning::new(
                    path.to_path_buf(),
                    line,
                    warning,
                    WarningType::BrokenCrossReference,
                )
                .with_category(None),
            );
        }
        let mut results = match walk {
            Ok(results) => results,
            Err(crash) => {
                out.raise_at(crash, path, line);
                return None;
            }
        };
        if results.is_empty() {
            return None;
        }
        if results.len() > 1 {
            let candidates = results
                .iter()
                .map(|candidate| format!(":{}:`{}`", candidate.role, candidate.label))
                .collect::<Vec<_>>()
                .join(" or ");
            out.warnings.push(
                BuildWarning::new(
                    path.to_path_buf(),
                    line,
                    format!(
                        "more than one target found for 'any' cross-reference {}: \
                         could be {candidates}",
                        py_repr_str(target)
                    ),
                    WarningType::BrokenCrossReference,
                )
                // `type='ref', subtype='any'` (`:227-233`).
                .with_category(Some("ref.any".to_string())),
            );
        }
        let AnyCandidate { role, node, .. } = results.remove(0);
        let mut built = reference_node(node, children.contnode(), span);
        // 'Override "any" class with the actual role type' (`:236-247`):
        // the winner's first child — when it is an element that has classes
        // — gains `[domain, role.replace(':', '-')]`. Note `'doc'` has no
        // colon, so both halves are `doc` (probe: `classes="doc doc doc"`),
        // and a `std:ref` winner's fresh inline doubles up to
        // `std std-ref std std-ref`.
        if let Some(first) = built.children.first_mut() {
            if first.kind != kinds::TEXT && !first.attrs.classes.is_empty() {
                let domain_half = role.split(':').next().unwrap_or_default().to_string();
                first.attrs.classes.push(domain_half);
                first.attrs.classes.push(role.replace(':', "-"));
            }
        }
        Some(built)
    };

    if let Some(node) = attempt(query.reftarget, out) {
        return vec![node];
    }
    if out.crashed() {
        return Vec::new();
    }

    // The `missing-reference` event: intersphinx's handler resolves `any`
    // by sweeping every domain's objtypes.
    let mut diagnostics = Vec::new();
    let outcome = resolver.intersphinx.resolve_detect(query, &mut diagnostics);
    report(out, diagnostics, line, path);
    match outcome {
        HookOutcome::Resolved(resolution) => {
            return vec![intersphinx_node(resolution, contnode, span)];
        }
        HookOutcome::SelfReferential(stripped) => {
            if let Some(node) = attempt(&stripped, out) {
                return vec![node];
            }
            if out.crashed() {
                return Vec::new();
            }
        }
        HookOutcome::Missing => {}
    }

    if let Some(message) = missing_reference_warning(
        resolver.env,
        nitpick,
        "",
        "any",
        query.reftarget,
        ctx.refwarn,
    ) {
        out.warnings.push(
            BuildWarning::new(
                path.to_path_buf(),
                line,
                message,
                WarningType::BrokenCrossReference,
            )
            // `logger.warning(..., type='ref', subtype=typ)`.
            .with_category(Some("ref.any".to_string())),
        );
    }
    children.fallback(out, line, path)
}

/// `IntersphinxRoleResolver.run` (`ext/intersphinx/_resolve.py:543-565`),
/// plus the two checks Sphinx's `:external:` role makes at parse time and
/// this port defers to here (see
/// [`crate::rst::inline`]'s `emit_external_xref`): the inventory-existence
/// test comes first, then the role-name failure.
#[allow(clippy::too_many_arguments)]
fn resolve_external(
    resolver: &Resolver<'_>,
    query: &XrefQuery<'_>,
    inventory: Option<&str>,
    role_error: Option<&str>,
    contnode: Option<Node>,
    span: crate::doctree::Span,
    line: Option<usize>,
    path: &Path,
    out: &mut DocumentResolution,
) -> Vec<Node> {
    if let Some(inventory) = inventory {
        if let Some(diagnostic) =
            intersphinx::external_inventory_missing(resolver.intersphinx, inventory)
        {
            report(out, vec![diagnostic], line, path);
            // Sphinx's role returns `([], [])`: no reference, and no
            // content either.
            return Vec::new();
        }
    }
    if let Some(message) = role_error {
        report(
            out,
            vec![Diagnostic {
                message: message.to_string(),
                category: Some("intersphinx.external".to_string()),
            }],
            line,
            path,
        );
        return Vec::new();
    }

    let mut diagnostics = Vec::new();
    let resolution = match inventory {
        Some(inventory) => {
            resolver
                .intersphinx
                .resolve_in_inventory(inventory, query, &mut diagnostics)
        }
        // `resolve_reference_any_inventory(env, False, ...)`: an
        // `:external:` reference never honours the disabled reftypes.
        None => resolver
            .intersphinx
            .resolve_any(false, query, &mut diagnostics),
    };
    report(out, diagnostics, line, path);

    match resolution {
        Some(resolution) => vec![intersphinx_node(resolution, contnode, span)],
        None => {
            report(
                out,
                vec![intersphinx::external_not_found(query)],
                line,
                path,
            );
            contnode.into_iter().collect()
        }
    }
}

/// A `pending_xref`'s children, split the way `ReferencesResolver.run`
/// reads them (`post_transforms/__init__.py:66-92`): the content node comes
/// from the first non-empty `pending_xref_condition` matching `'resolved'`
/// then `'*'` (docutils truthiness — a childless condition node is falsy
/// and skipped), else from the node's own first child.
struct XrefChildren {
    contnode: Option<Node>,
    /// All children of the first non-empty `condition="resolved"` node —
    /// what a resolved py xref adopts in place of the contnode.
    resolved: Option<Vec<Node>>,
    /// All children of the first non-empty `condition="*"` node — what
    /// replaces the `pending_xref` when resolution fails.
    star: Option<Vec<Node>>,
    /// `isinstance(node[0], pending_xref_condition)`, which gates the
    /// failure fallback.
    first_is_condition: bool,
}

impl XrefChildren {
    /// SIMPLIFICATION, deliberate: `find` takes the first NON-EMPTY node
    /// with the wanted condition, where sphinx takes the first node with
    /// that condition and *then* tests its truthiness — so on a
    /// `[resolved(empty), resolved(full)]` sequence sphinx falls through
    /// to `'*'` and this returns the second `resolved`. The two agree
    /// wherever the nodes come from `type_to_xref`, which emits at most
    /// one condition of each kind and never an empty one (task 10), and
    /// nothing else in this crate builds `pending_xref_condition` nodes.
    /// Kept as-is because the faithful form needs a two-pass search for a
    /// shape the parser cannot produce.
    fn split(children: Vec<Node>) -> Self {
        let first_is_condition = children
            .first()
            .is_some_and(|child| child.kind == "pending_xref_condition");
        let find = |condition: &str| -> Option<Vec<Node>> {
            children
                .iter()
                .find(|child| {
                    child.kind == "pending_xref_condition"
                        && !child.children.is_empty()
                        && matches!(child.get("condition"),
                                    Some(AttrValue::Str(value)) if value == condition)
                })
                .map(|child| child.children.clone())
        };
        let resolved = find("resolved");
        let star = find("*");
        let contnode = resolved
            .as_ref()
            .or(star.as_ref())
            .map(|content| content[0].clone())
            // `contnode = node[0].deepcopy()` — which is the (childless)
            // condition node itself when conditions exist but are empty.
            .or_else(|| children.into_iter().next());
        XrefChildren {
            contnode,
            resolved,
            star,
            first_is_condition,
        }
    }

    /// Sphinx's `contnode` (a deepcopy — every use hands out a fresh clone).
    fn contnode(&self) -> Option<Node> {
        self.contnode.clone()
    }

    /// The nodes that replace a `pending_xref` whose resolution FAILED —
    /// returned None, as opposed to a Kept/builtin-silenced outcome, which
    /// keeps the plain contnode: the `'*'` condition's children when the
    /// node leads with a condition, else the contnode (`run()`, `:76-90`).
    fn fallback(self, out: &mut DocumentResolution, line: Option<usize>, path: &Path) -> Vec<Node> {
        if self.first_is_condition {
            if let Some(star) = self.star {
                return star;
            }
            out.warnings.push(
                BuildWarning::new(
                    path.to_path_buf(),
                    line,
                    "Could not determine the fallback text for the cross-reference. \
                     Might be a bug."
                        .to_string(),
                    WarningType::BrokenCrossReference,
                )
                // Plain `logger.warning(msg, location=node)` — no category.
                .with_category(None),
            );
        }
        self.contnode.into_iter().collect()
    }
}

/// Turn intersphinx diagnostics into build warnings at the reference's line.
fn report(
    out: &mut DocumentResolution,
    diagnostics: Vec<Diagnostic>,
    line: Option<usize>,
    path: &Path,
) {
    for diagnostic in diagnostics {
        out.warnings.push(
            BuildWarning::new(
                path.to_path_buf(),
                line,
                diagnostic.message,
                WarningType::BrokenCrossReference,
            )
            .with_category(diagnostic.category),
        );
    }
}

/// `_create_element_from_result`'s node (`_resolve.py:71-77`): an *external*
/// reference carrying the inventory's hover title, whose child is either the
/// content node as parsed or a fresh one of the same kind holding the
/// inventory's display name.
fn intersphinx_node(
    resolution: crate::intersphinx::Resolution,
    contnode: Option<Node>,
    span: crate::doctree::Span,
) -> Node {
    let mut node = Node::elem(kinds::REFERENCE, span);
    node.set("internal", AttrValue::Int(0));
    node.set("refuri", AttrValue::Str(resolution.refuri));
    node.set("reftitle", AttrValue::Str(resolution.reftitle));
    match resolution.title {
        // `contnode.__class__(title, title)` — the same node kind, with the
        // new text and none of the original's classes.
        Some(title) => {
            let kind = contnode.as_ref().map_or(kinds::LITERAL, |node| node.kind);
            let mut inner = Node::elem(kind, span);
            inner.children.push(Node::text_node(title, span));
            node.children.push(inner);
        }
        None => node.children.extend(contnode),
    }
    node
}

/// `ReferencesResolver.warn_missing_reference` (`:255-298`) plus the std
/// domain's `warn-missing-reference` handler (`std/__init__.py:1444-1461`).
/// `None` means "resolution failed silently", which is the default for
/// roles that are not `warn_dangling` outside nitpicky mode.
///
/// `refdomain` is `"py"`, `"citation"`, `"math"`, `"std"` or `""` (a
/// domainless std role). Sphinx's nitpick-ignore matching tries the bare
/// `(typ, target)` form ON TOP of `(domain:typ, target)` only "for 'std'
/// types" — `not domain or domain.name == 'std'` — so a `('func', 'x')`
/// entry does NOT silence a missing `:py:func:`x`` (probe-verified;
/// `('py:func', 'x')` does).
fn missing_reference_warning(
    env: &BuildEnvironment,
    nitpick: &NitpickConfig<'_>,
    refdomain: &str,
    typ: &str,
    target: &str,
    refwarn: bool,
) -> Option<String> {
    let std_types = refdomain.is_empty() || refdomain == "std";
    let mut warn = refwarn;
    if nitpick.nitpicky {
        warn = true;
        // `dtype = f'{domain.name}:{typ}' if domain else typ` — a
        // domainless node (`:any:`) has NO domain-qualified spelling.
        let dtype = if refdomain.is_empty() {
            typ.to_string()
        } else {
            format!("{refdomain}:{typ}")
        };
        let bare = std_types;
        let ignored =
            nitpick.ignore.iter().any(|(ityp, itarget)| {
                (ityp == &dtype || (bare && ityp == typ)) && itarget == target
            }) || nitpick.ignore_regex.iter().any(|(ityp, itarget)| {
                (full_match(ityp, &dtype) || (bare && full_match(ityp, typ)))
                    && full_match(itarget, target)
            });
        if ignored {
            warn = false;
        }
    }
    if !warn {
        return None;
    }

    // A non-std domain: the std domain's `warn-missing-reference` handler
    // declines it, so the message is the domain's `dangling_warnings`
    // entry for the role — the citation domain's `ref`
    // (`SP/sphinx/domains/citation.py:37-39`) and the math domain's `eq`
    // (`SP/sphinx/domains/math.py:55-57`), `%(target)s` — or else the
    // generic non-std branch (`post_transforms/__init__.py:290-295`). The
    // py domain defines no `dangling_warnings`, so every missing py ref
    // takes the generic shape.
    if !std_types {
        return Some(match (refdomain, typ) {
            ("citation", "ref") => format!("citation not found: {target}"),
            ("math", "eq") => format!("equation not found: {target}"),
            _ => format!("{refdomain}:{typ} reference target not found: {target}"),
        });
    }

    // `:ref:` goes through the std domain's event handler, which
    // distinguishes "no such label" from "label with no title".
    if typ == "ref" {
        return Some(if env.std.anonlabels.contains_key(target) {
            format!(
                "Failed to create a cross reference. A title or caption not found: {}",
                py_repr_str(target)
            )
        } else {
            format!("undefined label: {}", py_repr_str(target))
        });
    }
    // `domain.dangling_warnings` (`std/__init__.py:790-796`).
    let message = match typ {
        "term" => Some(format!("term not in glossary: {}", py_repr_str(target))),
        "numref" => Some(format!("undefined label: {}", py_repr_str(target))),
        "keyword" => Some(format!("unknown keyword: {}", py_repr_str(target))),
        "doc" => Some(format!("unknown document: {}", py_repr_str(target))),
        "option" => Some(format!("unknown option: {}", py_repr_str(target))),
        _ => None,
    };
    Some(message.unwrap_or_else(|| {
        // The generic fallback. Sphinx's other branch — `%s:%s reference
        // target not found` — is for non-std domains, which return before
        // reaching this function.
        format!("{} reference target not found: {target}", py_repr_str(typ))
    }))
}

/// Python `re.fullmatch`.
fn full_match(pattern: &str, text: &str) -> bool {
    regex::Regex::new(&format!("^(?:{pattern})$"))
        .map(|re| re.is_match(text))
        .unwrap_or(false)
}

fn attr_str<'a>(node: &'a Node, key: &'static str) -> Option<&'a str> {
    match node.get(key) {
        Some(AttrValue::Str(value)) => Some(value.as_str()),
        _ => None,
    }
}

/// Materialize a [`ResolvedXref`] as the doctree node it describes.
fn reference_node(
    resolved: ResolvedXref,
    contnode: Option<Node>,
    span: crate::doctree::Span,
) -> Node {
    let mut node = Node::elem(resolved.kind, span);
    node.set("internal", AttrValue::Int(1));
    if let Some(refid) = resolved.refid {
        node.set("refid", AttrValue::Str(refid));
    }
    if let Some(refuri) = resolved.refuri {
        node.set("refuri", AttrValue::Str(refuri));
    }
    if let Some(title) = resolved.title {
        node.set("title", AttrValue::Str(title));
    }
    if let Some(reftitle) = resolved.reftitle {
        node.set("reftitle", AttrValue::Str(reftitle));
    }
    match resolved.inner {
        Inner::Contnode => node.children.extend(contnode),
        Inner::Inline { text, classes } => {
            let mut inner = Node::elem("inline", span);
            inner.attrs.classes = classes;
            inner.children.push(Node::text_node(text, span));
            node.children.push(inner);
        }
        Inner::Text(text) => node.children.push(Node::text_node(text, span)),
        Inner::Children(children) => node.children.extend(children),
    }
    node
}

#[cfg(test)]
mod intersphinx_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// `ws_re.split(target, maxsplit=1)` — `\s+` is Python's `str.isspace`,
    /// so the `\x1f` an `OptionXRefRole` keeps in its reftarget folds a
    /// subcommand off exactly as a space does: `:option:`git\x1fadd -x``
    /// reaches `(git-add, -x)` under sphinx 9.1.0 (env oracle project
    /// `names_round_d`, panel fix round D).
    #[test]
    fn subcommand_folding_splits_on_python_whitespace() {
        assert_eq!(
            split_once_whitespace("git\x1fadd -x"),
            Some(("git", "add -x"))
        );
        assert_eq!(split_once_whitespace("add -x"), Some(("add", "-x")));
        assert_eq!(
            split_once_whitespace("git \x1f\t add"),
            Some(("git", "add"))
        );
        assert_eq!(split_once_whitespace("-x"), None);
    }

    /// No `intersphinx_mapping`: every hook is a no-op, which is the state
    /// every one of these tests (and every environment-oracle project) is
    /// in.
    static INERT: Intersphinx = Intersphinx {
        data: crate::intersphinx::IntersphinxData {
            main: crate::inventory::Inventory {
                data: BTreeMap::new(),
            },
            named: BTreeMap::new(),
        },
        disabled_reftypes: std::collections::BTreeSet::new(),
        resolve_self: String::new(),
    };

    fn env_with_label() -> BuildEnvironment {
        let mut env = BuildEnvironment::default();
        env.std.labels.insert(
            "the-label".to_string(),
            (
                "a".to_string(),
                "the-label".to_string(),
                "The Section".to_string(),
            ),
        );
        env.std.anonlabels.insert(
            "the-label".to_string(),
            ("a".to_string(), "the-label".to_string()),
        );
        env.all_docs.insert("a".to_string(), 0);
        env.all_docs.insert("b".to_string(), 0);
        env
    }

    fn resolver<'a>(
        env: &'a BuildEnvironment,
        numfig_format: &'a BTreeMap<String, String>,
    ) -> Resolver<'a> {
        Resolver {
            env,
            numfig: true,
            numfig_format,
            math: crate::env::math::MathConfig::default(),
            doctree: &|_| None,
            relative_uri: &|_, _| String::new(),
            intersphinx: &INERT,
        }
    }

    fn request<'a>(fromdoc: &'a str, reftype: &'a str, reftarget: &'a str) -> XrefRequest<'a> {
        XrefRequest {
            fromdoc,
            refdoc: fromdoc,
            reftype,
            reftarget,
            refexplicit: false,
            program: None,
            contnode_text: reftarget,
        }
    }

    /// The `.. program::` in scope where an `:option:` was *written* is the
    /// first key `_resolve_option_xref` tries — which is what
    /// `pending_xref['std:program']` carries, and why reading that attribute
    /// has to strip docutils' `None` rendering first (a literal `"True"`
    /// program name would miss every registration).
    #[test]
    fn an_option_resolves_against_the_program_in_scope_where_it_was_written() {
        let mut env = BuildEnvironment::default();
        env.std
            .add_program_option(Some("myprog"), "--verbose", "a", "cmdoption-myprog-verbose");
        env.all_docs.insert("a".to_string(), 0);
        let formats = BTreeMap::new();
        let resolver = resolver(&env, &formats);

        let mut scoped = request("a", "option", "--verbose");
        scoped.program = Some("myprog");
        assert_eq!(
            resolver.resolve_xref(&scoped),
            XrefOutcome::Resolved(ResolvedXref {
                kind: kinds::REFERENCE,
                refid: Some("cmdoption-myprog-verbose".to_string()),
                refuri: None,
                title: None,
                reftitle: None,
                inner: Inner::Contnode,
            })
        );

        // Same target with no program in scope: only the word-folding
        // fallback could save it, and `--verbose` has no leading command.
        assert_eq!(
            resolver.resolve_xref(&request("a", "option", "--verbose")),
            XrefOutcome::Missing
        );
    }

    #[test]
    fn a_same_document_ref_uses_refid_and_a_cross_document_one_uses_refuri() {
        let env = env_with_label();
        let formats = BTreeMap::new();
        let resolver = resolver(&env, &formats);

        let same = resolver.resolve_xref(&request("a", "ref", "the-label"));
        assert_eq!(
            same,
            XrefOutcome::Resolved(ResolvedXref {
                kind: kinds::REFERENCE,
                refid: Some("the-label".to_string()),
                refuri: None,
                title: None,
                reftitle: None,
                inner: Inner::Inline {
                    text: "The Section".to_string(),
                    classes: vec!["std".to_string(), "std-ref".to_string()],
                },
            })
        );

        let cross = resolver.resolve_xref(&request("b", "ref", "the-label"));
        let XrefOutcome::Resolved(cross) = cross else {
            panic!("expected a resolved reference, got {cross:?}")
        };
        assert_eq!(cross.refuri.as_deref(), Some("#the-label"));
        assert_eq!(cross.refid, None);
    }

    #[test]
    fn an_explicit_ref_titles_itself_from_the_anonymous_label() {
        let env = env_with_label();
        let formats = BTreeMap::new();
        let resolver = resolver(&env, &formats);
        let mut req = request("a", "ref", "the-label");
        req.refexplicit = true;
        req.contnode_text = "My Own Words";

        let XrefOutcome::Resolved(resolved) = resolver.resolve_xref(&req) else {
            panic!("expected a resolved reference")
        };
        assert_eq!(
            resolved.inner,
            Inner::Inline {
                text: "My Own Words".to_string(),
                classes: vec!["std".to_string(), "std-ref".to_string()],
            }
        );
    }

    #[test]
    fn a_doc_reference_joins_the_target_against_the_referencing_document() {
        let mut env = BuildEnvironment::default();
        env.all_docs.insert("sub/c".to_string(), 0);
        let mut title = Node::elem(kinds::TITLE, crate::doctree::Span::ZERO);
        title
            .children
            .push(Node::text_node("Sub C", crate::doctree::Span::ZERO));
        env.titles.insert("sub/c".to_string(), title);
        let formats = BTreeMap::new();
        let resolver = resolver(&env, &formats);

        let relative = resolver.resolve_xref(&request("sub/b", "doc", "c"));
        assert_eq!(
            relative,
            XrefOutcome::Resolved(ResolvedXref {
                kind: kinds::REFERENCE,
                refid: None,
                refuri: Some(String::new()),
                title: None,
                reftitle: None,
                inner: Inner::Inline {
                    text: "Sub C".to_string(),
                    classes: vec!["doc".to_string()],
                },
            }),
            "the caption comes from the target's title, not the written target"
        );
        assert_eq!(
            resolver.resolve_xref(&request("sub/b", "doc", "/sub/c")),
            relative,
            "an absolute target names the same document"
        );
        assert_eq!(
            resolver.resolve_xref(&request("sub/b", "doc", "nope")),
            XrefOutcome::Missing
        );
    }

    #[test]
    fn option_resolution_folds_leading_words_into_the_program_name() {
        let mut env = BuildEnvironment::default();
        env.std
            .add_program_option(Some("myprog"), "--verbose", "a", "cmdoption-myprog-verbose");
        env.std
            .add_program_option(None, "--global", "a", "cmdoption-global");
        let formats = BTreeMap::new();
        let resolver = resolver(&env, &formats);

        let XrefOutcome::Resolved(scoped) =
            resolver.resolve_xref(&request("b", "option", "myprog --verbose"))
        else {
            panic!("`myprog --verbose` must resolve through the program fallback")
        };
        assert_eq!(scoped.refuri.as_deref(), Some("#cmdoption-myprog-verbose"));

        let XrefOutcome::Resolved(global) =
            resolver.resolve_xref(&request("b", "option", "--global"))
        else {
            panic!("an unscoped option resolves under the `None` program")
        };
        assert_eq!(global.refuri.as_deref(), Some("#cmdoption-global"));

        assert_eq!(
            resolver.resolve_xref(&request("b", "option", "--missing")),
            XrefOutcome::Missing
        );
    }

    #[test]
    fn option_resolution_strips_an_option_value() {
        let mut env = BuildEnvironment::default();
        env.std
            .add_program_option(None, "-foo", "a", "cmdoption-foo");
        let formats = BTreeMap::new();
        let resolver = resolver(&env, &formats);
        for target in ["-foo=bar", "-foo[=bar]"] {
            let outcome = resolver.resolve_xref(&request("b", "option", target));
            assert!(
                matches!(outcome, XrefOutcome::Resolved(_)),
                "{target} must fall back to the option stem, got {outcome:?}"
            );
        }
    }

    #[test]
    fn term_resolution_falls_back_to_a_case_insensitive_match() {
        let mut env = BuildEnvironment::default();
        env.std.note_term("environment", "a", "term-environment");
        let formats = BTreeMap::new();
        let resolver = resolver(&env, &formats);

        let exact = resolver.resolve_xref(&request("b", "term", "environment"));
        let other_case = resolver.resolve_xref(&request("b", "term", "Environment"));
        assert!(matches!(exact, XrefOutcome::Resolved(_)));
        assert_eq!(exact, other_case);
        assert_eq!(
            resolver.resolve_xref(&request("b", "term", "nonexistent term")),
            XrefOutcome::Missing
        );
    }

    #[test]
    fn numfig_off_keeps_the_content_node_and_says_so_once() {
        let mut env = BuildEnvironment::default();
        env.std.labels.insert(
            "fig-a".to_string(),
            ("a".to_string(), "fig-a".to_string(), "A Figure".to_string()),
        );
        let mut figure = Node::elem("figure", crate::doctree::Span::ZERO);
        figure.attrs.ids.push("fig-a".to_string());
        let mut root = Node::elem(kinds::DOCUMENT, crate::doctree::Span::ZERO);
        root.children.push(figure);
        let doctree = Doctree {
            root,
            sources: vec!["<test>".to_string()],
        };
        let formats = BTreeMap::new();
        let resolver = Resolver {
            env: &env,
            numfig: false,
            numfig_format: &formats,
            math: crate::env::math::MathConfig::default(),
            doctree: &|_| Some(Cow::Borrowed(&doctree)),
            relative_uri: &|_, _| String::new(),
            intersphinx: &INERT,
        };

        assert_eq!(
            resolver.resolve_xref(&request("b", "numref", "fig-a")),
            XrefOutcome::Kept {
                warning: Some("numfig is disabled. :numref: is ignored.".to_string())
            }
        );
    }

    /// The two ways a `{name}` can have nothing to fill it: no label entry
    /// at all (`figname is None` — "the link has no caption"), and a label
    /// whose section name is empty, which Sphinx's truthiness test sends
    /// down the `format(number=...)` path and straight into a `KeyError`.
    #[test]
    fn a_nameless_numref_target_reports_the_format_it_could_not_fill() {
        let mut env = BuildEnvironment::default();
        env.std.labels.insert(
            "captionless".to_string(),
            ("a".to_string(), "captionless".to_string(), String::new()),
        );
        env.std
            .anonlabels
            .insert("anon".to_string(), ("a".to_string(), "anon".to_string()));
        env.toc_fignumbers.insert(
            "a".to_string(),
            BTreeMap::from([(
                "figure".to_string(),
                BTreeMap::from([
                    ("captionless".to_string(), vec![1]),
                    ("anon".to_string(), vec![2]),
                ]),
            )]),
        );
        let mut root = Node::elem(kinds::DOCUMENT, crate::doctree::Span::ZERO);
        for id in ["captionless", "anon"] {
            let mut figure = Node::elem("figure", crate::doctree::Span::ZERO);
            figure.attrs.ids.push(id.to_string());
            root.children.push(figure);
        }
        let doctree = Doctree {
            root,
            sources: vec!["<test>".to_string()],
        };
        let formats = BTreeMap::from([("figure".to_string(), "Fig. {name} {number}".to_string())]);
        let resolver = Resolver {
            env: &env,
            numfig: true,
            numfig_format: &formats,
            math: crate::env::math::MathConfig::default(),
            doctree: &|_| Some(Cow::Borrowed(&doctree)),
            relative_uri: &|_, _| String::new(),
            intersphinx: &INERT,
        };

        assert_eq!(
            resolver.resolve_xref(&request("b", "numref", "anon")),
            XrefOutcome::Kept {
                warning: Some("the link has no caption: Fig. {name} {number}".to_string())
            },
            "an anonymous-only label has no caption to name"
        );
        assert_eq!(
            resolver.resolve_xref(&request("b", "numref", "captionless")),
            XrefOutcome::Kept {
                warning: Some(
                    "invalid numfig_format: Fig. {name} {number} (KeyError('name'))".to_string()
                )
            },
            "an empty caption is falsy, so `name` is never passed to format()"
        );
    }

    /// A label on a real figure that numbering never reached — an orphaned
    /// document's, say — is `get_fignumber`'s `ValueError`.
    #[test]
    fn a_numref_target_with_no_number_names_the_label_it_could_not_number() {
        let mut env = BuildEnvironment::default();
        env.std.labels.insert(
            "fig-a".to_string(),
            ("a".to_string(), "fig-a".to_string(), "A Figure".to_string()),
        );
        let mut figure = Node::elem("figure", crate::doctree::Span::ZERO);
        figure.attrs.ids.push("fig-a".to_string());
        let mut root = Node::elem(kinds::DOCUMENT, crate::doctree::Span::ZERO);
        root.children.push(figure);
        let doctree = Doctree {
            root,
            sources: vec!["<test>".to_string()],
        };
        let formats = BTreeMap::from([("figure".to_string(), "Fig. %s".to_string())]);
        let resolver = Resolver {
            env: &env,
            numfig: true,
            numfig_format: &formats,
            math: crate::env::math::MathConfig::default(),
            doctree: &|_| Some(Cow::Borrowed(&doctree)),
            relative_uri: &|_, _| String::new(),
            intersphinx: &INERT,
        };

        assert_eq!(
            resolver.resolve_xref(&request("b", "numref", "fig-a")),
            XrefOutcome::Kept {
                warning: Some(
                    "Failed to create a cross reference. Any number is not assigned: fig-a"
                        .to_string()
                )
            }
        );
    }

    #[test]
    fn numref_renders_both_format_styles_and_reports_broken_ones() {
        let format_new_style = |title: &str, figname: Option<&str>, fignum: &str| match figname {
            Some(figname) => pyformat::format(
                title,
                &[
                    ("name", Value::Str(figname)),
                    ("number", Value::Str(fignum)),
                ],
            ),
            None => pyformat::format(title, &[("number", Value::Str(fignum))]),
        };
        assert_eq!(pyformat::percent("Fig. %s", "1.2").unwrap(), "Fig. 1.2");
        assert!(
            matches!(
                pyformat::percent("Fig.", "1"),
                Err(FormatError::TypeError(_))
            ),
            "no conversion: TypeError"
        );
        assert!(
            matches!(
                pyformat::percent("%s %s", "1"),
                Err(FormatError::TypeError(_))
            ),
            "two conversions for one argument: TypeError"
        );
        assert_eq!(
            format_new_style("Custom {name} number {number}", Some("Cap"), "1").unwrap(),
            "Custom Cap number 1"
        );
        assert_eq!(
            format_new_style("Table {number}", None, "3").unwrap(),
            "Table 3"
        );
        assert!(matches!(
            format_new_style("{nope}", Some("Cap"), "1"),
            Err(FormatError::KeyError(key)) if key == "nope"
        ));
        // `{{` and `}}` are a literal brace, as Python's `str.format`
        // reads them.
        for (title, figname, expected) in [
            ("Tab. {{x}} {number}", None, "Tab. {x} 1"),
            ("Pic {{n}} {number}}}", None, "Pic {n} 1}"),
            (
                "Custom {{name}} {name} {number}",
                Some("Cap"),
                "Custom {name} Cap 1",
            ),
            ("}}{{", None, "}{"),
        ] {
            assert_eq!(
                format_new_style(title, figname, "1").unwrap(),
                expected,
                "{title}"
            );
        }
        assert!(matches!(
            format_new_style("{nope}}}", None, "1"),
            Err(FormatError::KeyError(key)) if key == "nope"
        ));
    }

    /// `_resolve_numref_xref` formats a `numfig_format` and an explicit
    /// title with `str.format`, which reads `{{` and `}}` as a literal brace
    /// (`SP/sphinx/domains/std/__init__.py:1087-1170`). Probed on Sphinx
    /// 9.1.0: `numfig_format = {'table': 'Tab. {{x}} {number}'}` and
    /// ``:numref:`Pic {{n}} {number}}} <tbl>` `` give `Tab. {x} 1` and
    /// `Pic {n} 1}`, with no warning.
    #[test]
    fn doubled_braces_in_a_numref_format_are_literal_braces() {
        let mut env = BuildEnvironment::default();
        env.std.labels.insert(
            "tbl".to_string(),
            (
                "index".to_string(),
                "tbl".to_string(),
                "The caption.".to_string(),
            ),
        );
        env.toc_fignumbers.insert(
            "index".to_string(),
            BTreeMap::from([(
                "table".to_string(),
                BTreeMap::from([("tbl".to_string(), vec![1])]),
            )]),
        );
        let mut table = Node::elem(kinds::TABLE, crate::doctree::Span::ZERO);
        table.attrs.ids.push("tbl".to_string());
        let mut root = Node::elem(kinds::DOCUMENT, crate::doctree::Span::ZERO);
        root.children.push(table);
        let doctree = Doctree {
            root,
            sources: vec!["<test>".to_string()],
        };
        let formats = BTreeMap::from([("table".to_string(), "Tab. {{x}} {number}".to_string())]);
        let resolver = Resolver {
            env: &env,
            numfig: true,
            numfig_format: &formats,
            math: crate::env::math::MathConfig::default(),
            doctree: &|_| Some(Cow::Borrowed(&doctree)),
            relative_uri: &|_, _| String::new(),
            intersphinx: &INERT,
        };
        let numbered = |title: Option<&str>, text: &str| {
            XrefOutcome::Resolved(ResolvedXref {
                kind: "number_reference",
                refid: Some("tbl".to_string()),
                refuri: None,
                title: title.map(str::to_string),
                reftitle: None,
                inner: Inner::Inline {
                    text: text.to_string(),
                    classes: vec!["std".to_string(), "std-numref".to_string()],
                },
            })
        };

        assert_eq!(
            resolver.resolve_xref(&request("index", "numref", "tbl")),
            numbered(Some("Tab. {{x}} {number}"), "Tab. {x} 1")
        );
        let mut explicit = request("index", "numref", "tbl");
        explicit.refexplicit = true;
        explicit.contnode_text = "Pic {{n}} {number}}}";
        assert_eq!(
            resolver.resolve_xref(&explicit),
            numbered(Some("Pic {{n}} {number}}}"), "Pic {n} 1}")
        );
    }

    /// `_resolve_numref_xref` formats the rest of a field on the string its
    /// keyword gives ([`pyformat::format`]): an index of digits fills the
    /// character; a name in brackets raises the `TypeError` the resolver
    /// catches, which warns `invalid numfig_format: <title>` without the
    /// exception; and what else `str.format` raises but `KeyError` it does
    /// not catch, so Sphinx 9.1.0 crashes (decision 13). Probed on Sphinx
    /// 9.1.0 (`a_format_field_indexes_a_string_as_python_does`,
    /// `invalid_new_style_format_exits_two`).
    #[test]
    fn a_numref_format_looks_up_the_rest_of_a_field_on_the_string() {
        let mut env = BuildEnvironment::default();
        env.std.labels.insert(
            "tbl".to_string(),
            ("index".to_string(), "tbl".to_string(), "Cap".to_string()),
        );
        env.toc_fignumbers.insert(
            "index".to_string(),
            BTreeMap::from([(
                "table".to_string(),
                BTreeMap::from([("tbl".to_string(), vec![1])]),
            )]),
        );
        let mut table = Node::elem(kinds::TABLE, crate::doctree::Span::ZERO);
        table.attrs.ids.push("tbl".to_string());
        let mut root = Node::elem(kinds::DOCUMENT, crate::doctree::Span::ZERO);
        root.children.push(table);
        let doctree = Doctree {
            root,
            sources: vec!["<test>".to_string()],
        };
        let formats = BTreeMap::from([("table".to_string(), "Table {number.}".to_string())]);
        let resolver = Resolver {
            env: &env,
            numfig: true,
            numfig_format: &formats,
            math: crate::env::math::MathConfig::default(),
            doctree: &|_| Some(Cow::Borrowed(&doctree)),
            relative_uri: &|_, _| String::new(),
            intersphinx: &INERT,
        };
        let titled = |title: &'static str| XrefRequest {
            refexplicit: true,
            contnode_text: title,
            ..request("index", "numref", "tbl")
        };
        let crash = |exception: &'static str, message: &str| {
            XrefOutcome::Crash(SphinxCrash::python(
                exception,
                message.to_string(),
                crate::rst::LogLocation::None,
            ))
        };

        assert_eq!(
            resolver.resolve_xref(&titled("Tab {name[0]}{number[0]}")),
            XrefOutcome::Resolved(ResolvedXref {
                kind: "number_reference",
                refid: Some("tbl".to_string()),
                refuri: None,
                title: Some("Tab {name[0]}{number[0]}".to_string()),
                reftitle: None,
                inner: Inner::Inline {
                    text: "Tab C1".to_string(),
                    classes: vec!["std".to_string(), "std-numref".to_string()],
                },
            })
        );
        // An index of decimal digits of any script (ARABIC-INDIC DIGIT ONE
        // and ZERO) is one too.
        assert_eq!(
            resolver.resolve_xref(&titled("Tab {name[\u{661}]}{number[\u{660}]}")),
            XrefOutcome::Resolved(ResolvedXref {
                kind: "number_reference",
                refid: Some("tbl".to_string()),
                refuri: None,
                title: Some("Tab {name[\u{661}]}{number[\u{660}]}".to_string()),
                reftitle: None,
                inner: Inner::Inline {
                    text: "Tab a1".to_string(),
                    classes: vec!["std".to_string(), "std-numref".to_string()],
                },
            })
        );
        assert_eq!(
            resolver.resolve_xref(&titled("Tab {name[x]} {number}")),
            XrefOutcome::Kept {
                warning: Some("invalid numfig_format: Tab {name[x]} {number}".to_string())
            }
        );
        assert_eq!(
            resolver.resolve_xref(&titled("Tab {name[3]} {number}")),
            crash("IndexError", "string index out of range")
        );
        // ARABIC-INDIC DIGIT THREE is the positional field 3, and twenty
        // ARABIC-INDIC DIGIT NINEs an index past `isize::MAX`.
        assert_eq!(
            resolver.resolve_xref(&titled("Tab {\u{663}} {number}")),
            crash(
                "IndexError",
                "Replacement index 3 out of range for positional args tuple"
            )
        );
        let many_digits = format!("Tab {{name[{}]}} {{number}}", "\u{669}".repeat(20));
        assert_eq!(
            resolver.resolve_xref(&XrefRequest {
                refexplicit: true,
                contnode_text: &many_digits,
                ..request("index", "numref", "tbl")
            }),
            crash("ValueError", "Too many decimal digits in format string")
        );
        assert_eq!(
            resolver.resolve_xref(&titled("Tab {name.real} {number}")),
            crash("AttributeError", "'str' object has no attribute 'real'")
        );
        assert_eq!(
            resolver.resolve_xref(&request("index", "numref", "tbl")),
            crash("ValueError", "Empty attribute in format string"),
            "numfig_format"
        );
    }

    #[test]
    fn dangling_warnings_use_the_exact_sphinx_texts() {
        let env = env_with_label();
        let nitpick = NitpickConfig {
            nitpicky: false,
            ignore: &[],
            ignore_regex: &[],
        };
        let warn = |typ: &str, target: &str| {
            missing_reference_warning(&env, &nitpick, "std", typ, target, true)
        };
        assert_eq!(
            warn("doc", "missing-doc").unwrap(),
            "unknown document: 'missing-doc'"
        );
        assert_eq!(
            warn("term", "nonexistent term").unwrap(),
            "term not in glossary: 'nonexistent term'"
        );
        assert_eq!(warn("option", "--x").unwrap(), "unknown option: '--x'");
        assert_eq!(warn("keyword", "k").unwrap(), "unknown keyword: 'k'");
        assert_eq!(warn("numref", "fig").unwrap(), "undefined label: 'fig'");
        assert_eq!(warn("ref", "nope").unwrap(), "undefined label: 'nope'");
        assert_eq!(
            warn("ref", "the-label").unwrap(),
            "Failed to create a cross reference. A title or caption not found: 'the-label'",
            "a label that exists but has no title takes the other branch"
        );
        assert_eq!(
            warn("envvar", "PATH").unwrap(),
            "'envvar' reference target not found: PATH",
            "a role with no dangling_warnings entry takes the generic form"
        );
    }

    #[test]
    fn a_role_that_is_not_warn_dangling_only_warns_under_nitpicky() {
        let env = env_with_label();
        let quiet = NitpickConfig {
            nitpicky: false,
            ignore: &[],
            ignore_regex: &[],
        };
        assert_eq!(
            missing_reference_warning(&env, &quiet, "std", "envvar", "PATH", false),
            None
        );
        let nitpicky = NitpickConfig {
            nitpicky: true,
            ignore: &[],
            ignore_regex: &[],
        };
        assert!(
            missing_reference_warning(&env, &nitpicky, "std", "envvar", "PATH", false).is_some()
        );
    }

    // ---- the :any: candidate walk (`_resolve_pending_any_xref`) ----------

    /// Candidate labels for the walk over one request.
    fn any_roles(resolver: &Resolver<'_>, req: &XrefRequest<'_>) -> Vec<(String, String)> {
        resolver
            .resolve_any(req, None, None, None, &mut Vec::new())
            .expect("the walk does not crash")
            .into_iter()
            .map(|candidate| (candidate.role, candidate.label))
            .collect()
    }

    /// The std half's candidate sets, in walk order: `:doc:` first (role
    /// `'doc'`, unprefixed), then `'ref'` over the LOWERCASED target, then
    /// `'option'`, then the objects table in `object_types` order.
    #[test]
    fn any_walks_doc_then_ref_then_option_then_the_objects_table() {
        let mut env = BuildEnvironment::default();
        env.all_docs.insert("same".to_string(), 0);
        let mut title = Node::elem(kinds::TITLE, crate::doctree::Span::ZERO);
        title
            .children
            .push(Node::text_node("Doc Title", crate::doctree::Span::ZERO));
        env.titles.insert("same".to_string(), title);
        env.std.labels.insert(
            "same".to_string(),
            ("a".to_string(), "same".to_string(), "Sect".to_string()),
        );
        env.std.note_object("envvar", "same", "a", "envvar-same");
        env.py.note_object(
            "m.same",
            crate::env::py_domain::PyObjectEntry {
                docname: "a".to_string(),
                node_id: "m.same".to_string(),
                objtype: "function".to_string(),
                aliased: false,
            },
        );
        let formats = BTreeMap::new();
        let resolver = resolver(&env, &formats);

        let req = request("a", "any", "same");
        assert_eq!(
            any_roles(&resolver, &req),
            vec![
                ("doc".to_string(), "Doc Title".to_string()),
                ("std:ref".to_string(), "Sect".to_string()),
                // make_refnode keeps the contnode, so the label is its text.
                ("std:envvar".to_string(), "same".to_string()),
                // py candidates label with the make_refnode reftitle.
                ("py:func".to_string(), "m.same".to_string()),
            ]
        );
    }

    /// Only the `'ref'` arm lowercases; the objects walk lowercases the
    /// TERM key alone — so a glossary term registered with an uppercase
    /// letter is unreachable through `:any:` under either spelling
    /// (probe: `:any:`Aterm`` and `:any:`aterm`` both dangle).
    #[test]
    fn any_lowercases_the_ref_arm_and_the_term_key_only() {
        let mut env = BuildEnvironment::default();
        env.std.labels.insert(
            "mixed".to_string(),
            ("a".to_string(), "mixed".to_string(), "Sect".to_string()),
        );
        env.std.note_term("Aterm", "a", "term-Aterm");
        env.std.note_term("bterm", "a", "term-bterm");
        let formats = BTreeMap::new();
        let resolver = resolver(&env, &formats);

        assert_eq!(
            any_roles(&resolver, &request("a", "any", "MIXED"))
                .iter()
                .map(|(role, _)| role.as_str())
                .collect::<Vec<_>>(),
            vec!["std:ref"],
            "the ref arm sees the lowercased target"
        );
        assert!(
            any_roles(&resolver, &request("a", "any", "Aterm")).is_empty(),
            "objects holds ('term', 'Aterm') but the walk asks for ('term', 'aterm')"
        );
        assert!(
            any_roles(&resolver, &request("a", "any", "aterm")).is_empty(),
            "and 'aterm' was never registered"
        );
        assert_eq!(
            any_roles(&resolver, &request("a", "any", "BTERM"))
                .iter()
                .map(|(role, _)| role.as_str())
                .collect::<Vec<_>>(),
            vec!["std:term"],
            "a lowercase-registered term is reachable under any case"
        );
    }

    /// A module candidate's label is the full `_make_module_refnode`
    /// reftitle — the ambiguity warning renders it verbatim.
    #[test]
    fn any_module_candidates_label_with_the_synopsis_reftitle() {
        let mut env = BuildEnvironment::default();
        env.py.note_object(
            "syn",
            crate::env::py_domain::PyObjectEntry {
                docname: "a".to_string(),
                node_id: "module-syn".to_string(),
                objtype: "module".to_string(),
                aliased: false,
            },
        );
        env.py.note_module(
            "syn",
            crate::env::py_domain::PyModuleEntry {
                docname: "a".to_string(),
                node_id: "module-syn".to_string(),
                synopsis: "The syn module.".to_string(),
                platform: String::new(),
                deprecated: false,
            },
        );
        let formats = BTreeMap::new();
        let resolver = resolver(&env, &formats);
        assert_eq!(
            any_roles(&resolver, &request("a", "any", "syn")),
            vec![("py:mod".to_string(), "syn: The syn module.".to_string())]
        );
    }

    #[test]
    fn a_missing_any_reference_warns_with_the_domainless_spelling() {
        let env = BuildEnvironment::default();
        let quiet = NitpickConfig {
            nitpicky: false,
            ignore: &[],
            ignore_regex: &[],
        };
        assert_eq!(
            missing_reference_warning(&env, &quiet, "", "any", "missing_thing", true).unwrap(),
            "'any' reference target not found: missing_thing"
        );
        // Nitpick-ignore matches the BARE ('any', target) pair — there is
        // no domain-qualified spelling for a domainless node.
        let ignore = vec![("any".to_string(), "missing_thing".to_string())];
        let nitpicky = NitpickConfig {
            nitpicky: true,
            ignore: &ignore,
            ignore_regex: &[],
        };
        assert_eq!(
            missing_reference_warning(&env, &nitpicky, "", "any", "missing_thing", true),
            None
        );
    }

    #[test]
    fn nitpick_ignore_filters_by_exact_pair_and_by_regex() {
        let env = env_with_label();
        let exact = vec![("std:doc".to_string(), "missing".to_string())];
        let config = NitpickConfig {
            nitpicky: true,
            ignore: &exact,
            ignore_regex: &[],
        };
        assert_eq!(
            missing_reference_warning(&env, &config, "std", "doc", "missing", true),
            None
        );
        assert!(missing_reference_warning(&env, &config, "std", "doc", "other", true).is_some());

        // The domainless form is accepted for std types too.
        let domainless = vec![("doc".to_string(), "missing".to_string())];
        let config = NitpickConfig {
            nitpicky: true,
            ignore: &domainless,
            ignore_regex: &[],
        };
        assert_eq!(
            missing_reference_warning(&env, &config, "std", "doc", "missing", true),
            None
        );

        let regex = vec![("std:.*".to_string(), "miss.*".to_string())];
        let config = NitpickConfig {
            nitpicky: true,
            ignore: &[],
            ignore_regex: &regex,
        };
        assert_eq!(
            missing_reference_warning(&env, &config, "std", "doc", "missing", true),
            None
        );
        assert!(
            missing_reference_warning(&env, &config, "std", "doc", "hit", true).is_some(),
            "the regexes must both full-match, not merely find"
        );
    }
}
