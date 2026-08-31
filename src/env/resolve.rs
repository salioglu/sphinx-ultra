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
use std::path::Path;

use crate::doctree::{kinds, AttrValue, Doctree, Node};
use crate::env::numbers::clean_astext;
use crate::env::std_domain::{node_line, DocumentIds};
use crate::env::toctree::{docname_join, py_repr_str};
use crate::env::BuildEnvironment;
use crate::error::{BuildWarning, WarningType};
use crate::intersphinx::{self, Diagnostic, HookOutcome, Intersphinx, XrefQuery};

/// One `pending_xref` to resolve — the attributes Sphinx's resolvers read
/// off the node.
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
    pub inner: Inner,
}

/// The reference's child node.
#[derive(Debug, PartialEq)]
pub enum Inner {
    /// Sphinx's `contnode`: whatever the parse layer produced, reused
    /// verbatim (`make_refnode(..., contnode)`).
    Contnode,
    /// A fresh `inline` node (`build_reference_node`, and the `:doc:`
    /// caption).
    Inline { text: String, classes: Vec<String> },
}

/// Everything resolution reads: the environment, the numbering
/// configuration, the other documents' doctrees, and the builder's URI
/// policy.
pub struct Resolver<'a> {
    pub env: &'a BuildEnvironment,
    pub numfig: bool,
    pub numfig_format: &'a BTreeMap<String, String>,
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
        let newtitle = if title.contains("{name}") || title.contains("number") {
            // New style (`Fig.{number}`). Sphinx passes `name` to `format`
            // only `if figname:` — a *truthiness* test, so an empty caption
            // is formatted without it, and a `{name}` in the title then
            // raises the KeyError below (the `figname is None` guard above
            // is the only None-ness test in this algorithm).
            let named = figname.as_deref().filter(|figname| !figname.is_empty());
            match format_new_style(&title, named, &fignum) {
                Ok(newtitle) => newtitle,
                Err(KeyError(key)) => {
                    return XrefOutcome::Kept {
                        warning: Some(format!(
                            "invalid numfig_format: {title} (KeyError({}))",
                            py_repr_str(&key)
                        )),
                    }
                }
            }
        } else {
            // Old style (`Fig.%s`).
            match format_old_style(&title, &fignum) {
                Ok(newtitle) => newtitle,
                Err(TypeError) => {
                    return XrefOutcome::Kept {
                        warning: Some(format!("invalid numfig_format: {title}")),
                    }
                }
            }
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

/// The label a reference resolved to, as `build_reference_node` takes it.
struct LabelTarget<'a> {
    fromdoc: &'a str,
    docname: &'a str,
    labelid: &'a str,
}

/// Sphinx's `ValueError` out of `get_fignumber`.
#[derive(Debug)]
struct NoNumber;

/// Python's `KeyError` out of `str.format`, carrying the missing field.
#[derive(Debug)]
struct KeyError(String);

/// Python's `TypeError` out of `%`-formatting.
#[derive(Debug)]
struct TypeError;

/// `title.format(name=..., number=...)` for the fields numfig formats can
/// name. Any other `{field}` is Python's `KeyError`.
fn format_new_style(title: &str, figname: Option<&str>, fignum: &str) -> Result<String, KeyError> {
    let mut out = String::with_capacity(title.len());
    let mut rest = title;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            // An unbalanced `{` is a ValueError in Python; Sphinx does not
            // catch it. Ours keeps the text as written rather than crashing
            // the build.
            out.push_str(&rest[open..]);
            return Ok(out);
        };
        let field = &after[..close];
        match field {
            // `title.format(number=fignum)` is called *without* `name` when
            // there is no figname, so `{name}` is a KeyError then.
            "name" => match figname {
                Some(figname) => out.push_str(figname),
                None => return Err(KeyError("name".to_string())),
            },
            "number" => out.push_str(fignum),
            other => return Err(KeyError(other.to_string())),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// `title % fignum` for a single string argument: exactly one `%s`
/// conversion, or Python raises `TypeError` — too few ("not enough
/// arguments") and too many ("not all arguments converted") both land on
/// the same warning. Any other conversion is reported as an invalid format
/// too; `%r` would in fact work in Python, but no `numfig_format` uses it
/// and guessing at the rest of `%`-formatting would be worse than saying
/// the format is unusable.
fn format_old_style(title: &str, fignum: &str) -> Result<String, TypeError> {
    let mut out = String::with_capacity(title.len());
    let mut rest = title;
    let mut conversions = 0usize;
    while let Some(percent) = rest.find('%') {
        out.push_str(&rest[..percent]);
        let mut chars = rest[percent + 1..].chars();
        match chars.next() {
            Some('%') => out.push('%'),
            Some('s') => {
                conversions += 1;
                out.push_str(fignum);
            }
            // `%d` with a string argument, or a trailing bare `%`, is a
            // TypeError/ValueError; either way Sphinx logs #13.
            _ => return Err(TypeError),
        }
        rest = &rest[percent + 2..];
    }
    if conversions != 1 {
        // "not all arguments converted during string formatting".
        return Err(TypeError);
    }
    out.push_str(rest);
    Ok(out)
}

/// `ws_re.split(target, maxsplit=1)`: the first whitespace run splits the
/// leading word off.
fn split_once_whitespace(target: &str) -> Option<(&str, &str)> {
    let start = target.find(char::is_whitespace)?;
    let end = target[start..]
        .find(|c: char| !c.is_whitespace())
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
    /// References left to a domain this build has no implementation for
    /// (python, today), counted rather than warned about.
    pub unresolvable_domain_refs: usize,
}

/// Resolve every `pending_xref` in one document, rewriting the tree the way
/// `ReferencesResolver.run` does: the node is replaced by the reference
/// that resolution built, or by its own content node when it failed.
pub fn resolve_document(
    resolver: &Resolver<'_>,
    nitpick: &NitpickConfig<'_>,
    docname: &str,
    doctree: &mut Doctree,
    text: &str,
    path: &Path,
) -> DocumentResolution {
    let mut out = DocumentResolution::default();
    resolve_children(
        resolver,
        nitpick,
        docname,
        &mut doctree.root,
        text,
        path,
        &mut out,
    );
    propagate_desc_domain(&mut doctree.root);
    out
}

/// `PropagateDescDomain` (`post_transforms/__init__.py:382-390`, priority
/// 200): "Add the domain name of the parent node as a class in each
/// desc_signature node." Only descriptions that named a domain get one, so
/// `describe`/`object` (`domain=""`) are left alone.
fn propagate_desc_domain(node: &mut Node) {
    if node.kind == "desc" {
        if let Some(AttrValue::Str(domain)) = node.get("domain") {
            if !domain.is_empty() {
                let domain = domain.clone();
                for child in &mut node.children {
                    if child.kind == "desc_signature" {
                        child.attrs.classes.push(domain.clone());
                    }
                }
            }
        }
    }
    for child in &mut node.children {
        propagate_desc_domain(child);
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_children(
    resolver: &Resolver<'_>,
    nitpick: &NitpickConfig<'_>,
    docname: &str,
    node: &mut Node,
    text: &str,
    path: &Path,
    out: &mut DocumentResolution,
) {
    for child in &mut node.children {
        resolve_children(resolver, nitpick, docname, child, text, path, out);
    }
    if !node
        .children
        .iter()
        .any(|child| child.kind == kinds::PENDING_XREF)
    {
        return;
    }
    let children = std::mem::take(&mut node.children);
    for child in children {
        if child.kind != kinds::PENDING_XREF {
            node.children.push(child);
            continue;
        }
        node.children.extend(resolve_one(
            resolver, nitpick, docname, child, text, path, out,
        ));
    }
}

/// `ReferencesResolver._resolve_pending_xref` for a single node.
#[allow(clippy::too_many_arguments)]
fn resolve_one(
    resolver: &Resolver<'_>,
    nitpick: &NitpickConfig<'_>,
    docname: &str,
    node: Node,
    text: &str,
    path: &Path,
    out: &mut DocumentResolution,
) -> Vec<Node> {
    let span = node.span;
    let line = node_line(&node, text);
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
    // `contnode = node[0].deepcopy()`.
    let contnode = node.children.into_iter().next();
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

    // Domains this build cannot resolve are left alone: warning about them
    // would report every python reference in every project as broken. The
    // count feeds the build's one-line notice. Intersphinx still gets a
    // look first — a python reference into another project's inventory is
    // exactly what it is for.
    if refdomain != "std" && !refdomain.is_empty() {
        let mut diagnostics = Vec::new();
        let outcome = resolver
            .intersphinx
            .resolve_detect(&query, &mut diagnostics);
        report(out, diagnostics, line, path);
        if let HookOutcome::Resolved(resolution) = outcome {
            return vec![intersphinx_node(resolution, contnode, span)];
        }
        out.unresolvable_domain_refs += 1;
        return contnode.into_iter().collect();
    }
    // An M1 heuristic kept deliberately: a `:doc:` target that is a URL is
    // somebody linking out, not a broken document reference. Sphinx has no
    // such carve-out and warns; ours stays silent (pinned by the CLI e2e
    // suite).
    if reftype == "doc" && is_url(&reftarget) {
        return contnode.into_iter().collect();
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
    let outcome = resolver.resolve_xref(&req);

    match outcome {
        XrefOutcome::Resolved(resolved) => {
            vec![reference_node(resolved, contnode, span)]
        }
        XrefOutcome::Kept { warning } => {
            if let Some(message) = warning {
                out.warnings.push(
                    BuildWarning::new(
                        path.to_path_buf(),
                        Some(line),
                        message,
                        WarningType::BrokenCrossReference,
                    )
                    .with_category(None),
                );
            }
            contnode.into_iter().collect()
        }
        XrefOutcome::Missing => {
            // The `missing-reference` event, which is where intersphinx
            // hooks in: after the domain, before the warning.
            let mut diagnostics = Vec::new();
            let outcome = resolver
                .intersphinx
                .resolve_detect(&query, &mut diagnostics);
            report(out, diagnostics, line, path);
            match outcome {
                HookOutcome::Resolved(resolution) => {
                    return vec![intersphinx_node(resolution, contnode, span)];
                }
                // The target named this project: retry the local domain
                // with the prefix stripped. The warning below still reports
                // the target as written, because Sphinx never rewrote it.
                HookOutcome::SelfReferential(stripped) => {
                    let retry = resolver.resolve_xref(&XrefRequest {
                        reftarget: &stripped,
                        ..req
                    });
                    if let XrefOutcome::Resolved(resolved) = retry {
                        return vec![reference_node(resolved, contnode, span)];
                    }
                }
                HookOutcome::Missing => {}
            }
            if let Some(message) =
                missing_reference_warning(resolver.env, nitpick, &reftype, &reftarget, refwarn)
            {
                out.warnings.push(
                    BuildWarning::new(
                        path.to_path_buf(),
                        Some(line),
                        message,
                        WarningType::BrokenCrossReference,
                    )
                    // `logger.warning(..., type='ref', subtype=typ)`.
                    .with_category(Some(format!("ref.{reftype}"))),
                );
            }
            contnode.into_iter().collect()
        }
    }
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
    line: usize,
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

/// Turn intersphinx diagnostics into build warnings at the reference's line.
fn report(out: &mut DocumentResolution, diagnostics: Vec<Diagnostic>, line: usize, path: &Path) {
    for diagnostic in diagnostics {
        out.warnings.push(
            BuildWarning::new(
                path.to_path_buf(),
                Some(line),
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
fn missing_reference_warning(
    env: &BuildEnvironment,
    nitpick: &NitpickConfig<'_>,
    typ: &str,
    target: &str,
    refwarn: bool,
) -> Option<String> {
    let mut warn = refwarn;
    if nitpick.nitpicky {
        warn = true;
        // Only the std domain reaches here, so `dtype` is `std:<typ>` and
        // the domainless `(typ, target)` form is always also tried.
        let dtype = format!("std:{typ}");
        let ignored = nitpick
            .ignore
            .iter()
            .any(|(ityp, itarget)| (ityp == &dtype || ityp == typ) && itarget == target)
            || nitpick.ignore_regex.iter().any(|(ityp, itarget)| {
                (full_match(ityp, &dtype) || full_match(ityp, typ)) && full_match(itarget, target)
            });
        if ignored {
            warn = false;
        }
    }
    if !warn {
        return None;
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

fn is_url(target: &str) -> bool {
    target.starts_with("http://") || target.starts_with("https://") || target.starts_with("file://")
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
    match resolved.inner {
        Inner::Contnode => node.children.extend(contnode),
        Inner::Inline { text, classes } => {
            let mut inner = Node::elem("inline", span);
            inner.attrs.classes = classes;
            inner.children.push(Node::text_node(text, span));
            node.children.push(inner);
        }
    }
    node
}

#[cfg(test)]
mod intersphinx_tests;

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(format_old_style("Fig. %s", "1.2").unwrap(), "Fig. 1.2");
        assert!(
            format_old_style("Fig.", "1").is_err(),
            "no conversion: TypeError"
        );
        assert!(
            format_old_style("%s %s", "1").is_err(),
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
        let err = format_new_style("{nope}", Some("Cap"), "1").err().unwrap();
        assert_eq!(err.0, "nope");
    }

    #[test]
    fn dangling_warnings_use_the_exact_sphinx_texts() {
        let env = env_with_label();
        let nitpick = NitpickConfig {
            nitpicky: false,
            ignore: &[],
            ignore_regex: &[],
        };
        let warn =
            |typ: &str, target: &str| missing_reference_warning(&env, &nitpick, typ, target, true);
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
            missing_reference_warning(&env, &quiet, "envvar", "PATH", false),
            None
        );
        let nitpicky = NitpickConfig {
            nitpicky: true,
            ignore: &[],
            ignore_regex: &[],
        };
        assert!(missing_reference_warning(&env, &nitpicky, "envvar", "PATH", false).is_some());
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
            missing_reference_warning(&env, &config, "doc", "missing", true),
            None
        );
        assert!(missing_reference_warning(&env, &config, "doc", "other", true).is_some());

        // The domainless form is accepted for std types too.
        let domainless = vec![("doc".to_string(), "missing".to_string())];
        let config = NitpickConfig {
            nitpicky: true,
            ignore: &domainless,
            ignore_regex: &[],
        };
        assert_eq!(
            missing_reference_warning(&env, &config, "doc", "missing", true),
            None
        );

        let regex = vec![("std:.*".to_string(), "miss.*".to_string())];
        let config = NitpickConfig {
            nitpicky: true,
            ignore: &[],
            ignore_regex: &regex,
        };
        assert_eq!(
            missing_reference_warning(&env, &config, "doc", "missing", true),
            None
        );
        assert!(
            missing_reference_warning(&env, &config, "doc", "hit", true).is_some(),
            "the regexes must both full-match, not merely find"
        );
    }
}
