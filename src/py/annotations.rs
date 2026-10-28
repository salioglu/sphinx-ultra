//! Annotation rendering: the `_parse_annotation` / `type_to_xref` port
//! (`sphinx/domains/python/_annotations.py:30-251`, sphinx 9.1.0).
//!
//! `parse_annotation` turns one annotation string into the flat node list
//! Sphinx splices into a `desc_sig_name` wrapper (parameter annotations), a
//! `desc_returns` (return annotations) or a `desc_annotation` (`:type:`
//! options): `pending_xref` for every name, `desc_sig_*` leaves for the
//! punctuation/constants between them. The walk runs over the wave-4.5
//! [`super::expr::PyExpr`] AST, parsed the way `_parse_annotation` parses
//! it: `ast.parse(annotation, type_comments=True)` — **exec** mode, not
//! eval ([`parse_py_expr_stmt`], `_annotations.py:232`). That is what
//! makes a PEP 646 `*Ts` annotation a legal `Expr(Starred(…))` statement,
//! what makes an empty annotation an empty node list rather than an
//! empty-target xref, and what makes a leading indent an
//! `IndentationError` whose xref keeps the unstripped text. Anything
//! [`parse_py_expr_stmt`] rejects — and any node shape Sphinx's own
//! `unparse` has no branch for (`ast.Add`, `ast.Not`, `ast.BoolOp`, sets,
//! dicts, …, which raise `SyntaxError` there) — falls back to a single
//! [`type_to_xref`] of the whole annotation text, exactly like Sphinx's
//! `except SyntaxError` arm (`_annotations.py:250-251`).
//!
//! Ground truth, cited throughout as [PY §n] / [SIG §n]:
//! - [PY] docs/plans/2026-09-01-m2-wave4.5-research-spec-py-domain.md
//!   (§2.3 semantics, §2.6 leaf classes, §1.6 probe outputs);
//! - [SIG] docs/plans/2026-09-01-m2-wave4.5-research-spec-signature-config.md
//!   (§4.2 `pending_xref_condition` pair, §5.1 short literal chains).
//!
//! Every expected pformat in the test module is copied verbatim from probe
//! runs against the pinned toolchain (sphinx 9.1.0 / docutils 0.22.4,
//! harness3 conventions from tools/gen_sphinx_fixture.py; cases cited as
//! `probe <app>/<case>`, logged in the task-4 report).
//!
//! Documented divergences (all conservative — we fall back to the same
//! single-xref shape Sphinx uses for `SyntaxError`, never print something
//! different):
//! - constructs [`parse_py_expr_stmt`] rejects but `ast.parse` accepts and
//!   Sphinx *would* render (complex literals, multi-statement strings such
//!   as `int;` or `int\nstr`) fall back to one xref;
//! - an `ast.Attribute` whose value's first fragment is not a text node
//!   (`(1).x`, `'s'.x`) falls back instead of reproducing Python's
//!   `str(Element)` garbage (`f'{unparse(node.value)[0]}.{node.attr}'`,
//!   `_annotations.py:101`);
//! - `Union[()]` / `Optional[()]` fall back where Sphinx raises an
//!   uncaught `IndexError` (`_annotations.py:217`).

use crate::doctree::{kinds, AttrValue, Node, Span};

use super::expr::{self, parse_py_expr_stmt, PyConst, PyExpr, PyOp, PyUnaryOp};
use super::PySigConfig;

/// The slice of `env.ref_context` that `type_to_xref` copies onto every
/// annotation xref (`_annotations.py:62-66`): the enclosing `py:module` /
/// `py:class`, or Python `None` when unset — which docutils pformat renders
/// as the `"True"` sentinel (same convention as the inline parser's role
/// xrefs, src/rst/inline.rs) — plus the provenance the built nodes carry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PyRefContext {
    pub module: Option<String>,
    pub class_: Option<String>,
    /// The span every node this context builds is stamped with: the
    /// enclosing `desc_signature`'s, i.e. the directive's own `(source,
    /// line)`. Sphinx gives annotation xrefs no provenance of their own,
    /// and a resolution warning on one then locates through docutils'
    /// `get_source_line` ancestor walk — which stops at the signature,
    /// because `ObjectDescription.run` calls `set_source_info(signode)`.
    /// Stamping the signature's span directly yields the same location
    /// without depending on the walk, and keeps the include case exact
    /// (an annotation inside an included file names THAT file). A
    /// `Span::ZERO` (line 0) means "unstamped", which the resolver treats
    /// as "locate at the nearest stamped ancestor".
    pub span: Span,
}

/// Port of `parse_reftarget` (`_annotations.py:30-55`), `suppress_prefix`
/// fixed to `False`: returns `(reftype, reftarget, title, refspecific)`.
///
/// Leading `.` → strip + `refspecific`; leading `~` → strip + title = last
/// dotted component; `typing.` prefix → stripped from the TITLE only; the
/// reftype is `"obj"` for `None` and `typing.*` targets, else `"class"`
/// [PY §2.3].
pub fn parse_reftarget(target: &str) -> (String, String, String, bool) {
    let (reftype, target, title, refspecific) = parse_reftarget_impl(target, false);
    (reftype.to_string(), target, title, refspecific)
}

fn parse_reftarget_impl(
    reftarget: &str,
    suppress_prefix: bool,
) -> (&'static str, String, String, bool) {
    let mut refspecific = false;
    let (target, title) = if let Some(stripped) = reftarget.strip_prefix('.') {
        refspecific = true;
        (stripped.to_string(), stripped.to_string())
    } else if let Some(stripped) = reftarget.strip_prefix('~') {
        (stripped.to_string(), last_component(stripped).to_string())
    } else if suppress_prefix {
        (reftarget.to_string(), last_component(reftarget).to_string())
    } else if let Some(stripped) = reftarget.strip_prefix("typing.") {
        (reftarget.to_string(), stripped.to_string())
    } else {
        (reftarget.to_string(), reftarget.to_string())
    };

    // typing module provides non-class types; obj references are good for
    // them (`_annotations.py:49-53`). Tested on the STRIPPED target.
    let reftype = if target == "None" || target.starts_with("typing.") {
        "obj"
    } else {
        "class"
    };

    (reftype, target, title, refspecific)
}

/// Python `s.split('.')[-1]`.
fn last_component(s: &str) -> &str {
    s.rsplit('.').next().unwrap_or(s)
}

/// Port of `type_to_xref` (`_annotations.py:58-92`): one `pending_xref`
/// carrying `refdomain`/`reftype`/`reftarget`/`refspecific` plus the
/// `py:module`/`py:class` ref-context attrs, with a `Text(title)` child —
/// or, under `python_use_unqualified_type_names`, the two
/// `pending_xref_condition` children (`condition="resolved"` short name /
/// `condition="*"` full title) [SIG §4.2].
pub fn type_to_xref(target: &str, ctx: &PyRefContext, cfg: &PySigConfig) -> Node {
    type_to_xref_impl(target, ctx, cfg, false)
}

fn type_to_xref_impl(
    target: &str,
    ctx: &PyRefContext,
    cfg: &PySigConfig,
    suppress_prefix: bool,
) -> Node {
    let (reftype, target, title, refspecific) = parse_reftarget_impl(target, suppress_prefix);

    let mut node = Node::elem("pending_xref", ctx.span);
    // Context attrs are Python None outside a py scope; pformat renders
    // None as the "True" sentinel (same convention as src/rst/inline.rs).
    node.set(
        "py:class",
        AttrValue::Str(ctx.class_.clone().unwrap_or_else(|| "True".to_string())),
    );
    node.set(
        "py:module",
        AttrValue::Str(ctx.module.clone().unwrap_or_else(|| "True".to_string())),
    );
    node.set("refdomain", AttrValue::Str("py".to_string()));
    // A Python bool prints as 0/1 in pformat (`Element.starttag` casts
    // bools to int), hence the Int here.
    node.set("refspecific", AttrValue::Int(i64::from(refspecific)));
    node.set("reftarget", AttrValue::Str(target));
    node.set("reftype", AttrValue::Str(reftype.to_string()));

    if cfg.python_use_unqualified_type_names {
        // `shortname = title.split('.')[-1]` (`_annotations.py:76-80`).
        let shortname = last_component(&title).to_string();
        for (condition, text) in [("resolved", shortname), ("*", title)] {
            let mut cond = Node::elem("pending_xref_condition", ctx.span);
            cond.set("condition", AttrValue::Str(condition.to_string()));
            cond.children.push(Node::text_node(text, ctx.span));
            node.children.push(cond);
        }
    } else {
        node.children.push(Node::text_node(title, ctx.span));
    }
    node
}

/// Port of `_parse_annotation` (`_annotations.py:95-251`): parse one
/// annotation string and render it as a flat node list. On any parse or
/// unparse failure the whole string becomes a single [`type_to_xref`]
/// (`_annotations.py:250-251`) [PY §2.3].
pub fn parse_annotation(text: &str, ctx: &PyRefContext, cfg: &PySigConfig) -> Vec<Node> {
    let fallback = || vec![type_to_xref_impl(text, ctx, cfg, false)];

    let parsed = match parse_py_expr_stmt(text) {
        Ok(Some(parsed)) => parsed,
        // `ast.parse('')` is `Module(body=[])`, and the `ast.Module` arm
        // reduces an empty body to `[]` (`_annotations.py:150-151`) — no
        // node at all, not an empty-target xref.
        Ok(None) => return Vec::new(),
        Err(_) => return fallback(),
    };
    let Ok(frags) = unparse_frags(&parsed, cfg.python_display_short_literal_types) else {
        return fallback();
    };

    // Post-walk (`_annotations.py:233-249`): unwrap literal-protected
    // text, convert every remaining non-blank text fragment into an xref,
    // and let a `~` punctuation directly before a name suppress the title
    // prefix.
    let mut result: Vec<Node> = Vec::new();
    for node in frags {
        if node.kind == kinds::LITERAL {
            // `result.append(node[0])` — the wrapper always holds exactly
            // the one Text child it was built with.
            result.extend(node.children);
        } else if node.kind == kinds::TEXT {
            let target = node.text.as_deref().unwrap_or("");
            if target.trim().is_empty() {
                result.push(node);
                continue;
            }
            let suppress = result
                .last()
                .is_some_and(|last| last.kind == "desc_sig_punctuation" && last.astext() == "~");
            if suppress {
                result.pop();
            }
            result.push(type_to_xref_impl(target, ctx, cfg, suppress));
        } else {
            result.push(node);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// The `unparse` walk (`_annotations.py:99-229`)
// ---------------------------------------------------------------------------

/// A node shape Sphinx's `unparse` raises `SyntaxError` for (or one of the
/// module-doc divergences); the caller falls back to a whole-text xref.
struct Unsupported;

fn text_frag(text: impl Into<String>) -> Node {
    Node::text_node(text, Span::ZERO)
}

/// `unparse(ast.BitOr())`: space, `|`, space.
fn bitor_frags(out: &mut Vec<Node>) {
    out.push(desc_sig_space());
    out.push(desc_sig_punctuation("|"));
    out.push(desc_sig_space());
}

/// `repr(value)` for a supported constant — [`expr::unparse`] of the bare
/// constant, which is `ast.unparse`'s own repr path. A `u` prefix is
/// cleared first: Sphinx goes through `repr(node.value)`, and repr does
/// not know the string was `u`-prefixed (`_annotations.py:121`).
fn const_repr(c: &PyConst) -> String {
    let plain = match c {
        PyConst::Str {
            value,
            quote,
            u_prefix: true,
        } => PyConst::Str {
            value: value.clone(),
            quote: *quote,
            u_prefix: false,
        },
        other => other.clone(),
    };
    expr::unparse(&PyExpr::Constant(plain))
}

/// Comma+space-joined fragments of `elts`, the shared List/Tuple/Call join
/// (`_annotations.py:142-147`).
fn join_frags(
    elts: &[PyExpr],
    short_literals: bool,
    out: &mut Vec<Node>,
) -> Result<(), Unsupported> {
    for (i, elt) in elts.iter().enumerate() {
        if i > 0 {
            out.push(desc_sig_punctuation(","));
            out.push(desc_sig_space());
        }
        out.extend(unparse_frags(elt, short_literals)?);
    }
    Ok(())
}

fn unparse_frags(e: &PyExpr, short_literals: bool) -> Result<Vec<Node>, Unsupported> {
    match e {
        // `[Text(f'{unparse(node.value)[0]}.{node.attr}')]`
        // (`_annotations.py:100-101`). Only a text first fragment is
        // joinable; an element there is the module-doc divergence
        // (Sphinx would interpolate `str(Element)` garbage — we fall
        // back conservatively).
        PyExpr::Attribute(value, attr) => {
            let frags = unparse_frags(value, short_literals)?;
            let first = frags.first().ok_or(Unsupported)?;
            let base = first.text.as_deref().ok_or(Unsupported)?;
            Ok(vec![text_frag(format!("{base}.{attr}"))])
        }
        // `unparse` has no `ast.BoolOp` branch, so `a or b` reaches the
        // `raise SyntaxError` fallthrough (`_annotations.py:209-210`) and
        // the whole annotation becomes one xref.
        PyExpr::BoolOp { .. } => Err(Unsupported),
        // Only `BitOr` has an unparse branch; any other operator raises
        // SyntaxError in Sphinx (`_annotations.py:102-112`, `209-210`).
        PyExpr::BinOp { left, op, right } => {
            if *op != PyOp::BitOr {
                return Err(Unsupported);
            }
            let mut out = unparse_frags(left, short_literals)?;
            bitor_frags(&mut out);
            out.extend(unparse_frags(right, short_literals)?);
            Ok(out)
        }
        PyExpr::Constant(c) => Ok(vec![match c {
            PyConst::Ellipsis => desc_sig_punctuation("..."),
            PyConst::True => desc_sig_keyword("True"),
            PyConst::False => desc_sig_keyword("False"),
            PyConst::Int(digits) => desc_sig_literal_number(digits),
            PyConst::Str { .. } => desc_sig_literal_string(&const_repr(c)),
            // The `Text(repr(value))` fallthrough (`_annotations.py:
            // 122-125`): None (xref'd later by the post-walk), floats,
            // bytes.
            PyConst::None => text_frag("None"),
            PyConst::Float(_) | PyConst::Bytes(_) => text_frag(const_repr(c)),
        }]),
        // `desc_sig_operator('*')` + value (`_annotations.py:128-131`).
        PyExpr::Starred(value) => {
            let mut out = vec![desc_sig_operator("*")];
            out.extend(unparse_frags(value, short_literals)?);
            Ok(out)
        }
        PyExpr::List(elts) => {
            let mut out = vec![desc_sig_punctuation("[")];
            join_frags(elts, short_literals, &mut out)?;
            out.push(desc_sig_punctuation("]"));
            Ok(out)
        }
        PyExpr::Name(id) => Ok(vec![text_frag(id.clone())]),
        PyExpr::Subscript { value, slice } => {
            // `getattr(node.value, 'id', '')` — a bare Name only
            // (`_annotations.py:155-158`); `typing.Optional` etc. take the
            // plain subscript path.
            if let PyExpr::Name(id) = value.as_ref() {
                if id == "Optional" || id == "Union" || (short_literals && id == "Literal") {
                    return unparse_pep_604(id, slice, short_literals);
                }
            }
            let mut out = unparse_frags(value, short_literals)?;
            out.push(desc_sig_punctuation("["));
            out.extend(unparse_frags(slice, short_literals)?);
            out.push(desc_sig_punctuation("]"));

            // `result[0] in {'Literal', 'typing.Literal'}`: protect the
            // member Text nodes from the xref post-walk by wrapping them
            // in `nodes.literal` (`_annotations.py:164-168`).
            let is_literal = matches!(
                out[0].text.as_deref(),
                Some("Literal") | Some("typing.Literal")
            );
            if is_literal {
                for node in &mut out[1..] {
                    if node.kind == kinds::TEXT {
                        let mut wrapper = Node::elem(kinds::LITERAL, Span::ZERO);
                        wrapper.children.push(std::mem::replace(
                            node,
                            Node::elem(kinds::LITERAL, Span::ZERO),
                        ));
                        *node = wrapper;
                    }
                }
            }
            Ok(out)
        }
        // Only `Invert` and `USub` have op branches (`_annotations.py:
        // 132-135`, `170-171`); `UAdd`/`Not` raise SyntaxError.
        PyExpr::UnaryOp { op, operand } => {
            let punct = match op {
                PyUnaryOp::Invert => desc_sig_punctuation("~"),
                PyUnaryOp::USub => desc_sig_punctuation("-"),
                PyUnaryOp::UAdd | PyUnaryOp::Not => return Err(Unsupported),
            };
            let mut out = vec![punct];
            out.extend(unparse_frags(operand, short_literals)?);
            Ok(out)
        }
        PyExpr::Tuple(elts) => {
            if elts.is_empty() {
                Ok(vec![desc_sig_punctuation("("), desc_sig_punctuation(")")])
            } else {
                let mut out = Vec::new();
                join_frags(elts, short_literals, &mut out)?;
                Ok(out)
            }
        }
        // Annotated metadata calls (`_annotations.py:188-208`): positional
        // args comma+space-joined, keywords as `name` `=` value with no
        // spaces around the `=`.
        PyExpr::Call { func, args, kwargs } => {
            let mut out = unparse_frags(func, short_literals)?;
            out.push(desc_sig_punctuation("("));
            let mut inner = Vec::new();
            join_frags(args, short_literals, &mut inner)?;
            for (name, value) in kwargs {
                if !inner.is_empty() {
                    inner.push(desc_sig_punctuation(","));
                    inner.push(desc_sig_space());
                }
                inner.push(desc_sig_name(name));
                inner.push(desc_sig_operator("="));
                inner.extend(unparse_frags(value, short_literals)?);
            }
            out.extend(inner);
            out.push(desc_sig_punctuation(")"));
            Ok(out)
        }
        // No unparse branch in Sphinx → SyntaxError → fallback.
        PyExpr::Set(_) | PyExpr::Dict(_) => Err(Unsupported),
    }
}

/// `_unparse_pep_604_annotation` (`_annotations.py:212-229`): flatten the
/// subscript into a `|` chain; `Optional` appends `| None`. A short-literal
/// `Literal` routes here too [SIG §5.1]. An empty tuple slice is the
/// module-doc `IndexError` divergence — we fall back.
fn unparse_pep_604(
    value_id: &str,
    slice: &PyExpr,
    short_literals: bool,
) -> Result<Vec<Node>, Unsupported> {
    let mut out = Vec::new();
    match slice {
        PyExpr::Tuple(elts) => {
            let (first, rest) = elts.split_first().ok_or(Unsupported)?;
            out.extend(unparse_frags(first, short_literals)?);
            for elt in rest {
                bitor_frags(&mut out);
                out.extend(unparse_frags(elt, short_literals)?);
            }
        }
        // e.g. a Union[] inside an Optional[] (`_annotations.py:221-223`).
        other => out.extend(unparse_frags(other, short_literals)?),
    }
    if value_id == "Optional" {
        bitor_frags(&mut out);
        out.push(text_frag("None"));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// desc_sig_* leaf builders ([PY §2.6] class map)
// ---------------------------------------------------------------------------

fn sig_leaf(kind: &'static str, class: &str, text: &str) -> Node {
    let mut node = Node::elem(kind, Span::ZERO);
    node.attrs.classes.push(class.to_string());
    node.children.push(Node::text_node(text, Span::ZERO));
    node
}

/// `desc_sig_space` (class `w`), always the single space Sphinx's default
/// constructor inserts (`addnodes.py:341-346`).
pub(crate) fn desc_sig_space() -> Node {
    sig_leaf("desc_sig_space", "w", " ")
}

/// `desc_sig_name` (class `n`).
pub(crate) fn desc_sig_name(text: &str) -> Node {
    sig_leaf("desc_sig_name", "n", text)
}

/// `desc_sig_operator` (class `o`).
pub(crate) fn desc_sig_operator(text: &str) -> Node {
    sig_leaf("desc_sig_operator", "o", text)
}

/// `desc_sig_punctuation` (class `p`).
pub(crate) fn desc_sig_punctuation(text: &str) -> Node {
    sig_leaf("desc_sig_punctuation", "p", text)
}

/// `desc_sig_keyword` (class `k`).
pub(crate) fn desc_sig_keyword(text: &str) -> Node {
    sig_leaf("desc_sig_keyword", "k", text)
}

/// `desc_sig_literal_number` (class `m`).
pub(crate) fn desc_sig_literal_number(text: &str) -> Node {
    sig_leaf("desc_sig_literal_number", "m", text)
}

/// `desc_sig_literal_string` (class `s`).
pub(crate) fn desc_sig_literal_string(text: &str) -> Node {
    sig_leaf("desc_sig_literal_string", "s", text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wrap a fragment list in the probe's own parent so the assertion
    /// text is byte-verbatim probe output (the brief's throwaway parent;
    /// `desc_returns`/`desc_annotation` both carry `xml:space="preserve"`,
    /// [PY §1.6]).
    fn wrap(kind: &'static str, children: Vec<Node>) -> String {
        let mut parent = Node::elem(kind, Span::ZERO);
        parent.set("xml:space", AttrValue::Str("preserve".to_string()));
        parent.children = children;
        parent.pformat()
    }

    /// `parse_annotation` under a default context/config, rendered as the
    /// probes' `.. py:function:: f(x) -> <annotation>` return fragment.
    fn returns(annotation: &str) -> String {
        returns_with(annotation, &PySigConfig::default())
    }

    fn returns_with(annotation: &str, cfg: &PySigConfig) -> String {
        wrap(
            "desc_returns",
            parse_annotation(annotation, &PyRefContext::default(), cfg),
        )
    }

    /// The probes' `.. py:data::` + `:type:` fragment: the caller-side
    /// `: ` prefix ([PY §1.6] CASE attribute_typed) plus `parse_annotation`
    /// output, so the assertion matches the probe's `desc_annotation`
    /// verbatim.
    fn type_option(annotation: &str) -> String {
        let mut children = vec![desc_sig_punctuation(":"), desc_sig_space()];
        children.extend(parse_annotation(
            annotation,
            &PyRefContext::default(),
            &PySigConfig::default(),
        ));
        wrap("desc_annotation", children)
    }

    fn unqualified() -> PySigConfig {
        PySigConfig {
            python_use_unqualified_type_names: true,
            ..PySigConfig::default()
        }
    }

    fn short_literals() -> PySigConfig {
        PySigConfig {
            python_display_short_literal_types: true,
            ..PySigConfig::default()
        }
    }

    // -- parse_reftarget ----------------------------------------------------

    /// [PY §2.3]: a plain (possibly dotted) name passes through untouched
    /// and refers as a class.
    #[test]
    fn parse_reftarget_plain_name_is_class() {
        assert_eq!(
            parse_reftarget("pkg.Cls"),
            (
                "class".to_string(),
                "pkg.Cls".to_string(),
                "pkg.Cls".to_string(),
                false
            )
        );
        assert_eq!(
            parse_reftarget("int"),
            (
                "class".to_string(),
                "int".to_string(),
                "int".to_string(),
                false
            )
        );
    }

    /// [PY §2.3] + probe default/data_dot: a leading `.` is stripped from
    /// target AND title, and sets the refspecific flag.
    #[test]
    fn parse_reftarget_leading_dot_sets_refspecific() {
        assert_eq!(
            parse_reftarget(".MyClass"),
            (
                "class".to_string(),
                "MyClass".to_string(),
                "MyClass".to_string(),
                true
            )
        );
    }

    /// [PY §2.3] + probe default/tilde: a leading `~` is stripped from the
    /// target and the title keeps only the last dotted component.
    #[test]
    fn parse_reftarget_tilde_title_is_last_component() {
        assert_eq!(
            parse_reftarget("~pkg.Cls"),
            (
                "class".to_string(),
                "pkg.Cls".to_string(),
                "Cls".to_string(),
                false
            )
        );
    }

    /// [PY §2.3] + probe default/typing_prefix: `typing.` is stripped from
    /// the TITLE only — the reftarget keeps the prefix — and the reftype is
    /// "obj"; same for `None` (probe default/none_obj).
    #[test]
    fn parse_reftarget_none_and_typing_targets_are_obj() {
        assert_eq!(
            parse_reftarget("typing.Any"),
            (
                "obj".to_string(),
                "typing.Any".to_string(),
                "Any".to_string(),
                false
            )
        );
        assert_eq!(
            parse_reftarget("None"),
            (
                "obj".to_string(),
                "None".to_string(),
                "None".to_string(),
                false
            )
        );
    }

    // -- unions and the PEP-604 rewrite ------------------------------------

    /// [PY §2.3] `X | Y` → xref, space, `|`, space, xref; the `None` arm is
    /// an "obj" xref. Verbatim probe default/union.
    #[test]
    fn a_union_renders_xref_space_pipe_space_xref() {
        assert_eq!(
            returns("int | None"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                "        int\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        |\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"None\" reftype=\"obj\">\n",
                "        None\n",
            )
        );
    }

    /// [PY §2.3] `Optional[X]` rewrites to `X | None` — byte-identical to
    /// the `int | None` shape. Verbatim probe default/optional.
    #[test]
    fn optional_rewrites_to_pep_604_with_obj_none() {
        assert_eq!(returns("Optional[int]"), returns("int | None"));
    }

    /// [PY §2.3] `Union[X, Y]` rewrites to pipes. Verbatim probe
    /// default/union_explicit.
    #[test]
    fn union_subscript_rewrites_to_pipes() {
        assert_eq!(
            returns("Union[int, str]"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                "        int\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        |\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"str\" reftype=\"class\">\n",
                "        str\n",
            )
        );
    }

    /// `Optional[Union[int, str]]` — the non-tuple slice recurses into the
    /// inner `Union` rewrite ("e.g. a Union[] inside an Optional[]",
    /// `_annotations.py:221-223`) and `Optional` still appends `| None`.
    /// Verbatim probe default/union_of_optional.
    #[test]
    fn optional_of_union_flattens_and_appends_none() {
        assert_eq!(
            returns("Optional[Union[int, str]]"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                "        int\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        |\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"str\" reftype=\"class\">\n",
                "        str\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        |\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"None\" reftype=\"obj\">\n",
                "        None\n",
            )
        );
    }

    // -- subscripts ---------------------------------------------------------

    /// [PY §2.3] a subscript renders `value [ slice ]` with punctuation
    /// brackets. Verbatim probe default/subscript_simple.
    #[test]
    fn a_subscript_renders_value_bracket_slice_bracket() {
        assert_eq!(
            returns("list[str]"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"list\" reftype=\"class\">\n",
                "        list\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        [\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"str\" reftype=\"class\">\n",
                "        str\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ]\n",
            )
        );
    }

    /// [PY §2.3] a tuple slice joins members with `desc_sig_punctuation(",")`
    /// + `desc_sig_space`. Verbatim probe default/subscript_tuple.
    #[test]
    fn a_tuple_slice_joins_with_comma_and_space() {
        assert_eq!(
            returns("dict[str, int]"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"dict\" reftype=\"class\">\n",
                "        dict\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        [\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"str\" reftype=\"class\">\n",
                "        str\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ,\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                "        int\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ]\n",
            )
        );
    }

    /// A nested subscript value recurses flat — no re-wrapping of the inner
    /// value (the brief's ambiguity probe). Verbatim probe
    /// default/subscript_nested.
    #[test]
    fn nested_subscripts_recurse_flat() {
        assert_eq!(
            returns("dict[str, list[int]]"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"dict\" reftype=\"class\">\n",
                "        dict\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        [\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"str\" reftype=\"class\">\n",
                "        str\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ,\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"list\" reftype=\"class\">\n",
                "        list\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        [\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                "        int\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ]\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ]\n",
            )
        );
    }

    /// A list display inside a subscript renders punctuation brackets with
    /// the same comma+space joins (`_annotations.py:136-149`). Verbatim
    /// probe default/callable_list.
    #[test]
    fn a_list_display_renders_punctuation_brackets() {
        assert_eq!(
            returns("Callable[[int, str], bool]"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"Callable\" reftype=\"class\">\n",
                "        Callable\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        [\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        [\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                "        int\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ,\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"str\" reftype=\"class\">\n",
                "        str\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ]\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ,\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"bool\" reftype=\"class\">\n",
                "        bool\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ]\n",
            )
        );
    }

    /// An empty tuple slice renders as the `(` `)` punctuation pair
    /// (`_annotations.py:181-185`). Verbatim probe default/tuple_empty.
    #[test]
    fn an_empty_tuple_slice_renders_paren_pair() {
        assert_eq!(
            returns("Tuple[()]"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"Tuple\" reftype=\"class\">\n",
                "        Tuple\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        [\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        (\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        )\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ]\n",
            )
        );
    }

    // -- Literal ------------------------------------------------------------

    /// [PY §2.3] trap 15: `Literal` members stay `desc_sig_literal_string`
    /// — never an xref — while `Literal` itself is one. Verbatim probe
    /// default/literal_default.
    #[test]
    fn literal_members_stay_literal_strings_next_to_a_literal_xref() {
        assert_eq!(
            returns("Literal['a', 'b']"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"Literal\" reftype=\"class\">\n",
                "        Literal\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        [\n",
                "    <desc_sig_literal_string classes=\"s\">\n",
                "        'a'\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ,\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <desc_sig_literal_string classes=\"s\">\n",
                "        'b'\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ]\n",
            )
        );
    }

    /// A `None` member of `Literal[...]` is wrapped in `nodes.literal` and
    /// unwrapped by the post-loop (`_annotations.py:164-168`, `235-236`),
    /// so it lands as bare text — NOT an obj xref. Verbatim probe
    /// default/literal_none_member.
    #[test]
    fn a_none_member_of_literal_stays_bare_text() {
        assert_eq!(
            returns("Literal[None]"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"Literal\" reftype=\"class\">\n",
                "        Literal\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        [\n",
                "    None\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ]\n",
            )
        );
    }

    /// `typing.Literal[...]` — the literal-wrap check matches the joined
    /// `typing.Literal` text too (`_annotations.py:165`) and the xref gets
    /// the obj reftype with the `typing.`-stripped title. Verbatim probe
    /// default/typing_literal.
    #[test]
    fn typing_literal_is_obj_with_stripped_title() {
        assert_eq!(
            returns("typing.Literal['a']"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"typing.Literal\" reftype=\"obj\">\n",
                "        Literal\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        [\n",
                "    <desc_sig_literal_string classes=\"s\">\n",
                "        'a'\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ]\n",
            )
        );
    }

    // -- tilde suppression --------------------------------------------------

    /// [PY §2.3] a `~` punctuation directly before a name is popped and the
    /// name's xref keeps only the last dotted component as its title
    /// (`_annotations.py:237-244`). Verbatim probes default/tilde and
    /// default/tilde_bare.
    #[test]
    fn a_tilde_before_a_name_suppresses_the_title_prefix() {
        assert_eq!(
            returns("~pkg.Cls"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"pkg.Cls\" reftype=\"class\">\n",
                "        Cls\n",
            )
        );
        assert_eq!(
            returns("~Cls"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"Cls\" reftype=\"class\">\n",
                "        Cls\n",
            )
        );
    }

    // -- names, typing.*, None ---------------------------------------------

    /// [PY §2.3] `typing.Any` → obj reftype, full reftarget, stripped
    /// title. Verbatim probe default/typing_prefix.
    #[test]
    fn typing_prefix_yields_obj_reftype() {
        assert_eq!(
            returns("typing.Any"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"typing.Any\" reftype=\"obj\">\n",
                "        Any\n",
            )
        );
    }

    /// [PY §2.3] a bare `None` annotation is an obj xref. Verbatim probe
    /// default/none_obj.
    #[test]
    fn bare_none_annotation_is_an_obj_xref() {
        assert_eq!(
            returns("None"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"None\" reftype=\"obj\">\n",
                "        None\n",
            )
        );
    }

    // -- constants ----------------------------------------------------------

    /// [PY §2.3] `...` → `desc_sig_punctuation("...")`. Verbatim probe
    /// default/ellipsis.
    #[test]
    fn ellipsis_renders_punctuation() {
        assert_eq!(
            returns("..."),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ...\n",
            )
        );
    }

    /// [PY §2.3] booleans → `desc_sig_keyword`. Verbatim probe
    /// default/bool_true.
    #[test]
    fn true_renders_keyword() {
        assert_eq!(
            returns("True"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <desc_sig_keyword classes=\"k\">\n",
                "        True\n",
            )
        );
    }

    /// [PY §2.3] ints → `desc_sig_literal_number`. Verbatim probe
    /// default/int_const.
    #[test]
    fn an_int_renders_literal_number() {
        assert_eq!(
            returns("42"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <desc_sig_literal_number classes=\"m\">\n",
                "        42\n",
            )
        );
    }

    /// `-1` is `USub` + constant: `desc_sig_punctuation("-")` then the
    /// number (`_annotations.py:134-135`). Verbatim probe default/neg_int.
    #[test]
    fn a_negative_int_renders_minus_punctuation_then_number() {
        assert_eq!(
            returns("-1"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        -\n",
                "    <desc_sig_literal_number classes=\"m\">\n",
                "        1\n",
            )
        );
    }

    /// [PY §2.3] trap 15: a string-literal annotation stays
    /// `desc_sig_literal_string` — never an xref. Verbatim probe
    /// default/str_const.
    #[test]
    fn a_string_annotation_stays_literal_string_never_an_xref() {
        assert_eq!(
            returns("'MyClass'"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <desc_sig_literal_string classes=\"s\">\n",
                "        'MyClass'\n",
            )
        );
    }

    /// Sphinx renders string constants through `repr(node.value)`
    /// (`_annotations.py:121`), which drops a `u` prefix. Verbatim probe
    /// default/u_string.
    #[test]
    fn a_u_prefixed_string_drops_the_prefix_like_repr() {
        assert_eq!(
            returns("u'x'"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <desc_sig_literal_string classes=\"s\">\n",
                "        'x'\n",
            )
        );
    }

    /// Float and bytes constants hit the `Text(repr(value))` fallthrough
    /// (`_annotations.py:122-125`) and so become XREFS of their repr text —
    /// not literal leaves. Verbatim probes default/float_const and
    /// default/bytes_const.
    #[test]
    fn float_and_bytes_constants_become_xrefs_via_repr_text() {
        assert_eq!(
            returns("1.5"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"1.5\" reftype=\"class\">\n",
                "        1.5\n",
            )
        );
        assert_eq!(
            returns("b'x'"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"b'x'\" reftype=\"class\">\n",
                "        b'x'\n",
            )
        );
    }

    // -- calls (Annotated metadata) -----------------------------------------

    /// A call renders `func ( args )` with comma+space joins; keywords are
    /// `desc_sig_name(arg)` + `desc_sig_operator("=")` + value, no spaces
    /// (`_annotations.py:188-208`). Verbatim probe default/annotated_call.
    #[test]
    fn a_call_renders_args_and_keywords() {
        assert_eq!(
            returns("Annotated[str, Validator(str, len=10)]"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"Annotated\" reftype=\"class\">\n",
                "        Annotated\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        [\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"str\" reftype=\"class\">\n",
                "        str\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ,\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"Validator\" reftype=\"class\">\n",
                "        Validator\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        (\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"str\" reftype=\"class\">\n",
                "        str\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ,\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <desc_sig_name classes=\"n\">\n",
                "        len\n",
                "    <desc_sig_operator classes=\"o\">\n",
                "        =\n",
                "    <desc_sig_literal_number classes=\"m\">\n",
                "        10\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        )\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ]\n",
            )
        );
    }

    // -- attributes ---------------------------------------------------------

    /// `ast.Attribute` joins only its value's FIRST fragment with the attr
    /// (`_annotations.py:100-101`) — the rest of a subscript value is
    /// dropped. Verbatim probe default/dotted_attr_of_subscript.
    #[test]
    fn an_attribute_of_a_subscript_keeps_only_the_first_fragment() {
        assert_eq!(
            type_option("list[int].x"),
            concat!(
                "<desc_annotation xml:space=\"preserve\">\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        :\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"list.x\" reftype=\"class\">\n",
                "        list.x\n",
            )
        );
    }

    // -- SyntaxError fallback ----------------------------------------------

    /// [PY §2.3] a parse error turns the WHOLE annotation string into one
    /// `type_to_xref`. Verbatim probe default/data_syntax_error.
    #[test]
    fn a_syntax_error_falls_back_to_one_xref_of_the_whole_text() {
        assert_eq!(
            type_option("List[int"),
            concat!(
                "<desc_annotation xml:space=\"preserve\">\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        :\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"List[int\" reftype=\"class\">\n",
                "        List[int\n",
            )
        );
    }

    /// [PY §2.3] `.MyClass` is a SyntaxError to `ast.parse`, so the
    /// fallback xref carries the leading-dot handling: stripped target +
    /// refspecific="1". Verbatim probe default/data_dot.
    #[test]
    fn a_leading_dot_falls_back_and_sets_refspecific() {
        assert_eq!(
            type_option(".MyClass"),
            concat!(
                "<desc_annotation xml:space=\"preserve\">\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        :\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"1\" reftarget=\"MyClass\" reftype=\"class\">\n",
                "        MyClass\n",
            )
        );
    }

    /// Node shapes Sphinx's `unparse` has no branch for raise SyntaxError
    /// there (`_annotations.py:209-210`) and fall back the same way: a
    /// non-BitOr BinOp and a set display both become one whole-text xref.
    /// Verbatim probes default/binop_add and default/set_display.
    #[test]
    fn unsupported_node_shapes_fall_back_to_one_xref() {
        assert_eq!(
            returns("X + Y"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"X + Y\" reftype=\"class\">\n",
                "        X + Y\n",
            )
        );
        assert_eq!(
            returns("{1, 2}"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"{1, 2}\" reftype=\"class\">\n",
                "        {1, 2}\n",
            )
        );
    }

    // -- ref context --------------------------------------------------------

    /// The enclosing module/class land in the `py:module`/`py:class` attrs;
    /// unset halves keep the None sentinel. Verbatim probe
    /// default/ctx_module_class (pending_xref subtree).
    #[test]
    fn ref_context_lands_in_py_module_and_py_class_attrs() {
        let ctx = PyRefContext {
            module: Some("mymod".to_string()),
            class_: Some("C".to_string()),
            span: Span::ZERO,
        };
        assert_eq!(
            type_to_xref("int", &ctx, &PySigConfig::default()).pformat(),
            concat!(
                "<pending_xref py:class=\"C\" py:module=\"mymod\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                "    int\n",
            )
        );
    }

    // -- python_use_unqualified_type_names ----------------------------------

    /// [SIG §4.2] under `python_use_unqualified_type_names` the xref content
    /// becomes TWO `pending_xref_condition` children — `condition="resolved"`
    /// short name / `condition="*"` full title — not a Text. Verbatim probe
    /// unqualified/unqualified_dotted.
    #[test]
    fn unqualified_config_emits_condition_pair() {
        assert_eq!(
            returns_with("pkg.Cls", &unqualified()),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"pkg.Cls\" reftype=\"class\">\n",
                "        <pending_xref_condition condition=\"resolved\">\n",
                "            Cls\n",
                "        <pending_xref_condition condition=\"*\">\n",
                "            pkg.Cls\n",
            )
        );
    }

    /// The `condition="*"` child carries the TITLE — after `~` shortening
    /// both conditions show the short name. Verbatim probe
    /// unqualified/unqualified_tilde.
    #[test]
    fn unqualified_tilde_conditions_share_the_short_title() {
        assert_eq!(
            returns_with("~pkg.mod.Cls", &unqualified()),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"pkg.mod.Cls\" reftype=\"class\">\n",
                "        <pending_xref_condition condition=\"resolved\">\n",
                "            Cls\n",
                "        <pending_xref_condition condition=\"*\">\n",
                "            Cls\n",
            )
        );
    }

    /// Same under a `typing.` title strip: both conditions show the
    /// stripped title, and the obj reftype is untouched. Verbatim probe
    /// unqualified/unqualified_typing.
    #[test]
    fn unqualified_typing_conditions_share_the_stripped_title() {
        assert_eq!(
            returns_with("typing.Any", &unqualified()),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"typing.Any\" reftype=\"obj\">\n",
                "        <pending_xref_condition condition=\"resolved\">\n",
                "            Any\n",
                "        <pending_xref_condition condition=\"*\">\n",
                "            Any\n",
            )
        );
    }

    // -- python_display_short_literal_types ----------------------------------

    /// [SIG §5.1] `Literal['a', 'b']` under the short-literal config becomes
    /// the `'a' | 'b'` chain — no `Literal` xref, no brackets. Verbatim
    /// probe short_literals/short_literal.
    #[test]
    fn short_literal_types_render_pipe_chain_without_literal_xref() {
        assert_eq!(
            returns_with("Literal['a', 'b']", &short_literals()),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <desc_sig_literal_string classes=\"s\">\n",
                "        'a'\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        |\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <desc_sig_literal_string classes=\"s\">\n",
                "        'b'\n",
            )
        );
    }

    /// In the short-literal chain there is no literal-wrap step, so a
    /// `None` member DOES become an obj xref (unlike the bracketed form).
    /// Verbatim probe short_literals/short_literal_mixed.
    #[test]
    fn a_short_literal_none_member_becomes_an_obj_xref() {
        assert_eq!(
            returns_with("Literal[1, 'a', None]", &short_literals()),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <desc_sig_literal_number classes=\"m\">\n",
                "        1\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        |\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <desc_sig_literal_string classes=\"s\">\n",
                "        'a'\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        |\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"None\" reftype=\"obj\">\n",
                "        None\n",
            )
        );
    }

    /// The short-literal route checks `getattr(node.value, 'id', '')` — a
    /// bare `Literal` Name only. `typing.Literal[...]` keeps the bracketed
    /// form even under the config. Verbatim probe
    /// short_literals/short_literal_typing.
    #[test]
    fn short_literal_config_ignores_typing_literal() {
        assert_eq!(
            returns_with("typing.Literal['a', 'b']", &short_literals()),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"typing.Literal\" reftype=\"obj\">\n",
                "        Literal\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        [\n",
                "    <desc_sig_literal_string classes=\"s\">\n",
                "        'a'\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ,\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <desc_sig_literal_string classes=\"s\">\n",
                "        'b'\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ]\n",
            )
        );
    }
    // -- exec-mode parse (`_parse_annotation` uses `ast.parse`, not eval) ----

    /// PEP 646: `*Ts` is a legal `Expr(Starred(Name))` statement in exec
    /// mode, so Sphinx renders `desc_sig_operator('*')` + the xref rather
    /// than falling back to one `reftarget="*Ts"` xref
    /// (`_annotations.py:128-131` + `:232`).
    ///
    // oracle: scratchpad A/p5.py, `_parse_annotation('*Ts', env)` under
    // sphinx 9.1.0 / docutils 0.22.4.
    #[test]
    fn pep_646_star_annotation_splits_the_operator() {
        assert_eq!(
            returns("*Ts"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <desc_sig_operator classes=\"o\">\n",
                "        *\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"Ts\" reftype=\"class\">\n",
                "        Ts\n",
            )
        );
    }

    /// The bracketed unpack — the spelling autodoc emits — walks into the
    /// subscript as usual after the `*`.
    ///
    // oracle: scratchpad A/p5.py, `_parse_annotation('*tuple[int, ...]')`.
    #[test]
    fn pep_646_star_annotation_over_a_subscript() {
        assert_eq!(
            returns("*tuple[int, ...]"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <desc_sig_operator classes=\"o\">\n",
                "        *\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"tuple\" reftype=\"class\">\n",
                "        tuple\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        [\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                "        int\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ,\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ...\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ]\n",
            )
        );
    }

    /// A bare starred tuple is an `Expr(Tuple([Starred, ...]))` statement.
    ///
    // oracle: scratchpad A/p5.py, `_parse_annotation('*a, b')`.
    #[test]
    fn exec_mode_renders_a_bare_starred_tuple() {
        assert_eq!(
            returns("*a, b"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <desc_sig_operator classes=\"o\">\n",
                "        *\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"a\" reftype=\"class\">\n",
                "        a\n",
                "    <desc_sig_punctuation classes=\"p\">\n",
                "        ,\n",
                "    <desc_sig_space classes=\"w\">\n",
                "         \n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"b\" reftype=\"class\">\n",
                "        b\n",
            )
        );
    }

    /// A leading indent is an `IndentationError` — a `SyntaxError`
    /// subclass — so the `except SyntaxError` arm xrefs the text
    /// UNSTRIPPED, spaces and all.
    ///
    // oracle: scratchpad A/p5.py, `_parse_annotation('  int')` → one
    // pending_xref with reftarget="  int".
    #[test]
    fn leading_indent_keeps_the_unstripped_text() {
        assert_eq!(
            returns("  int"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"  int\" reftype=\"class\">\n",
                "          int\n",
            )
        );
    }

    /// `ast.parse('')` is `Module(body=[])` and the `ast.Module` arm
    /// reduces an empty body to `[]` — no node at all, so no empty-target
    /// xref ever reaches the resolver.
    ///
    // oracle: scratchpad A/p5.py — `len(_parse_annotation(''))` and
    // `len(_parse_annotation(' '))` are both 0.
    #[test]
    fn an_empty_annotation_renders_no_nodes() {
        for text in ["", " ", "  ", "\n", "\t"] {
            assert!(
                parse_annotation(text, &PyRefContext::default(), &PySigConfig::default())
                    .is_empty(),
                "{text:?} must render no nodes"
            );
        }
    }

    /// `unparse` has no `ast.BoolOp` branch, so `a or b` reaches the
    /// `raise SyntaxError` fallthrough and becomes one whole-text xref
    /// (even though [`super::expr::parse_py_expr_stmt`] now parses it).
    ///
    // oracle: `_parse_annotation('a or b', env)` → a single pending_xref
    // with reftarget="a or b" (sphinx 9.1.0, scratchpad A/p6.py).
    #[test]
    fn a_boolop_annotation_falls_back_to_one_xref() {
        assert_eq!(
            returns("a or b"),
            concat!(
                "<desc_returns xml:space=\"preserve\">\n",
                "    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"a or b\" reftype=\"class\">\n",
                "        a or b\n",
            )
        );
    }
}
