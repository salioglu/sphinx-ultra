//! Arglist and PEP-695 type-parameter-list parsing for the py domain: the
//! `_parse_arglist` / `_pseudo_parse_arglist` / `_parse_type_list` port
//! (`sphinx/domains/python/_annotations.py:254-619`, sphinx 9.1.0) plus the
//! `signature_from_str` grammar (`sphinx/util/inspect.py:967-1038`) and the
//! `multi_line_parameter_list` measurement (`_object.py:291-312`).
//!
//! Ground truth, cited throughout as [PY §n] / [SIG §n]:
//! - [PY] docs/plans/2026-09-01-m2-wave4.5-research-spec-py-domain.md
//!   (§2.1 arglist node shapes, §2.2 pseudo fallback, §2.4 type parameter
//!   lists, §1.6 probe outputs);
//! - [SIG] docs/plans/2026-09-01-m2-wave4.5-research-spec-signature-config.md
//!   (§1.3 measurement, §1.4 attr placement, appendix A.1 matrix).
//!
//! Expected pformats in the test module are verbatim probe output against the
//! pinned toolchain (sphinx 9.1.0 / docutils 0.22.4, harness3 conventions);
//! cases cited as `probe <case>` come from the spec's §1.6/§A.1 blocks or the
//! task-5 probe run (`probe task5/<case>`, logged in the task-5 report).
//!
//! ## The two render pipelines (trap)
//!
//! Defaults and annotations do NOT round through CPython's `ast.unparse`
//! (which task 3's [`expr::unparse`] mirrors): `signature_from_str` routes
//! them through `sphinx.pycode.ast.unparse` — "a greatly cut-down version of
//! `ast._Unparser`" — whose rules differ observably (probe
//! task5/default_normalized: `f(x=0xFF, y=[1,2])` renders `0xFF` and
//! `[1, 2]`):
//! - int/float constants keep their SOURCE text via `ast.get_source_segment`
//!   (`0xFF`, `1_000`, `1e5`), falling back to `repr` only when the segment
//!   is unavailable;
//! - no precedence parentheses at all (`(a+b)*c` → `a + b * c`);
//! - `**` is rendered without surrounding spaces (`a**b`);
//! - unary operators never parenthesize (`-(a+b)` → `-a + b`);
//! - a `u''` string prefix is dropped (`repr` of the value).
//!
//! [`pycode_unparse`] implements those rules over the task-3 [`PyExpr`] AST;
//! annotation *nodes* are still rendered by task 4's
//! [`parse_annotation`], which re-parses the pycode-normalized string exactly
//! as `_parse_annotation(param.annotation)` does (`_annotations.py:495`).
//!
//! ## Documented divergences (all conservative)
//!
//! - Expressions outside the task-3 subset (lambdas, comparisons, ternaries,
//!   slices, f-strings, starred `**` dict unpacks, complex literals) make
//!   [`signature_from_str`] return [`SigParseError::Syntax`], routing task
//!   6 into the silent pseudo fallback. Sphinx renders most of these (or
//!   warns via `NotImplementedError`/`ValueError` for `a == b`, `a if b
//!   else c`, f-strings and `{**a}`), so the fallback shape — and a missing
//!   warning — can diverge for such signatures.
//! - A top-level `lambda` keyword in the arglist is rejected before comma
//!   splitting (its parameter commas are not bracket-protected, so no naive
//!   split is faithful); sphinx parses it and prints `lambda a, b: ...`.
//! - Numeric source recovery maps number tokens to constants in render
//!   order and verifies each token re-parses to the same value; when the
//!   mapping is ambiguous the whole fragment falls back to `repr` form
//!   (normalized digits) where sphinx would keep source text.
//!   `visit_Constant` recovers its text by AST POSITION
//!   (`ast.get_source_segment`), so render order only has to agree with
//!   source order — which it now does everywhere except one shape:
//!   `ast.Call` splits `args` from `keywords`, losing their interleaving,
//!   so a positional (necessarily a `*` unpack) written AFTER a keyword
//!   argument is rendered before it and consumes the earlier token. The
//!   text order still matches sphinx (`visit_Call` reorders identically),
//!   but a non-decimal spelling in such a fragment lands in the `repr`
//!   fallback: `f(k=0x1, *a(0x2))` prints `f(*a(2), k=1)` where sphinx
//!   keeps `f(*a(0x2), k=0x1)`. Fixing it needs source spans on
//!   [`PyConst`], not a render-order tweak.
//! - Type-parameter tokenization errors surface as
//!   [`SigParseError::Syntax`] with an approximate message where sphinx's
//!   warning embeds the exact `tokenize.TokenError` text.
//! - [`pseudo_parse_arglist`] takes `(ctx, cfg)` in addition to the brief's
//!   `(arglist, multi_line)`: the pseudo path renders annotation xrefs
//!   through `_parse_annotation` and stamps `multi_line_trailing_comma`
//!   from `python_trailing_comma_in_multi_line_signatures`
//!   (`_object.py:363-380`), neither of which is derivable without them.
//! - [`multi_line_flags`] measures Python `len(sig)` — Unicode scalar
//!   count — while [`PySigMatch`] spans are byte offsets; widths are
//!   computed as the char count of the spanned slice, so non-ASCII
//!   signatures measure exactly as CPython does.

use std::collections::HashSet;
use std::fmt;

use crate::doctree::{kinds, AttrValue, Node, Span};

use super::annotations::{
    desc_sig_operator, desc_sig_punctuation, desc_sig_space, parse_annotation, PyRefContext,
};
use super::expr::{
    self, parse_py_expr, parse_py_star_annotation, PyBoolOp, PyConst, PyExpr, PyOp, PyUnaryOp,
};
use super::PySigConfig;

/// `inspect._ParameterKind`, as classified by `signature_from_ast`
/// (`sphinx/util/inspect.py:976-1025`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    PositionalOnly,
    PositionalOrKeyword,
    VarPositional,
    KeywordOnly,
    VarKeyword,
}

/// One parameter of a parsed arglist. `annotation` and `default` are
/// `sphinx.pycode.ast.unparse`-normalized source strings, exactly what
/// `inspect.Parameter.annotation` / `DefaultValue` carry in sphinx
/// (`util/inspect.py:1027-1038`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyParam {
    pub name: String,
    pub kind: ParamKind,
    pub annotation: Option<String>,
    pub default: Option<String>,
}

/// Why an arglist / type-parameter list failed to parse. The two variants
/// carry sphinx's two observable failure channels ([PY §1.3 step 5],
/// `_object.py:355-381`):
///
/// - [`SigParseError::Syntax`] — `ast.parse` `SyntaxError`: task 6 logs at
///   debug level (invisible) and falls back to [`pseudo_parse_arglist`];
///   for a tp-list, any failure is a warning (`_object.py:342-345`).
/// - [`SigParseError::Duplicate`] — `inspect.Signature`'s
///   `ValueError('duplicate parameter name: ...')`: task 6 emits WARNING
///   `could not parse arglist (%r): %s` and falls back to pseudo. Note
///   `ast.parse` accepts duplicate `def` parameters (the check lives in the
///   symtable pass), so this genuinely reaches `Signature.__init__`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigParseError {
    /// A `SyntaxError`-equivalent; the message approximates CPython's and
    /// is only ever surfaced on the (warning) tp-list path.
    Syntax(String),
    /// Duplicate parameter name (the payload is the offending name).
    Duplicate(String),
}

impl fmt::Display for SigParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SigParseError::Syntax(msg) => f.write_str(msg),
            // CPython `inspect.Signature`: 'duplicate parameter name: {name!r}'.
            SigParseError::Duplicate(name) => write!(f, "duplicate parameter name: '{name}'"),
        }
    }
}

impl std::error::Error for SigParseError {}

/// The `py_sig_re` match carrier (`_object.py:41-50`): groups (prefix,
/// name, tp_list, arglist, retann) plus the two inner-text spans the
/// multi-line measurement subtracts (`_object.py:300-311`). Task 6's
/// `handle_py_signature` constructs this from its matcher; spans are BYTE
/// offsets of the group's inner text within the stripped signature,
/// `(0, 0)` when the group did not participate (Python's `(-1, -1)` span
/// normalizes to width 0 either way).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PySigMatch {
    /// Group 1: dotted class prefix including the trailing `.`.
    pub prefix: Option<String>,
    /// Group 2: the object name.
    pub name: String,
    /// Group 3: inner text of the `[type params]` brackets.
    pub tp_list: Option<String>,
    /// Group 4: inner text of the `(...)` parens.
    pub arglist: Option<String>,
    /// Group 5: return annotation after `->`.
    pub retann: Option<String>,
    /// Byte span of group 3's inner text within the stripped sig.
    pub tp_span: (usize, usize),
    /// Byte span of group 4's inner text within the stripped sig.
    pub arg_span: (usize, usize),
}

/// The two `single-line-*` directive flags (`_object.py:180-181`); each
/// suppresses only its own list ([SIG §1.5], probes D1/D2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SingleLineOpts {
    /// `:single-line-parameter-list:` present.
    pub parameter_list: bool,
    /// `:single-line-type-parameter-list:` present.
    pub type_parameter_list: bool,
}

/// The `(multi_line_parameter_list, multi_line_type_parameter_list)` pair
/// (`_object.py:300-311`, [SIG §1.3]): each flag is
/// `!single-line-option && (len(sig) - width(other group)) > max_len > 0`,
/// strictly greater, with `max_len` resolved by [`PySigConfig::max_len`].
/// Lengths are Python `len()` — Unicode scalar counts — computed from the
/// byte spans' slices; an out-of-range span counts as width 0.
pub fn multi_line_flags(
    sig: &str,
    m: &PySigMatch,
    opts: SingleLineOpts,
    cfg: &PySigConfig,
) -> (bool, bool) {
    let max_len = cfg.max_len();
    let sig_len = sig.chars().count() as i64;
    let width = |span: (usize, usize)| -> i64 {
        sig.get(span.0..span.1)
            .map(|s| s.chars().count() as i64)
            .unwrap_or(0)
    };
    let parameter_list =
        !opts.parameter_list && (sig_len - width(m.tp_span)) > max_len && max_len > 0;
    let type_parameter_list =
        !opts.type_parameter_list && (sig_len - width(m.arg_span)) > max_len && max_len > 0;
    (parameter_list, type_parameter_list)
}

/// Parse `arglist` with the grammar of `def func(<arglist>): pass`
/// (`sphinx/util/inspect.py:967-974`): positional-only via `/`,
/// keyword-only after a bare `*` or `*args`, `**kwargs` last. Defaults and
/// annotations are validated by task 3's [`parse_py_expr`] and rendered by
/// [`pycode_unparse`]; any rejected fragment or grammar violation is
/// [`SigParseError::Syntax`], and duplicate parameter names — which
/// `ast.parse` accepts — are [`SigParseError::Duplicate`] exactly as
/// `inspect.Signature.__init__` raises `ValueError` after a clean parse.
pub fn signature_from_str(arglist: &str) -> Result<Vec<PyParam>, SigParseError> {
    if arglist.trim().is_empty() {
        return Ok(Vec::new());
    }
    let toks = lex(arglist).ok_or_else(invalid_syntax)?;
    let items = split_top_level(&toks)?;

    let mut params: Vec<PyParam> = Vec::new();
    let mut seen_slash = false;
    let mut seen_star = false;
    let mut bare_star = false;
    let mut seen_kwargs = false;
    let mut kwonly_count = 0usize;
    let mut pos_default_seen = false;

    let last = items.len() - 1;
    for (i, item) in items.iter().enumerate() {
        let shape = classify_item(item);
        if matches!(shape, ItemShape::Empty) {
            // A trailing comma leaves one empty final item; any other empty
            // item is CPython's plain "invalid syntax".
            if i == last && i > 0 {
                continue;
            }
            return Err(invalid_syntax());
        }
        if seen_kwargs {
            return Err(SigParseError::Syntax(
                "arguments cannot follow var-keyword argument".to_string(),
            ));
        }
        match shape {
            ItemShape::Empty => unreachable!("handled above"),
            ItemShape::Slash => {
                if seen_star {
                    return Err(SigParseError::Syntax("/ must be ahead of *".to_string()));
                }
                if seen_slash {
                    return Err(SigParseError::Syntax("/ may appear only once".to_string()));
                }
                if params.is_empty() {
                    return Err(SigParseError::Syntax(
                        "at least one argument must precede /".to_string(),
                    ));
                }
                seen_slash = true;
                for p in &mut params {
                    p.kind = ParamKind::PositionalOnly;
                }
            }
            ItemShape::BareStar => {
                if seen_star {
                    return Err(SigParseError::Syntax(
                        "* argument may appear only once".to_string(),
                    ));
                }
                seen_star = true;
                bare_star = true;
            }
            ItemShape::VarArgs(rest) => {
                if seen_star {
                    return Err(SigParseError::Syntax(
                        "* argument may appear only once".to_string(),
                    ));
                }
                seen_star = true;
                let raw = parse_param_tokens(rest, arglist, true)?;
                if raw.default.is_some() {
                    return Err(SigParseError::Syntax(
                        "var-positional argument cannot have default value".to_string(),
                    ));
                }
                params.push(PyParam {
                    name: raw.name,
                    kind: ParamKind::VarPositional,
                    annotation: raw.annotation,
                    default: None,
                });
            }
            ItemShape::KwArgs(rest) => {
                let raw = parse_param_tokens(rest, arglist, false)?;
                if raw.default.is_some() {
                    return Err(SigParseError::Syntax(
                        "var-keyword argument cannot have default value".to_string(),
                    ));
                }
                seen_kwargs = true;
                params.push(PyParam {
                    name: raw.name,
                    kind: ParamKind::VarKeyword,
                    annotation: raw.annotation,
                    default: None,
                });
            }
            ItemShape::Plain(toks) => {
                let raw = parse_param_tokens(toks, arglist, false)?;
                let kind = if seen_star {
                    kwonly_count += 1;
                    ParamKind::KeywordOnly
                } else {
                    // The non-default-after-default rule spans `/` but not
                    // `*`: `f(a=1, /, b)` is a SyntaxError while
                    // `f(a=1, *, b)` is fine (probed, task-5 report).
                    if raw.default.is_none() && pos_default_seen {
                        return Err(SigParseError::Syntax(
                            "parameter without a default follows parameter with a default"
                                .to_string(),
                        ));
                    }
                    if raw.default.is_some() {
                        pos_default_seen = true;
                    }
                    ParamKind::PositionalOrKeyword
                };
                params.push(PyParam {
                    name: raw.name,
                    kind,
                    annotation: raw.annotation,
                    default: raw.default,
                });
            }
        }
    }
    if bare_star && kwonly_count == 0 {
        return Err(SigParseError::Syntax(
            "named arguments must follow bare *".to_string(),
        ));
    }

    // `inspect.Signature.__init__`: first duplicate in sequence order wins.
    let mut seen: HashSet<&str> = HashSet::new();
    for p in &params {
        if !seen.insert(p.name.as_str()) {
            return Err(SigParseError::Duplicate(p.name.clone()));
        }
    }
    Ok(params)
}

/// Port of `_parse_arglist` (`_annotations.py:462-516`, [PY §2.1]): a
/// `desc_parameterlist` carrying `multi_line_parameter_list` /
/// `multi_line_trailing_comma` UNCONDITIONALLY ([SIG §1.4]), one
/// `desc_parameter` per param, `/` and `*` separator parameters between
/// kind transitions, and the trailing-`/` epilogue when the list ends
/// positional-only.
pub fn parse_arglist(
    arglist: &str,
    multi_line: bool,
    ctx: &PyRefContext,
    cfg: &PySigConfig,
) -> Result<Node, SigParseError> {
    let sig_params = signature_from_str(arglist)?;
    let mut params = attr_list_node("desc_parameterlist", multi_line, cfg);
    let mut last_kind: Option<ParamKind> = None;
    for param in &sig_params {
        if param.kind != ParamKind::PositionalOnly && last_kind == Some(ParamKind::PositionalOnly) {
            params.children.push(positional_only_separator());
        }
        if param.kind == ParamKind::KeywordOnly
            && matches!(
                last_kind,
                Some(ParamKind::PositionalOrKeyword) | Some(ParamKind::PositionalOnly) | None
            )
        {
            params.children.push(keyword_only_separator());
        }

        let mut node = desc_parameter();
        match param.kind {
            ParamKind::VarPositional => {
                node.children.push(desc_sig_operator("*"));
                node.children.push(desc_sig_name_text(&param.name));
            }
            ParamKind::VarKeyword => {
                node.children.push(desc_sig_operator("**"));
                node.children.push(desc_sig_name_text(&param.name));
            }
            _ => node.children.push(desc_sig_name_text(&param.name)),
        }
        if let Some(ann) = &param.annotation {
            node.children.push(desc_sig_punctuation(":"));
            node.children.push(desc_sig_space());
            node.children
                .push(annotation_wrapper(parse_annotation(ann, ctx, cfg)));
        }
        if let Some(default) = &param.default {
            if param.annotation.is_some() {
                node.children.push(desc_sig_space());
                node.children.push(desc_sig_operator("="));
                node.children.push(desc_sig_space());
            } else {
                node.children.push(desc_sig_operator("="));
            }
            node.children.push(default_value_inline(default));
        }
        params.children.push(node);
        last_kind = Some(param.kind);
    }
    // Loop epilogue (`_annotations.py:513-514`): `func(a, /)`.
    if last_kind == Some(ParamKind::PositionalOnly) {
        params.children.push(positional_only_separator());
    }
    Ok(params)
}

/// Port of `_pseudo_parse_arglist` (`_annotations.py:541-619`, [PY §2.2]):
/// comma-split fallback with `[`/`]` push/pop of `desc_optional`,
/// `name[:annotation][=default]` partition, `=` always a bare
/// `desc_sig_operator` (space-wrapped only when annotated), and — on total
/// bracket imbalance — a fresh paramlist containing the raw arglist as one
/// `desc_parameter`, with NO multi_line attributes (the one attr-less
/// exception, [SIG §1.4]).
///
/// Signature note: sphinx's version takes `(signode, arglist, *,
/// multi_line_parameter_list, trailing_comma, env)`; ours returns the
/// paramlist node and reads `trailing_comma` from `cfg` / annotation
/// context from `ctx` (see module docs).
pub fn pseudo_parse_arglist(
    arglist: &str,
    multi_line: bool,
    ctx: &PyRefContext,
    cfg: &PySigConfig,
) -> Node {
    let list = attr_list_node("desc_parameterlist", multi_line, cfg);
    match pseudo_build(arglist, list, ctx, cfg) {
        Some(done) => done,
        None => {
            // "just give up and treat the whole argument list as one
            // argument" (`_annotations.py:609-617`): fresh list, no attrs.
            let mut fresh = Node::elem("desc_parameterlist", Span::ZERO);
            fresh.set("xml:space", AttrValue::Str("preserve".to_string()));
            let mut par = desc_parameter();
            if !arglist.is_empty() {
                par.children.push(Node::text_node(arglist, Span::ZERO));
            }
            fresh.children.push(par);
            fresh
        }
    }
}

/// Port of `_parse_type_list` + `_TypeParameterListParser`
/// (`_annotations.py:254-459`, [PY §2.4]): a `desc_type_parameter_list`
/// with the same two multi_line attrs, one `desc_type_parameter` per
/// param, `*`/`**` operators for variadics, bounds/constraints after
/// `:`+space inside a `desc_sig_name` wrapper (constraints
/// re-parenthesized), and `␣=␣` + `default_value` inline defaults.
pub fn parse_type_list(
    tp_list: &str,
    multi_line: bool,
    ctx: &PyRefContext,
    cfg: &PySigConfig,
) -> Result<Node, SigParseError> {
    // `_TypeParameterListParser.__init__`: sig.replace('\n', '').strip().
    let cleaned = tp_list.replace('\n', "");
    let cleaned = cleaned.trim();
    let toks = lex(cleaned).ok_or_else(invalid_syntax)?;
    // `tokenize` raises TokenError('unexpected EOF in multi-line
    // statement', (lnum, 0)) for unclosed brackets (extra closers are
    // lenient); the parser's caller catches any Exception into the
    // tp-list warning, where `%s` renders the two-arg exception as its
    // args-tuple repr — and `lnum` is always 1 because the parser joins
    // the tp-list to one line first. Probe tp_list_tokerror pins the
    // rendered bytes verbatim.
    let mut level = 0i64;
    for t in &toks {
        if t.kind == TokKind::Op {
            match t.text.as_str() {
                "(" | "[" | "{" => level += 1,
                ")" | "]" | "}" => level -= 1,
                _ => {}
            }
        }
    }
    if level > 0 {
        return Err(SigParseError::Syntax(
            "('unexpected EOF in multi-line statement', (1, 0))".to_string(),
        ));
    }

    let type_params = tp_parse(&toks)?;

    let mut list = attr_list_node("desc_type_parameter_list", multi_line, cfg);
    for tp in &type_params {
        let mut node = Node::elem("desc_type_parameter", Span::ZERO);
        node.set("xml:space", AttrValue::Str("preserve".to_string()));
        match tp.kind {
            ParamKind::VarPositional => node.children.push(desc_sig_operator("*")),
            ParamKind::VarKeyword => node.children.push(desc_sig_operator("**")),
            _ => {}
        }
        node.children.push(desc_sig_name_text(&tp.name));

        if let Some(ann_text) = &tp.annotation {
            let children = parse_annotation(ann_text, ctx, cfg);
            if children.is_empty() {
                // `if not annotation: continue` (`_annotations.py:428-430`)
                // drops the whole parameter, default included.
                continue;
            }
            node.children.push(desc_sig_punctuation(":"));
            node.children.push(desc_sig_space());
            let wrapper = annotation_wrapper(children);
            // A type bound is `T: U`; constraints are parenthesized
            // `T: (U, V)` — and `_parse_annotation` loses tuple parens, so
            // they are re-added around the wrapper (`_annotations.py:434-445`).
            if ann_text.starts_with('(') && ann_text.ends_with(')') {
                let text = wrapper.astext();
                if text.starts_with('(') && text.ends_with(')') {
                    node.children.push(wrapper);
                } else {
                    node.children.push(desc_sig_punctuation("("));
                    node.children.push(wrapper);
                    node.children.push(desc_sig_punctuation(")"));
                }
            } else {
                node.children.push(wrapper);
            }
        }
        if let Some(default) = &tp.default {
            // "Always surround '=' with spaces, even if there is no
            // annotation" (`_annotations.py:449-456`) — unlike arglists.
            node.children.push(desc_sig_space());
            node.children.push(desc_sig_operator("="));
            node.children.push(desc_sig_space());
            node.children.push(default_value_inline(default));
        }
        list.children.push(node);
    }
    Ok(list)
}

// ---------------------------------------------------------------------------
// Node builders ([PY §2.1/§2.6] shapes)
// ---------------------------------------------------------------------------

/// `desc_parameterlist` / `desc_type_parameter_list` with the two
/// multi_line attributes set unconditionally ([SIG §1.4]) and docutils'
/// `FixedTextElement` `xml:space="preserve"`.
fn attr_list_node(kind: &'static str, multi_line: bool, cfg: &PySigConfig) -> Node {
    let mut node = Node::elem(kind, Span::ZERO);
    node.set(
        "multi_line_parameter_list",
        AttrValue::Int(i64::from(multi_line)),
    );
    node.set(
        "multi_line_trailing_comma",
        AttrValue::Int(i64::from(
            cfg.python_trailing_comma_in_multi_line_signatures,
        )),
    );
    node.set("xml:space", AttrValue::Str("preserve".to_string()));
    node
}

fn desc_parameter() -> Node {
    let mut node = Node::elem("desc_parameter", Span::ZERO);
    node.set("xml:space", AttrValue::Str("preserve".to_string()));
    node
}

fn desc_optional() -> Node {
    let mut node = Node::elem("desc_optional", Span::ZERO);
    node.set("xml:space", AttrValue::Str("preserve".to_string()));
    node
}

/// `desc_sig_name(text)` that mirrors docutils `TextElement('', text)`:
/// an empty text adds NO text child (the pseudo parser can produce empty
/// parameter names, e.g. for `=x`).
fn desc_sig_name_text(text: &str) -> Node {
    if text.is_empty() {
        let mut node = Node::elem("desc_sig_name", Span::ZERO);
        node.attrs.classes.push("n".to_string());
        node
    } else {
        super::annotations::desc_sig_name(text)
    }
}

/// `desc_sig_name('', '', *children)` — the classes-`n` wrapper around a
/// rendered annotation (`_annotations.py:498`).
fn annotation_wrapper(children: Vec<Node>) -> Node {
    let mut node = Node::elem("desc_sig_name", Span::ZERO);
    node.attrs.classes.push("n".to_string());
    node.children = children;
    node
}

/// `nodes.inline('', text, classes=['default_value'],
/// support_smartquotes=False)` (`_annotations.py:505-508`).
fn default_value_inline(text: &str) -> Node {
    let mut node = Node::elem("inline", Span::ZERO);
    node.attrs.classes.push("default_value".to_string());
    node.set("support_smartquotes", AttrValue::Int(0));
    if !text.is_empty() {
        node.children.push(Node::text_node(text, Span::ZERO));
    }
    node
}

/// `_positional_only_separator()` / `_keyword_only_separator()`
/// (`_annotations.py:519-538`): `desc_parameter > desc_sig_operator(classes
/// [<separator class>, "o"]) > abbreviation(explanation=<PEP text>)`.
fn separator(op_text: &str, class: &str, explanation: &str) -> Node {
    let mut abbr = Node::elem(kinds::ABBREVIATION, Span::ZERO);
    abbr.set("explanation", AttrValue::Str(explanation.to_string()));
    abbr.children.push(Node::text_node(op_text, Span::ZERO));
    let mut op = Node::elem("desc_sig_operator", Span::ZERO);
    op.attrs.classes.push(class.to_string());
    op.attrs.classes.push("o".to_string());
    op.children.push(abbr);
    let mut par = desc_parameter();
    par.children.push(op);
    par
}

fn positional_only_separator() -> Node {
    separator(
        "/",
        "positional-only-separator",
        "Positional-only parameter separator (PEP 570)",
    )
}

fn keyword_only_separator() -> Node {
    separator(
        "*",
        "keyword-only-separator",
        "Keyword-only parameters separator (PEP 3102)",
    )
}

// ---------------------------------------------------------------------------
// Pseudo parser internals ([PY §2.2])
// ---------------------------------------------------------------------------

/// The happy path of `_pseudo_parse_arglist`; `None` reproduces every
/// `IndexError` route into the give-up fallback (too many `]`, unclosed
/// `[`, operations after the root was popped).
fn pseudo_build(arglist: &str, list: Node, ctx: &PyRefContext, cfg: &PySigConfig) -> Option<Node> {
    let mut stack: Vec<Node> = vec![list];
    for argument in arglist.split(',') {
        let mut argument = argument.trim();
        let mut ends_open = 0usize;
        let mut ends_close = 0usize;
        while let Some(rest) = argument.strip_prefix('[') {
            stack_push(&mut stack)?;
            argument = rest.trim();
        }
        while let Some(rest) = argument.strip_prefix(']') {
            stack_pop(&mut stack)?;
            argument = rest.trim();
        }
        while argument.ends_with(']') && !argument.ends_with("[]") {
            ends_close += 1;
            argument = argument[..argument.len() - 1].trim();
        }
        while let Some(rest) = argument.strip_suffix('[') {
            ends_open += 1;
            argument = rest.trim();
        }
        if !argument.is_empty() {
            // `argument.partition('=')` then `.partition(':')` — first
            // occurrence, no bracket awareness (`_annotations.py:578-600`).
            let (param_with_annotation, default_value) = match argument.split_once('=') {
                Some((head, tail)) => (head, tail),
                None => (argument, ""),
            };
            let (param_name, annotation) = match param_with_annotation.split_once(':') {
                Some((head, tail)) => (head, tail),
                None => (param_with_annotation, ""),
            };
            let mut node = desc_parameter();
            node.children.push(desc_sig_name_text(param_name.trim()));
            if !annotation.is_empty() {
                node.children.push(desc_sig_punctuation(":"));
                node.children.push(desc_sig_space());
                node.children.push(annotation_wrapper(parse_annotation(
                    annotation.trim(),
                    ctx,
                    cfg,
                )));
            }
            if !default_value.is_empty() {
                if !annotation.is_empty() {
                    node.children.push(desc_sig_space());
                }
                node.children.push(desc_sig_operator("="));
                if !annotation.is_empty() {
                    node.children.push(desc_sig_space());
                }
                node.children
                    .push(default_value_inline(default_value.trim()));
            }
            stack.last_mut()?.children.push(node);
        }
        for _ in 0..ends_open {
            stack_push(&mut stack)?;
        }
        for _ in 0..ends_close {
            stack_pop(&mut stack)?;
        }
    }
    if stack.len() == 1 {
        stack.pop()
    } else {
        None
    }
}

/// Python push (`stack.append(desc_optional()); stack[-2] += stack[-1]`):
/// attaching to a missing parent is the IndexError give-up.
fn stack_push(stack: &mut Vec<Node>) -> Option<()> {
    if stack.is_empty() {
        return None;
    }
    stack.push(desc_optional());
    Some(())
}

/// Python `stack.pop()`. Children are attached at pop time (Python attaches
/// at push time by reference; behaviorally identical because the give-up
/// path discards the whole tree). Popping the root succeeds — exactly like
/// Python — and every subsequent operation then fails.
fn stack_pop(stack: &mut Vec<Node>) -> Option<()> {
    let top = stack.pop()?;
    if let Some(parent) = stack.last_mut() {
        parent.children.push(top);
    }
    Some(())
}

// ---------------------------------------------------------------------------
// Lexer: Python-shaped tokens for arglist splitting, numeric source
// recovery and the tp-list TokenProcessor mirror
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokKind {
    Name,
    Number,
    Str,
    Op,
}

#[derive(Debug, Clone)]
struct Tok {
    kind: TokKind,
    text: String,
    /// Byte span in the lexed string.
    start: usize,
    end: usize,
}

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_name_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Python string-literal prefixes: 1-2 letters from rRbBuUfF.
fn is_string_prefix(word: &str) -> bool {
    !word.is_empty() && word.len() <= 2 && word.chars().all(|c| "rRbBuUfF".contains(c))
}

const OPS3: &[&str] = &["**=", "//=", "<<=", ">>=", "..."];
const OPS2: &[&str] = &[
    "**", "//", "<<", ">>", "<=", ">=", "==", "!=", "->", ":=", "+=", "-=", "*=", "/=", "%=", "@=",
    "&=", "|=", "^=",
];

/// Tokenize with Python-`tokenize`-shaped boundaries: names (Unicode),
/// numbers (source text kept verbatim), strings (prefixes, triple quotes,
/// backslash escapes) and maximal-munch operators. Whitespace separates.
/// `None` on an unterminated string. Unknown characters become single-char
/// Op tokens (Python's ERRORTOKEN leniency); real validity is decided by
/// [`parse_py_expr`] on the fragments.
fn lex(src: &str) -> Option<Vec<Tok>> {
    let mut toks: Vec<Tok> = Vec::new();
    let mut iter = src.char_indices().peekable();
    while let Some(&(i, c)) = iter.peek() {
        if c.is_whitespace() {
            iter.next();
            continue;
        }
        if is_name_start(c) {
            let mut end = i + c.len_utf8();
            iter.next();
            while let Some(&(j, d)) = iter.peek() {
                if is_name_continue(d) {
                    end = j + d.len_utf8();
                    iter.next();
                } else {
                    break;
                }
            }
            let word = &src[i..end];
            if let Some(&(_, q)) = iter.peek() {
                if (q == '\'' || q == '"') && is_string_prefix(word) {
                    let send = lex_string(&mut iter)?;
                    toks.push(Tok {
                        kind: TokKind::Str,
                        text: src[i..send].to_string(),
                        start: i,
                        end: send,
                    });
                    continue;
                }
            }
            toks.push(Tok {
                kind: TokKind::Name,
                text: word.to_string(),
                start: i,
                end,
            });
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && peek2_is_digit(&iter)) {
            let end = lex_number(src, &mut iter);
            toks.push(Tok {
                kind: TokKind::Number,
                text: src[i..end].to_string(),
                start: i,
                end,
            });
            continue;
        }
        if c == '\'' || c == '"' {
            let send = lex_string(&mut iter)?;
            toks.push(Tok {
                kind: TokKind::Str,
                text: src[i..send].to_string(),
                start: i,
                end: send,
            });
            continue;
        }
        // Operators: maximal munch 3-2-1.
        let rest = &src[i..];
        let mut matched = None;
        for cand in OPS3.iter().chain(OPS2.iter()) {
            if rest.starts_with(*cand) {
                matched = Some(*cand);
                break;
            }
        }
        let op_len = matched.map_or(c.len_utf8(), str::len);
        let end = i + op_len;
        toks.push(Tok {
            kind: TokKind::Op,
            text: src[i..end].to_string(),
            start: i,
            end,
        });
        for _ in 0..src[i..end].chars().count() {
            iter.next();
        }
    }
    Some(toks)
}

fn peek2_is_digit(iter: &std::iter::Peekable<std::str::CharIndices<'_>>) -> bool {
    let mut it = iter.clone();
    it.next();
    matches!(it.peek(), Some(&(_, d)) if d.is_ascii_digit())
}

/// Consume a numeric literal, returning its end byte offset. Keeps the
/// source text verbatim (that is the whole point — `ast.get_source_segment`
/// parity); boundaries approximate Python's number token: alnum, `_`, `.`,
/// plus a sign directly after an exponent `e`/`E` (never in radix-prefixed
/// literals, so `0xEF+1` splits after `0xEF`).
fn lex_number(src: &str, iter: &mut std::iter::Peekable<std::str::CharIndices<'_>>) -> usize {
    let (start, first) = *iter.peek().expect("caller peeked a digit");
    let radix_prefixed = {
        let rest = &src[start..];
        rest.len() >= 2
            && rest.starts_with('0')
            && matches!(
                rest[1..].chars().next(),
                Some('x' | 'X' | 'b' | 'B' | 'o' | 'O')
            )
    };
    let mut end = start + first.len_utf8();
    let mut prev = first;
    iter.next();
    while let Some(&(j, d)) = iter.peek() {
        let continues = d.is_ascii_alphanumeric()
            || d == '_'
            || d == '.'
            || ((d == '+' || d == '-') && matches!(prev, 'e' | 'E') && !radix_prefixed);
        if continues {
            end = j + d.len_utf8();
            prev = d;
            iter.next();
        } else {
            break;
        }
    }
    end
}

/// Consume a string literal starting at the opening quote; returns the end
/// byte offset past the closing quote, or `None` when unterminated. A
/// backslash always escapes the next character for termination purposes
/// (true even for raw strings in Python's tokenizer).
fn lex_string(iter: &mut std::iter::Peekable<std::str::CharIndices<'_>>) -> Option<usize> {
    let (_, quote) = iter.next()?;
    // Triple quote?
    let mut probe = iter.clone();
    let triple = matches!(
        (probe.next(), probe.next()),
        (Some((_, a)), Some((_, b))) if a == quote && b == quote
    );
    if triple {
        iter.next();
        iter.next();
        loop {
            let (_, c) = iter.next()?;
            if c == '\\' {
                iter.next();
                continue;
            }
            if c == quote {
                let mut probe = iter.clone();
                if matches!(
                    (probe.next(), probe.next()),
                    (Some((_, a)), Some((_, b))) if a == quote && b == quote
                ) {
                    iter.next();
                    let (j, q) = iter.next().expect("probed above");
                    return Some(j + q.len_utf8());
                }
            }
        }
    } else {
        loop {
            let (j, c) = iter.next()?;
            if c == '\\' {
                iter.next();
                continue;
            }
            if c == quote {
                return Some(j + c.len_utf8());
            }
            if c == '\n' {
                return None;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Arglist splitting and per-parameter token parsing
// ---------------------------------------------------------------------------

fn invalid_syntax() -> SigParseError {
    SigParseError::Syntax("invalid syntax".to_string())
}

/// Split the token stream at depth-0 commas. Bracket imbalance (either
/// direction) is `ast.parse`'s SyntaxError; a depth-0 `lambda` keyword is
/// rejected up front (see module docs — its parameter commas would defeat
/// any comma split, and the task-3 subset rejects lambdas anyway).
fn split_top_level(toks: &[Tok]) -> Result<Vec<&[Tok]>, SigParseError> {
    let mut items: Vec<&[Tok]> = Vec::new();
    let mut depth = 0i64;
    let mut start = 0usize;
    for (idx, t) in toks.iter().enumerate() {
        match t.kind {
            TokKind::Op => match t.text.as_str() {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => {
                    depth -= 1;
                    if depth < 0 {
                        return Err(invalid_syntax());
                    }
                }
                "," if depth == 0 => {
                    items.push(&toks[start..idx]);
                    start = idx + 1;
                }
                _ => {}
            },
            TokKind::Name if depth == 0 && t.text == "lambda" => {
                return Err(invalid_syntax());
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(invalid_syntax());
    }
    items.push(&toks[start..]);
    Ok(items)
}

enum ItemShape<'t> {
    Empty,
    Slash,
    BareStar,
    VarArgs(&'t [Tok]),
    KwArgs(&'t [Tok]),
    Plain(&'t [Tok]),
}

fn classify_item<'t>(toks: &'t [Tok]) -> ItemShape<'t> {
    let Some(first) = toks.first() else {
        return ItemShape::Empty;
    };
    if first.kind == TokKind::Op {
        match first.text.as_str() {
            "/" if toks.len() == 1 => return ItemShape::Slash,
            "*" if toks.len() == 1 => return ItemShape::BareStar,
            "**" => return ItemShape::KwArgs(&toks[1..]),
            "*" => return ItemShape::VarArgs(&toks[1..]),
            _ => {}
        }
    }
    ItemShape::Plain(toks)
}

struct RawParam {
    name: String,
    annotation: Option<String>,
    default: Option<String>,
}

/// `name[: annotation][= default]` over one item's tokens. The name must
/// be a single Name token that task 3 parses as `PyExpr::Name` (rejecting
/// keywords, applying CPython's NFKC identifier normalization).
///
/// `star_annotation` selects CPython's `star_annotation` production for
/// the annotation slot: `def f(*args: *Ts)` is legal (PEP 646) and
/// `signature_from_str` hands sphinx the annotation string `*Ts`, while
/// `def f(x: *Ts)` and `def f(**k: *Ts)` are SyntaxErrors — so only the
/// var-positional caller passes `true`.
fn parse_param_tokens(
    toks: &[Tok],
    src: &str,
    star_annotation: bool,
) -> Result<RawParam, SigParseError> {
    if toks.is_empty() {
        return Err(invalid_syntax());
    }
    let mut depth = 0i64;
    let mut colon: Option<usize> = None;
    let mut eq: Option<usize> = None;
    for (i, t) in toks.iter().enumerate() {
        if t.kind == TokKind::Op {
            match t.text.as_str() {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                ":" if depth == 0 && colon.is_none() && eq.is_none() => colon = Some(i),
                "=" if depth == 0 && eq.is_none() => eq = Some(i),
                _ => {}
            }
        }
    }
    let name_end = colon.or(eq).unwrap_or(toks.len());
    let name_toks = &toks[..name_end];
    if name_toks.len() != 1 || name_toks[0].kind != TokKind::Name {
        return Err(invalid_syntax());
    }
    let name = match parse_py_expr(&name_toks[0].text) {
        Ok(PyExpr::Name(n)) => n,
        _ => return Err(invalid_syntax()),
    };
    let ann_end = eq.unwrap_or(toks.len());
    let annotation = match colon {
        Some(ci) => Some(render_fragment(
            &toks[ci + 1..ann_end],
            src,
            star_annotation,
        )?),
        None => None,
    };
    let default = match eq {
        Some(ei) => Some(render_fragment(&toks[ei + 1..], src, false)?),
        None => None,
    };
    Ok(RawParam {
        name,
        annotation,
        default,
    })
}

fn render_fragment(
    toks: &[Tok],
    src: &str,
    star_annotation: bool,
) -> Result<String, SigParseError> {
    if toks.is_empty() {
        return Err(invalid_syntax());
    }
    let fragment = &src[toks[0].start..toks[toks.len() - 1].end];
    pycode_unparse(fragment, star_annotation)
}

// ---------------------------------------------------------------------------
// sphinx.pycode.ast.unparse over the task-3 AST (see module docs)
// ---------------------------------------------------------------------------

enum RenderErr {
    /// The numeric-token pool did not line up with the tree; re-render in
    /// `repr` fallback mode.
    SourceMismatch,
    /// A shape sphinx's `_UnparseVisitor` cannot render either (`{**a}`
    /// dies in its strict zip).
    Unrenderable,
}

struct NumPool {
    texts: Vec<String>,
    idx: usize,
}

/// Parse one source fragment with task 3 and render it with
/// `sphinx.pycode.ast.unparse` semantics, recovering numeric source text
/// by order-mapping the fragment's number tokens onto the tree's numeric
/// constants (each verified by re-parsing; any mismatch falls back to
/// `repr` form for the whole fragment, mirroring `get_source_segment`'s
/// `or repr(...)` arm).
fn pycode_unparse(fragment: &str, star_annotation: bool) -> Result<String, SigParseError> {
    let parsed = if star_annotation {
        parse_py_star_annotation(fragment)
    } else {
        parse_py_expr(fragment)
    }
    .map_err(|_| invalid_syntax())?;
    let texts: Vec<String> = lex(fragment)
        .map(|toks| {
            toks.into_iter()
                .filter(|t| t.kind == TokKind::Number)
                .map(|t| t.text)
                .collect()
        })
        .unwrap_or_default();
    let mut pool = Some(NumPool { texts, idx: 0 });
    match render_pycode(&parsed, &mut pool) {
        Ok(s) if pool.as_ref().is_some_and(|p| p.idx == p.texts.len()) => Ok(s),
        Ok(_) | Err(RenderErr::SourceMismatch) => {
            let mut no_pool = None;
            render_pycode(&parsed, &mut no_pool).map_err(|_| invalid_syntax())
        }
        Err(RenderErr::Unrenderable) => Err(invalid_syntax()),
    }
}

fn binop_text(op: PyOp) -> &'static str {
    match op {
        PyOp::Add => "+",
        PyOp::Sub => "-",
        PyOp::Mult => "*",
        PyOp::MatMult => "@",
        PyOp::Div => "/",
        PyOp::Mod => "%",
        PyOp::Pow => "**",
        PyOp::LShift => "<<",
        PyOp::RShift => ">>",
        PyOp::BitOr => "|",
        PyOp::BitXor => "^",
        PyOp::BitAnd => "&",
        PyOp::FloorDiv => "//",
    }
}

fn render_join(elts: &[PyExpr], pool: &mut Option<NumPool>) -> Result<String, RenderErr> {
    let mut parts = Vec::with_capacity(elts.len());
    for e in elts {
        parts.push(render_pycode(e, pool)?);
    }
    Ok(parts.join(", "))
}

fn render_pycode(e: &PyExpr, pool: &mut Option<NumPool>) -> Result<String, RenderErr> {
    Ok(match e {
        PyExpr::Name(id) => id.clone(),
        PyExpr::Attribute(value, attr) => format!("{}.{attr}", render_pycode(value, pool)?),
        PyExpr::BinOp { left, op, right } => {
            let l = render_pycode(left, pool)?;
            let r = render_pycode(right, pool)?;
            let o = binop_text(*op);
            // "Special case ``**`` to not have surrounding spaces."
            if matches!(op, PyOp::Pow) {
                format!("{l}{o}{r}")
            } else {
                format!("{l} {o} {r}")
            }
        }
        PyExpr::UnaryOp { op, operand } => {
            let s = render_pycode(operand, pool)?;
            match op {
                PyUnaryOp::Not => format!("not {s}"),
                PyUnaryOp::Invert => format!("~{s}"),
                PyUnaryOp::UAdd => format!("+{s}"),
                PyUnaryOp::USub => format!("-{s}"),
            }
        }
        PyExpr::Constant(c) => const_text(c, pool)?,
        PyExpr::Tuple(elts) => match elts.len() {
            0 => "()".to_string(),
            1 => format!("({},)", render_pycode(&elts[0], pool)?),
            _ => format!("({})", render_join(elts, pool)?),
        },
        PyExpr::List(elts) => format!("[{}]", render_join(elts, pool)?),
        PyExpr::Set(elts) => format!("{{{}}}", render_join(elts, pool)?),
        PyExpr::Dict(entries) => {
            let mut parts = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                // `visit_Dict` skips None keys in its keys generator and
                // then zips strict → ValueError in sphinx.
                let Some(key) = key else {
                    return Err(RenderErr::Unrenderable);
                };
                parts.push(format!(
                    "{}: {}",
                    render_pycode(key, pool)?,
                    render_pycode(value, pool)?
                ));
            }
            format!("{{{}}}", parts.join(", "))
        }
        PyExpr::Call { func, args, kwargs } => {
            // The callee is rendered FIRST: it precedes the arguments in
            // the source, and `NumPool` hands out the fragment's number
            // tokens in render order. Rendering it after the arguments
            // (as `visit_Call`'s f-string reads, but sphinx recovers
            // numeric text by AST position instead) mis-pairs every
            // literal in `a(0x10).b(16)`.
            let f = render_pycode(func, pool)?;
            let mut parts: Vec<String> = Vec::with_capacity(args.len() + kwargs.len());
            for a in args {
                parts.push(render_pycode(a, pool)?);
            }
            for (k, v) in kwargs {
                parts.push(format!("{k}={}", render_pycode(v, pool)?));
            }
            format!("{f}({})", parts.join(", "))
        }
        // `visit_BoolOp`: `' and '` / `' or '` joined, never parenthesized
        // (sphinx's unparser has no precedence table, so `(a or b) and c`
        // renders as `a or b and c`).
        PyExpr::BoolOp { op, values } => {
            let sep = match op {
                PyBoolOp::And => " and ",
                PyBoolOp::Or => " or ",
            };
            let mut parts = Vec::with_capacity(values.len());
            for v in values {
                parts.push(render_pycode(v, pool)?);
            }
            parts.join(sep)
        }
        PyExpr::Starred(value) => format!("*{}", render_pycode(value, pool)?),
        PyExpr::Subscript { value, slice } => {
            let v = render_pycode(value, pool)?;
            match &**slice {
                // `is_simple_tuple`: non-empty, no Starred elements.
                PyExpr::Tuple(elts)
                    if !elts.is_empty()
                        && !elts.iter().any(|e| matches!(e, PyExpr::Starred(_))) =>
                {
                    format!("{v}[{}]", render_join(elts, pool)?)
                }
                other => format!("{v}[{}]", render_pycode(other, pool)?),
            }
        }
    })
}

/// `visit_Constant`: source segment for int/float (verified pool token),
/// `...` for Ellipsis, `repr(value)` otherwise — which drops a `u` string
/// prefix (`repr` never knew about it).
fn const_text(c: &PyConst, pool: &mut Option<NumPool>) -> Result<String, RenderErr> {
    match c {
        PyConst::Ellipsis => Ok("...".to_string()),
        PyConst::Int(_) | PyConst::Float(_) => {
            if let Some(p) = pool.as_mut() {
                let tok = p.texts.get(p.idx).cloned();
                p.idx += 1;
                if let Some(tok) = tok {
                    if matches!(parse_py_expr(&tok), Ok(PyExpr::Constant(parsed)) if parsed == *c) {
                        return Ok(tok);
                    }
                }
                Err(RenderErr::SourceMismatch)
            } else {
                Ok(expr::unparse(&PyExpr::Constant(c.clone())))
            }
        }
        PyConst::Str {
            value,
            quote,
            u_prefix: _,
        } => Ok(expr::unparse(&PyExpr::Constant(PyConst::Str {
            value: value.clone(),
            quote: *quote,
            u_prefix: false,
        }))),
        other => Ok(expr::unparse(&PyExpr::Constant(other.clone()))),
    }
}

// ---------------------------------------------------------------------------
// Type-parameter-list parser (`_TypeParameterListParser`, [PY §2.4])
// ---------------------------------------------------------------------------

struct TpParam {
    name: String,
    kind: ParamKind,
    annotation: Option<String>,
    default: Option<String>,
}

/// `TokenProcessor` mirror: `fetch_token` advances `current`/`previous`
/// exactly like `sphinx/pycode/parser.py` (on exhaustion `previous` still
/// shifts and `current` becomes `None`).
struct TpCursor<'t> {
    toks: &'t [Tok],
    next: usize,
    current: Option<usize>,
    previous: Option<usize>,
}

impl<'t> TpCursor<'t> {
    fn new(toks: &'t [Tok]) -> Self {
        Self {
            toks,
            next: 0,
            current: None,
            previous: None,
        }
    }

    fn fetch(&mut self) -> Option<usize> {
        self.previous = self.current;
        if self.next < self.toks.len() {
            self.current = Some(self.next);
            self.next += 1;
        } else {
            self.current = None;
        }
        self.current
    }

    fn tok(&self, idx: usize) -> &'t Tok {
        &self.toks[idx]
    }

    fn is_op(&self, idx: Option<usize>, text: &str) -> bool {
        idx.is_some_and(|i| self.toks[i].kind == TokKind::Op && self.toks[i].text == text)
    }

    /// `fetch_until(rdelim)`, iterative (the sphinx original recurses per
    /// nesting level; an explicit closer stack keeps totality on
    /// adversarial nesting). Mismatched closers are collected and ignored,
    /// exactly like the original; exhaustion returns what was collected.
    fn fetch_until_into(&mut self, rdelim: &'static str, out: &mut Vec<usize>) {
        let mut expected: Vec<&'static str> = vec![rdelim];
        while let Some(i) = self.fetch() {
            out.push(i);
            let t = self.tok(i);
            if t.kind == TokKind::Op {
                if expected.last().copied() == Some(t.text.as_str()) {
                    expected.pop();
                    if expected.is_empty() {
                        return;
                    }
                    continue;
                }
                match t.text.as_str() {
                    "(" => expected.push(")"),
                    "{" => expected.push("}"),
                    "[" => expected.push("]"),
                    _ => {}
                }
            }
        }
    }

    /// `fetch_type_param_spec`: collect until a top-level `:`, `=` or `,`
    /// (the terminator is consumed but not returned), balancing brackets.
    fn fetch_type_param_spec(&mut self) -> Vec<usize> {
        let mut tokens: Vec<usize> = Vec::new();
        while let Some(i) = self.fetch() {
            tokens.push(i);
            let t = self.tok(i);
            let mut handled = false;
            if t.kind == TokKind::Op {
                match t.text.as_str() {
                    "(" => {
                        self.fetch_until_into(")", &mut tokens);
                        handled = true;
                    }
                    "{" => {
                        self.fetch_until_into("}", &mut tokens);
                        handled = true;
                    }
                    "[" => {
                        self.fetch_until_into("]", &mut tokens);
                        handled = true;
                    }
                    _ => {}
                }
            }
            if !handled && t.kind == TokKind::Op && matches!(t.text.as_str(), ":" | "=" | ",") {
                tokens.pop();
                break;
            }
        }
        tokens
    }
}

/// `_TypeParameterListParser.parse`: only NAME tokens start a parameter;
/// a `*`/`**` previous token selects the variadic kind; `:` fetches a
/// bound/constraint spec, a following `=` fetches a default. Non-NAME
/// junk at the top level is silently skipped (as in sphinx). A bound on a
/// variadic parameter raises — message verbatim (`_annotations.py:308-315`).
fn tp_parse(toks: &[Tok]) -> Result<Vec<TpParam>, SigParseError> {
    let mut cur = TpCursor::new(toks);
    let mut out: Vec<TpParam> = Vec::new();
    while let Some(i) = cur.fetch() {
        if cur.tok(i).kind != TokKind::Name {
            continue;
        }
        let name = cur.tok(i).text.clone();
        let kind = if cur.is_op(cur.previous, "*") {
            ParamKind::VarPositional
        } else if cur.is_op(cur.previous, "**") {
            ParamKind::VarKeyword
        } else {
            ParamKind::PositionalOrKeyword
        };

        let mut annotation: Option<String> = None;
        let mut default: Option<String> = None;
        let next = cur.fetch();
        if cur.is_op(next, ":") || cur.is_op(next, "=") {
            if cur.is_op(next, ":") {
                let spec = cur.fetch_type_param_spec();
                annotation = Some(build_identifier(&spec, &cur));
            }
            if cur.is_op(cur.current, "=") {
                let spec = cur.fetch_type_param_spec();
                default = Some(build_identifier(&spec, &cur));
            }
        }

        if kind != ParamKind::PositionalOrKeyword && annotation.is_some() {
            let desc = match kind {
                ParamKind::VarPositional => "variadic positional",
                ParamKind::VarKeyword => "variadic keyword",
                _ => unreachable!("guarded above"),
            };
            return Err(SigParseError::Syntax(format!(
                "type parameter bound or constraint is not allowed for {desc} parameters"
            )));
        }
        out.push(TpParam {
            name,
            kind,
            annotation,
            default,
        });
    }
    Ok(out)
}

fn is_operand_left(t: &Tok) -> bool {
    matches!(t.kind, TokKind::Name | TokKind::Number | TokKind::Str)
        || (t.kind == TokKind::Op && matches!(t.text.as_str(), ")" | "]" | "}"))
}

fn is_operand_right(t: Option<&Tok>) -> bool {
    t.is_some_and(|t| {
        matches!(t.kind, TokKind::Name | TokKind::Number | TokKind::Str)
            || (t.kind == TokKind::Op && matches!(t.text.as_str(), "(" | "[" | "{"))
    })
}

/// `_TypeParameterListParser._build_identifier`: bound/default text is
/// reassembled from raw tokens with spacing rules — `:`/`,` get a trailing
/// space, binary-ish operators get surrounding spaces, an unpack `*`/`**`
/// (operator between a non-operand and an operand) stays flush. The
/// first-token unpack check matches only `*` (sphinx compares against a
/// nested list for `**` — a bug mirrored deliberately).
fn build_identifier(spec: &[usize], cur: &TpCursor<'_>) -> String {
    let toks: Vec<&Tok> = spec.iter().map(|&i| cur.tok(i)).collect();
    let mut idents: Vec<String> = Vec::new();
    let mut pos = 0usize;
    while pos < toks.len()
        && toks[pos].kind == TokKind::Op
        && matches!(toks[pos].text.as_str(), "(" | "[" | "{")
    {
        idents.push(toks[pos].text.clone());
        pos += 1;
    }
    if pos < toks.len() {
        let first = toks[pos];
        let is_unpack = first.kind == TokKind::Op && first.text == "*";
        idents.push(tp_pformat_token(first, is_unpack));
        pos += 1;
    }
    let rest = &toks[pos..];
    let mut is_unpack = false;
    for (j, tok) in rest.iter().enumerate() {
        idents.push(tp_pformat_token(tok, is_unpack));
        let op = rest.get(j + 1);
        let after = rest.get(j + 2).copied();
        is_unpack = op
            .is_some_and(|o| o.kind == TokKind::Op && matches!(o.text.as_str(), "*" | "**"))
            && !(is_operand_left(tok) && is_operand_right(after));
    }
    idents.concat().trim().to_string()
}

/// `_TypeParameterListParser._pformat_token`.
fn tp_pformat_token(tok: &Tok, native: bool) -> String {
    if native {
        return tok.text.clone();
    }
    if tok.kind == TokKind::Op {
        if matches!(tok.text.as_str(), ":" | "," | "#") {
            return format!("{} ", tok.text);
        }
        if matches!(
            tok.text.as_str(),
            "=" | "|"
                | "&"
                | "^"
                | "<"
                | ">"
                | "+"
                | "-"
                | "*"
                | "**"
                | "@"
                | "/"
                | "//"
                | "%"
                | "<<"
                | ">>"
                | ">>>"
                | "<="
                | ">="
                | "=="
                | "!="
        ) {
            return format!(" {} ", tok.text);
        }
    }
    tok.text.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dctx() -> PyRefContext {
        PyRefContext::default()
    }

    fn dcfg() -> PySigConfig {
        PySigConfig::default()
    }

    fn parsed(arglist: &str) -> String {
        parse_arglist(arglist, false, &dctx(), &dcfg())
            .expect("arglist parses")
            .pformat()
    }

    fn pseudo(arglist: &str) -> String {
        pseudo_parse_arglist(arglist, false, &dctx(), &dcfg()).pformat()
    }

    fn tp(tp_list: &str) -> String {
        parse_type_list(tp_list, false, &dctx(), &dcfg())
            .expect("tp list parses")
            .pformat()
    }

    fn kinds_of(arglist: &str) -> Vec<ParamKind> {
        signature_from_str(arglist)
            .expect("arglist parses")
            .into_iter()
            .map(|p| p.kind)
            .collect()
    }

    fn sig_match(tp_span: (usize, usize), arg_span: (usize, usize)) -> PySigMatch {
        PySigMatch {
            prefix: None,
            name: String::new(),
            tp_list: None,
            arglist: None,
            retann: None,
            tp_span,
            arg_span,
        }
    }

    fn cfg_max(global: i64) -> PySigConfig {
        PySigConfig {
            maximum_signature_line_length: Some(global),
            ..PySigConfig::default()
        }
    }

    /// `multi_line_flags` under a global max_len and no directive options.
    fn flags(
        sig: &str,
        tp_span: (usize, usize),
        arg_span: (usize, usize),
        max: i64,
    ) -> (bool, bool) {
        multi_line_flags(
            sig,
            &sig_match(tp_span, arg_span),
            SingleLineOpts::default(),
            &cfg_max(max),
        )
    }

    /// The default attr head every parsed/pseudo list carries ([SIG §1.4]).
    const PL_HEAD: &str = "<desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n";
    const TPL_HEAD: &str = "<desc_type_parameter_list multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n";

    // -- signature_from_str: grammar ---------------------------------------

    /// `def func(): pass` and whitespace-only arglists have no parameters.
    #[test]
    fn sig_empty_arglist_is_no_params() {
        assert_eq!(signature_from_str(""), Ok(Vec::new()));
        assert_eq!(signature_from_str("   "), Ok(Vec::new()));
    }

    /// Plain names classify POSITIONAL_OR_KEYWORD
    /// (`util/inspect.py:997-1001`).
    #[test]
    fn sig_plain_params_are_positional_or_keyword() {
        assert_eq!(
            signature_from_str("a, b").unwrap(),
            vec![
                PyParam {
                    name: "a".to_string(),
                    kind: ParamKind::PositionalOrKeyword,
                    annotation: None,
                    default: None,
                },
                PyParam {
                    name: "b".to_string(),
                    kind: ParamKind::PositionalOrKeyword,
                    annotation: None,
                    default: None,
                },
            ]
        );
    }

    /// The full marker set of probe §1.6 function_full_markers: default,
    /// `*args`, annotated keyword-only default, `**kwargs`.
    #[test]
    fn sig_full_marker_classification() {
        assert_eq!(
            signature_from_str("a, b=1, *args, c: int = 2, **kwargs").unwrap(),
            vec![
                PyParam {
                    name: "a".to_string(),
                    kind: ParamKind::PositionalOrKeyword,
                    annotation: None,
                    default: None,
                },
                PyParam {
                    name: "b".to_string(),
                    kind: ParamKind::PositionalOrKeyword,
                    annotation: None,
                    default: Some("1".to_string()),
                },
                PyParam {
                    name: "args".to_string(),
                    kind: ParamKind::VarPositional,
                    annotation: None,
                    default: None,
                },
                PyParam {
                    name: "c".to_string(),
                    kind: ParamKind::KeywordOnly,
                    annotation: Some("int".to_string()),
                    default: Some("2".to_string()),
                },
                PyParam {
                    name: "kwargs".to_string(),
                    kind: ParamKind::VarKeyword,
                    annotation: None,
                    default: None,
                },
            ]
        );
    }

    /// `/` reclassifies everything before it as positional-only; a bare
    /// `*` opens the keyword-only zone (probe §1.6 function_posonly).
    #[test]
    fn sig_slash_and_star_zones() {
        assert_eq!(
            kinds_of("a, /, b, *, c"),
            vec![
                ParamKind::PositionalOnly,
                ParamKind::PositionalOrKeyword,
                ParamKind::KeywordOnly,
            ]
        );
        assert_eq!(kinds_of("a, /"), vec![ParamKind::PositionalOnly]);
        assert_eq!(
            kinds_of("a, *args, b"),
            vec![
                ParamKind::PositionalOrKeyword,
                ParamKind::VarPositional,
                ParamKind::KeywordOnly,
            ]
        );
        assert_eq!(kinds_of("*, a"), vec![ParamKind::KeywordOnly]);
    }

    /// Duplicate names are `inspect.Signature`'s ValueError — the warning
    /// channel, distinct from Syntax [PY §1.3 step 5]. `ast.parse` accepts
    /// them across every parameter kind (probed: `def f(a, /, a)` and
    /// `def f(*a, **a)` both parse).
    #[test]
    fn sig_duplicate_names_are_duplicate_errors() {
        for arglist in ["a, a", "a, /, a", "a, *a", "a, **a", "a, *, a"] {
            assert_eq!(
                signature_from_str(arglist),
                Err(SigParseError::Duplicate("a".to_string())),
                "arglist {arglist:?}"
            );
        }
        // First duplicate in sequence order wins.
        assert_eq!(
            signature_from_str("x, y, y, x"),
            Err(SigParseError::Duplicate("y".to_string()))
        );
    }

    /// CPython-parser grammar violations (each probed against ast.parse in
    /// the task-5 report) are the silent Syntax channel.
    #[test]
    fn sig_grammar_violations_are_syntax_errors() {
        for arglist in [
            "/",
            "/, a",
            "a, /, b, /",
            "*",
            "a, *,",
            "*, **kw",
            "**kw, a",
            "*a, *b",
            "*args, /",
            ",",
            "a, , b",
            "*args=1",
            "**kw=2",
            "a=1, b",
            "a=1, /, b",
            "a b",
            "if",
            "None",
            "x.y",
            "x()",
            "42",
        ] {
            assert!(
                matches!(signature_from_str(arglist), Err(SigParseError::Syntax(_))),
                "arglist {arglist:?}"
            );
        }
    }

    /// Keyword-only parameters are exempt from the non-default-after-
    /// default rule (probed: `def f(a=1, *, b)` is valid, and so is a
    /// keyword-only gap like `def f(*, a=1, b)`).
    #[test]
    fn sig_keyword_only_zone_allows_default_gaps() {
        assert!(signature_from_str("a=1, *, b").is_ok());
        assert!(signature_from_str("*, a=1, b").is_ok());
        assert!(signature_from_str("a=1, *args, b").is_ok());
        assert!(signature_from_str("a=1, /, b=2").is_ok());
    }

    /// Trailing commas are fine everywhere ast allows them.
    #[test]
    fn sig_trailing_comma_allowed() {
        assert_eq!(kinds_of("a,").len(), 1);
        assert_eq!(kinds_of("**kw, ").len(), 1);
        assert_eq!(kinds_of("*args,").len(), 1);
    }

    // -- signature_from_str: pycode-unparse normalization ------------------

    /// Defaults keep numeric SOURCE text (`get_source_segment`) but
    /// normalize container spacing — probe task5/default_normalized:
    /// `f(x=0xFF, y=[1,2])` renders `0xFF` and `[1, 2]`.
    #[test]
    fn sig_defaults_keep_numeric_source_text() {
        let params = signature_from_str("x=0xFF, y=[1,2], z=1_000, w=1e5").unwrap();
        let defaults: Vec<&str> = params
            .iter()
            .map(|p| p.default.as_deref().unwrap())
            .collect();
        assert_eq!(defaults, vec!["0xFF", "[1, 2]", "1_000", "1e5"]);
    }

    /// `sphinx.pycode.ast._UnparseVisitor` differences from `ast.unparse`
    /// (module docs): no precedence parens, unspaced `**`, `u''` prefix
    /// dropped, string/bytes/bool via repr.
    #[test]
    fn sig_defaults_use_pycode_unparse_rules() {
        let params = signature_from_str(
            "a=(x+y)*z, b=x**y, c=u'v', d='s', e=-1, f=~x, g=(1,), h={1: 2}, i={3}, j=f2(4, k=5), k=x[1:2:3]",
        );
        // `x[1:2:3]` is a slice — outside the task-3 subset → Syntax.
        assert!(matches!(params, Err(SigParseError::Syntax(_))));
        let params = signature_from_str(
            "a=(x+y)*z, b=x**y, c=u'v', d='s', e=-1, f=~x, g=(1,), h={1: 2}, i={3}, j=f2(4, k=5)",
        )
        .unwrap();
        let defaults: Vec<&str> = params
            .iter()
            .map(|p| p.default.as_deref().unwrap())
            .collect();
        assert_eq!(
            defaults,
            vec![
                "x + y * z",
                "x**y",
                "'v'",
                "'s'",
                "-1",
                "~x",
                "(1,)",
                "{1: 2}",
                "{3}",
                "f2(4, k=5)",
            ]
        );
    }

    /// Annotations round through the same normalizer before task 4
    /// re-parses them (`_annotations.py:495`).
    #[test]
    fn sig_annotations_are_pycode_normalized() {
        let params = signature_from_str("x: dict[str,int], y: 'T'").unwrap();
        assert_eq!(params[0].annotation.as_deref(), Some("dict[str, int]"));
        assert_eq!(params[1].annotation.as_deref(), Some("'T'"));
    }

    /// Commas inside string literals do not split parameters (probe
    /// task5/string_comma_default).
    #[test]
    fn sig_string_protected_comma() {
        let params = signature_from_str("x='a,b'").unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].default.as_deref(), Some("'a,b'"));
    }

    /// A depth-0 `lambda` is rejected up front (module docs: its parameter
    /// commas would defeat the comma split; sphinx renders
    /// `lambda a, b: ...` — documented divergence).
    #[test]
    fn sig_top_level_lambda_is_syntax() {
        assert!(matches!(
            signature_from_str("x=lambda a, b: 0"),
            Err(SigParseError::Syntax(_))
        ));
    }

    // -- multi_line_flags: the [SIG A.1] matrix ----------------------------

    /// Probe family A: what counts toward the length. `foo(aaaa)` len 9,
    /// arg inner span (4,8); strictly greater, so equality never flips.
    #[test]
    fn matrix_a_measurement() {
        // A1: len 9, max 9 → no flip.
        assert_eq!(flags("foo(aaaa)", (0, 0), (4, 8), 9), (false, false));
        // A2: len 9, max 8 → arglist flips.
        assert_eq!(flags("foo(aaaa)", (0, 0), (4, 8), 8), (true, false));
        // A3: 'foo(a) -> int' len 13 — the return annotation counts.
        assert_eq!(flags("foo(a) -> int", (0, 0), (4, 5), 12), (true, false));
        // A4: equal again → no flip.
        assert_eq!(flags("foo(a) -> int", (0, 0), (4, 5), 13), (false, false));
        // A5: 'Klass.foo(a)' len 12 — the dotted prefix counts.
        assert_eq!(flags("Klass.foo(a)", (0, 0), (10, 11), 11), (true, false));
        // A6: measurement happens on the STRIPPED signature (get_signatures
        // strips before py_sig_re runs) — same result as A1 by contract.
        assert_eq!(flags("foo(aaaa)", (0, 0), (4, 8), 9), (false, false));
    }

    /// Probe family B: `foo[T](aaaa)` len 12, tp inner 'T' (4,5), arg
    /// inner 'aaaa' (7,11). Only the OTHER group's inner text is
    /// subtracted; the bracket characters themselves still count.
    #[test]
    fn matrix_b_span_subtraction() {
        // B1: arglist 12-1=11 > 10 flips; tp 12-4=8 > 10 doesn't.
        assert_eq!(flags("foo[T](aaaa)", (4, 5), (7, 11), 10), (true, false));
        // B2: both flip (11>7, 8>7).
        assert_eq!(flags("foo[T](aaaa)", (4, 5), (7, 11), 7), (true, true));
        // B3: neither (11>11 false, 8>11 false).
        assert_eq!(flags("foo[T](aaaa)", (4, 5), (7, 11), 11), (false, false));
    }

    /// Probe family C: config precedence through [`PySigConfig::max_len`],
    /// including the falsy-zero trap (C4) and the `> max_len > 0` guard
    /// making the resolved 0 mean "off" (C5).
    #[test]
    fn matrix_c_config_precedence() {
        let m = sig_match((0, 0), (4, 8));
        let opts = SingleLineOpts::default();
        let run = |python: Option<i64>, global: Option<i64>| {
            let cfg = PySigConfig {
                python_maximum_signature_line_length: python,
                maximum_signature_line_length: global,
                ..PySigConfig::default()
            };
            multi_line_flags("foo(aaaa)", &m, opts, &cfg).0
        };
        assert!(!run(Some(1000), Some(1))); // C1: python wins, no flip
        assert!(run(Some(1), Some(1000))); // C2: python wins, flip
        assert!(run(None, Some(1))); // C3: fallback to global
        assert!(run(Some(0), Some(1))); // C4: falsy 0 falls through
        assert!(!run(None, None)); // C5: resolved 0 → feature off
    }

    /// Probes D1/D2: each single-line-* option suppresses ONLY its own
    /// list.
    #[test]
    fn matrix_d_single_line_options_are_independent() {
        let m = sig_match((4, 5), (7, 11));
        let cfg = cfg_max(1);
        assert_eq!(
            multi_line_flags(
                "foo[T](aaaa)",
                &m,
                SingleLineOpts {
                    parameter_list: true,
                    type_parameter_list: false,
                },
                &cfg
            ),
            (false, true)
        );
        assert_eq!(
            multi_line_flags(
                "foo[T](aaaa)",
                &m,
                SingleLineOpts {
                    parameter_list: false,
                    type_parameter_list: true,
                },
                &cfg
            ),
            (true, false)
        );
    }

    // -- parse_arglist: node shapes ([PY §2.1], probe-verbatim) ------------

    /// Probe §1.6 function_plain_args: one `desc_parameter >
    /// desc_sig_name` per plain parameter; attrs unconditional.
    #[test]
    fn arglist_plain_params() {
        assert_eq!(
            parsed("a, b"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            a\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            b\n",
                )
            ]
            .concat()
        );
    }

    /// Probe §1.6 function_full_markers: unannotated default is a bare
    /// `=` operator + `default_value` inline; annotated default wraps the
    /// `=` in spaces; `*args`/`**kwargs` get a leading operator.
    #[test]
    fn arglist_full_markers() {
        assert_eq!(
            parsed("a, b=1, *args, c: int = 2, **kwargs"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            a\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            b\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            =\n",
                    "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                    "            1\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            *\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            args\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            c\n",
                    "        <desc_sig_punctuation classes=\"p\">\n",
                    "            :\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                    "                int\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            =\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                    "            2\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            **\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            kwargs\n",
                )
            ]
            .concat()
        );
    }

    /// Probe §1.6 function_posonly: the separators are `desc_parameter >
    /// desc_sig_operator(classes [<sep>, "o"]) > abbreviation` with the
    /// exact PEP explanations.
    #[test]
    fn arglist_separator_shapes() {
        assert_eq!(
            parsed("a, /, b, *, c"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            a\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_operator classes=\"positional-only-separator o\">\n",
                    "            <abbreviation explanation=\"Positional-only parameter separator (PEP 570)\">\n",
                    "                /\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            b\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_operator classes=\"keyword-only-separator o\">\n",
                    "            <abbreviation explanation=\"Keyword-only parameters separator (PEP 3102)\">\n",
                    "                *\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            c\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/posonly_trailing: `func(a, /)` — the loop epilogue
    /// (`_annotations.py:513-514`) emits the trailing `/` separator.
    #[test]
    fn arglist_trailing_slash_epilogue() {
        assert_eq!(
            parsed("a, /"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            a\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_operator classes=\"positional-only-separator o\">\n",
                    "            <abbreviation explanation=\"Positional-only parameter separator (PEP 570)\">\n",
                    "                /\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/posonly_then_kwonly: `f(a, /, *, b)` emits BOTH
    /// separators back to back (the `/` from the kind transition, the `*`
    /// because last_kind was positional-only).
    #[test]
    fn arglist_adjacent_separators() {
        assert_eq!(
            parsed("a, /, *, b"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            a\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_operator classes=\"positional-only-separator o\">\n",
                    "            <abbreviation explanation=\"Positional-only parameter separator (PEP 570)\">\n",
                    "                /\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_operator classes=\"keyword-only-separator o\">\n",
                    "            <abbreviation explanation=\"Keyword-only parameters separator (PEP 3102)\">\n",
                    "                *\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            b\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/kwonly_first: `f(*, a)` — the keyword-only separator
    /// also fires from last_kind None.
    #[test]
    fn arglist_kwonly_separator_first() {
        assert_eq!(
            parsed("*, a"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_operator classes=\"keyword-only-separator o\">\n",
                    "            <abbreviation explanation=\"Keyword-only parameters separator (PEP 3102)\">\n",
                    "                *\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            a\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/starargs_then_kwonly: after `*args` NO `*` separator is
    /// inserted before keyword-only parameters.
    #[test]
    fn arglist_no_separator_after_varargs() {
        assert_eq!(
            parsed("a, *args, b"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            a\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            *\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            args\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            b\n",
                )
            ]
            .concat()
        );
    }

    /// Probe §1.6 function_default_str + probe task5/default_normalized:
    /// defaults keep their source-ish form through pycode-unparse.
    #[test]
    fn arglist_default_value_text() {
        assert_eq!(
            parsed("name='x', items=[]"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            name\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            =\n",
                    "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                    "            'x'\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            items\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            =\n",
                    "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                    "            []\n",
                )
            ]
            .concat()
        );
        assert_eq!(
            parsed("x=0xFF, y=[1,2]"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            x\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            =\n",
                    "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                    "            0xFF\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            y\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            =\n",
                    "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                    "            [1, 2]\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/annotated_default_parsed: `f(x: int = 2)` space-wraps
    /// the `=` because the parameter is annotated.
    #[test]
    fn arglist_annotated_default_is_space_wrapped() {
        assert_eq!(
            parsed("x: int = 2"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            x\n",
                    "        <desc_sig_punctuation classes=\"p\">\n",
                    "            :\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                    "                int\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            =\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                    "            2\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/string_comma_default: `f(x='a,b')` parses as ONE
    /// parameter through the real grammar.
    #[test]
    fn arglist_string_comma_default() {
        assert_eq!(
            parsed("x='a,b'"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            x\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            =\n",
                    "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                    "            'a,b'\n",
                )
            ]
            .concat()
        );
    }

    /// Contract: parse_arglist ALWAYS sets both attrs, an empty arglist
    /// included. NOTE probe task5/empty_args: sphinx's `f()` renders a
    /// BARE `<desc_parameterlist xml:space="preserve">` with NEITHER attr,
    /// because py_sig_re's group 4 is `''` (falsy) and `handle_signature`
    /// takes the `needs_arglist()` branch (`_object.py:382-385`) — task 6
    /// must route empty parens there, never through parse_arglist.
    #[test]
    fn arglist_empty_still_carries_attrs() {
        assert_eq!(parsed(""), PL_HEAD);
        assert_eq!(parsed("  "), PL_HEAD);
    }

    /// [SIG §1.4] probes E1/E2/E3: multi_line mirrors the measured flag,
    /// multi_line_trailing_comma mirrors
    /// `python_trailing_comma_in_multi_line_signatures`, and both are
    /// recorded even when nothing wraps.
    #[test]
    fn arglist_attr_values_follow_inputs() {
        let flipped = parse_arglist("aaaa", true, &dctx(), &dcfg())
            .unwrap()
            .pformat();
        assert!(flipped.starts_with(
            "<desc_parameterlist multi_line_parameter_list=\"1\" multi_line_trailing_comma=\"1\""
        ));
        let no_comma_cfg = PySigConfig {
            python_trailing_comma_in_multi_line_signatures: false,
            ..PySigConfig::default()
        };
        let no_comma = parse_arglist("aaaa", true, &dctx(), &no_comma_cfg)
            .unwrap()
            .pformat();
        assert!(no_comma.starts_with(
            "<desc_parameterlist multi_line_parameter_list=\"1\" multi_line_trailing_comma=\"0\""
        ));
    }

    /// parse_arglist propagates both error channels for task 6's
    /// warning/debug split.
    #[test]
    fn arglist_error_channels() {
        assert_eq!(
            parse_arglist("a, a", false, &dctx(), &dcfg()),
            Err(SigParseError::Duplicate("a".to_string()))
        );
        assert!(matches!(
            parse_arglist("a[, b]", false, &dctx(), &dcfg()),
            Err(SigParseError::Syntax(_))
        ));
    }

    /// Widths come from the spanned slice; degenerate or out-of-range
    /// spans count 0 and never panic (totality).
    #[test]
    fn matrix_span_edge_cases() {
        assert_eq!(flags("foo(aaaa)", (5, 2), (4, 8), 8), (true, false));
        assert_eq!(flags("foo(aaaa)", (3, 999), (4, 8), 8), (true, false));
        // Char counting: 'fóó(aaaa)' is 9 Python chars (11 bytes).
        assert_eq!(
            flags("f\u{f3}\u{f3}(aaaa)", (0, 0), (6, 10), 9),
            (false, false)
        );
        assert_eq!(
            flags("f\u{f3}\u{f3}(aaaa)", (0, 0), (6, 10), 8),
            (true, false)
        );
    }

    // -- pseudo_parse_arglist ([PY §2.2], probe-verbatim) ------------------

    /// Probe §1.6 function_brackets_fallback: `func(a[, b])` — brackets
    /// push/pop `desc_optional`; the pseudo list carries both attrs.
    #[test]
    fn pseudo_brackets_become_optional() {
        assert_eq!(
            pseudo("a[, b]"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            a\n",
                    "    <desc_optional xml:space=\"preserve\">\n",
                    "        <desc_parameter xml:space=\"preserve\">\n",
                    "            <desc_sig_name classes=\"n\">\n",
                    "                b\n",
                )
            ]
            .concat()
        );
    }

    /// Nested optionals stack (`_annotations.py:564-576`, `602-607`).
    #[test]
    fn pseudo_nested_optionals() {
        assert_eq!(
            pseudo("a[, b[, c]]"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            a\n",
                    "    <desc_optional xml:space=\"preserve\">\n",
                    "        <desc_parameter xml:space=\"preserve\">\n",
                    "            <desc_sig_name classes=\"n\">\n",
                    "                b\n",
                    "        <desc_optional xml:space=\"preserve\">\n",
                    "            <desc_parameter xml:space=\"preserve\">\n",
                    "                <desc_sig_name classes=\"n\">\n",
                    "                    c\n",
                )
            ]
            .concat()
        );
    }

    /// Unannotated pseudo default: `=` is a BARE desc_sig_operator, no
    /// spaces (`_annotations.py:592-599`).
    #[test]
    fn pseudo_bare_equals_when_unannotated() {
        assert_eq!(
            pseudo("n=1"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            n\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            =\n",
                    "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                    "            1\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/pseudo_annotated_default: `func(x: int=2[, y])` — the
    /// pseudo parser space-wraps `=` only when annotated and renders the
    /// annotation through `_parse_annotation`.
    #[test]
    fn pseudo_annotated_default() {
        assert_eq!(
            pseudo("x: int=2[, y]"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            x\n",
                    "        <desc_sig_punctuation classes=\"p\">\n",
                    "            :\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                    "                int\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            =\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                    "            2\n",
                    "    <desc_optional xml:space=\"preserve\">\n",
                    "        <desc_parameter xml:space=\"preserve\">\n",
                    "            <desc_sig_name classes=\"n\">\n",
                    "                y\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/dup_param_warning: the pseudo fallback for `f(a, a)`
    /// still carries both attrs — they are unconditional on this path too
    /// ([SIG §1.4]).
    #[test]
    fn pseudo_attrs_are_unconditional() {
        assert_eq!(
            pseudo("a, a"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            a\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            a\n",
                )
            ]
            .concat()
        );
    }

    /// Star prefixes survive verbatim inside the pseudo name (the
    /// partition puts them in param_name).
    #[test]
    fn pseudo_star_names_kept() {
        assert_eq!(
            pseudo("*args, **kw"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            *args\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            **kw\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/imbalance: `func(a[, b)` — total bracket imbalance
    /// discards the built list for a FRESH paramlist with NO multi_line
    /// attrs holding the raw arglist as one desc_parameter (the single
    /// attr-less exception, [SIG §1.4]).
    #[test]
    fn pseudo_imbalance_gives_attrless_raw_parameter() {
        assert_eq!(
            pseudo("a[, b"),
            concat!(
                "<desc_parameterlist xml:space=\"preserve\">\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        a[, b\n",
            )
        );
        // Too many closers hits the same give-up route.
        assert_eq!(
            pseudo("a], b"),
            concat!(
                "<desc_parameterlist xml:space=\"preserve\">\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        a], b\n",
            )
        );
    }

    // -- parse_type_list ([PY §2.4], probe-verbatim) -----------------------

    /// Probe task5/class_typeparams: `C[T, *Ts, **P]` — plain name,
    /// `*`/`**` operators before variadic names.
    #[test]
    fn tp_plain_and_variadic_params() {
        assert_eq!(
            tp("T, *Ts, **P"),
            [
                TPL_HEAD,
                concat!(
                    "    <desc_type_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            T\n",
                    "    <desc_type_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            *\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            Ts\n",
                    "    <desc_type_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            **\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            P\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/typeparams_bound: bound after `:` + space inside one
    /// desc_sig_name wrapper.
    #[test]
    fn tp_bound() {
        assert_eq!(
            tp("T: int"),
            [
                TPL_HEAD,
                concat!(
                    "    <desc_type_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            T\n",
                    "        <desc_sig_punctuation classes=\"p\">\n",
                    "            :\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                    "                int\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/typeparams_constraint: `f[T: (int, str)]` — the tuple
    /// loses its parens in `_parse_annotation`, so they are re-added as
    /// punctuation around the wrapper.
    #[test]
    fn tp_constraint_reparenthesized() {
        assert_eq!(
            tp("T: (int, str)"),
            [
                TPL_HEAD,
                concat!(
                    "    <desc_type_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            T\n",
                    "        <desc_sig_punctuation classes=\"p\">\n",
                    "            :\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_punctuation classes=\"p\">\n",
                    "            (\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                    "                int\n",
                    "            <desc_sig_punctuation classes=\"p\">\n",
                    "                ,\n",
                    "            <desc_sig_space classes=\"w\">\n",
                    "                 \n",
                    "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"str\" reftype=\"class\">\n",
                    "                str\n",
                    "        <desc_sig_punctuation classes=\"p\">\n",
                    "            )\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/typeparams_default: `C[T = int]` — tp defaults always
    /// space-wrap `=` (unlike arglists) and the text is token-rebuilt, not
    /// pycode-unparsed.
    #[test]
    fn tp_default() {
        assert_eq!(
            tp("T = int"),
            [
                TPL_HEAD,
                concat!(
                    "    <desc_type_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            T\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            =\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                    "            int\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/typeparams_bound_default: bound and default combine.
    #[test]
    fn tp_bound_and_default() {
        assert_eq!(
            tp("T: int = str"),
            [
                TPL_HEAD,
                concat!(
                    "    <desc_type_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            T\n",
                    "        <desc_sig_punctuation classes=\"p\">\n",
                    "            :\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                    "                int\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            =\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                    "            str\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/typeparams_star_default: `C[*Ts = *tuple[int, ...]]` —
    /// PEP 696 default on a TypeVarTuple; the unpack `*` stays flush
    /// (native) while `,` gets its trailing space in the token rebuild.
    #[test]
    fn tp_star_default_native_unpack() {
        assert_eq!(
            tp("*Ts = *tuple[int, ...]"),
            [
                TPL_HEAD,
                concat!(
                    "    <desc_type_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            *\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            Ts\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            =\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                    "            *tuple[int, ...]\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/typeparams_union_bound: `C[T: int | str]` — the `|`
    /// gets spaces in the token rebuild and xrefs in the annotation walk.
    #[test]
    fn tp_union_bound() {
        assert_eq!(
            tp("T: int | str"),
            [
                TPL_HEAD,
                concat!(
                    "    <desc_type_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            T\n",
                    "        <desc_sig_punctuation classes=\"p\">\n",
                    "            :\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                    "                int\n",
                    "            <desc_sig_space classes=\"w\">\n",
                    "                 \n",
                    "            <desc_sig_punctuation classes=\"p\">\n",
                    "                |\n",
                    "            <desc_sig_space classes=\"w\">\n",
                    "                 \n",
                    "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"str\" reftype=\"class\">\n",
                    "                str\n",
                )
            ]
            .concat()
        );
    }

    /// A bound/constraint on a variadic type parameter is the exact
    /// SyntaxError sphinx raises (`_annotations.py:308-315`); its message
    /// feeds task 6's tp-list warning verbatim.
    #[test]
    fn tp_variadic_bound_is_error() {
        assert_eq!(
            parse_type_list("*Ts: int", false, &dctx(), &dcfg()),
            Err(SigParseError::Syntax(
                "type parameter bound or constraint is not allowed for variadic positional parameters"
                    .to_string()
            ))
        );
        assert_eq!(
            parse_type_list("**P: int", false, &dctx(), &dcfg()),
            Err(SigParseError::Syntax(
                "type parameter bound or constraint is not allowed for variadic keyword parameters"
                    .to_string()
            ))
        );
        // ...but a DEFAULT on a variadic is fine (PEP 696, probe
        // task5/typeparams_star_default).
        assert!(parse_type_list("*Ts = 1", false, &dctx(), &dcfg()).is_ok());
    }

    /// Unclosed brackets mirror tokenize's TokenError (sphinx's tp-list
    /// arm catches ANY exception into its warning).
    #[test]
    fn tp_unclosed_bracket_is_error() {
        assert!(parse_type_list("T: (int", false, &dctx(), &dcfg()).is_err());
    }

    /// The tp list carries the same unconditional attr pair (probes
    /// long_typeparams / long_typeparams_single, [PY §2.5]).
    #[test]
    fn tp_attr_values_follow_inputs() {
        let flipped = parse_type_list("T", true, &dctx(), &dcfg())
            .unwrap()
            .pformat();
        assert!(flipped.starts_with(
            "<desc_type_parameter_list multi_line_parameter_list=\"1\" multi_line_trailing_comma=\"1\""
        ));
        let no_comma_cfg = PySigConfig {
            python_trailing_comma_in_multi_line_signatures: false,
            ..PySigConfig::default()
        };
        let no_comma = parse_type_list("T", true, &dctx(), &no_comma_cfg)
            .unwrap()
            .pformat();
        assert!(no_comma.starts_with(
            "<desc_type_parameter_list multi_line_parameter_list=\"1\" multi_line_trailing_comma=\"0\""
        ));
    }

    // -- post-commit probe pins (probe task5/probe_arglist2, see report) ---

    /// Probe task5/neg_default: a unary minus renders flush against the
    /// numeric SOURCE segment — `y=- 2` becomes `-2`.
    #[test]
    fn sig_negative_defaults_render_flush() {
        let params = signature_from_str("x=-1, y=- 2").unwrap();
        assert_eq!(params[0].default.as_deref(), Some("-1"));
        assert_eq!(params[1].default.as_deref(), Some("-2"));
    }

    /// Probe task5/none_default_str_ann: a string annotation survives as
    /// its repr (and task 4 renders it as a literal string, not an xref);
    /// `None` defaults are repr'd.
    #[test]
    fn sig_string_annotation_and_none_default() {
        let params = signature_from_str("x: 'A[int]' = None").unwrap();
        assert_eq!(params[0].annotation.as_deref(), Some("'A[int]'"));
        assert_eq!(params[0].default.as_deref(), Some("None"));
    }

    /// Probe task5/varargs_annotated: annotations attach after the
    /// `*`/`**` operator + name pair inside the same desc_parameter.
    #[test]
    fn arglist_annotated_variadics() {
        assert_eq!(
            parsed("*args: int, **kw: str"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            *\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            args\n",
                    "        <desc_sig_punctuation classes=\"p\">\n",
                    "            :\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                    "                int\n",
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_operator classes=\"o\">\n",
                    "            **\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            kw\n",
                    "        <desc_sig_punctuation classes=\"p\">\n",
                    "            :\n",
                    "        <desc_sig_space classes=\"w\">\n",
                    "             \n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"str\" reftype=\"class\">\n",
                    "                str\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/pseudo_empty_default: `f(a=[, b])` — the pseudo
    /// parser's `if default_value:` truthiness drops an empty default
    /// entirely (the `[` was already stripped as an optional-opener).
    #[test]
    fn pseudo_empty_default_is_dropped() {
        assert_eq!(
            pseudo("a=[, b]"),
            [
                PL_HEAD,
                concat!(
                    "    <desc_parameter xml:space=\"preserve\">\n",
                    "        <desc_sig_name classes=\"n\">\n",
                    "            a\n",
                    "    <desc_optional xml:space=\"preserve\">\n",
                    "        <desc_parameter xml:space=\"preserve\">\n",
                    "            <desc_sig_name classes=\"n\">\n",
                    "                b\n",
                )
            ]
            .concat()
        );
    }

    /// Probe task5/eq_in_string_default + tuple_default: `=`/`:` inside
    /// string literals never split, and tuple defaults keep canonical
    /// parens.
    #[test]
    fn sig_string_and_tuple_default_edges() {
        let params = signature_from_str("x='a=b', y: str='c:d', z=(1, 2), w=()").unwrap();
        assert_eq!(params[0].default.as_deref(), Some("'a=b'"));
        assert_eq!(params[1].annotation.as_deref(), Some("str"));
        assert_eq!(params[1].default.as_deref(), Some("'c:d'"));
        assert_eq!(params[2].default.as_deref(), Some("(1, 2)"));
        assert_eq!(params[3].default.as_deref(), Some("()"));
    }

    // -- totality ----------------------------------------------------------

    /// No entry point panics on arbitrary garbage (grammar, lexer and
    /// nesting edge cases alike).
    #[test]
    fn totality_on_garbage_input() {
        let horrors = [
            "((((((",
            "]]]]",
            "'unterminated",
            "a=(",
            "\\",
            "\u{1f980}",
            "a: :",
            "=x",
            ":int",
            "x=='y'",
            "a[b[c[d[",
            "\"\"\"",
            "0x, 1_, 1e",
            "f'{a,b}'",
            "., .., ...",
        ];
        let deep_parens = "(".repeat(10_000);
        let deep_brackets = "[, ".repeat(5_000);
        for arglist in horrors
            .iter()
            .copied()
            .chain([deep_parens.as_str(), deep_brackets.as_str()])
        {
            let _ = signature_from_str(arglist);
            let _ = parse_arglist(arglist, false, &dctx(), &dcfg());
            let _ = pseudo_parse_arglist(arglist, true, &dctx(), &dcfg());
            let _ = parse_type_list(arglist, false, &dctx(), &dcfg());
        }
    }

    /// PEP 646 `def f(*args: *Ts)`: CPython's `star_annotation`
    /// production makes `*args` the only slot a starred annotation may
    /// occupy, `signature_from_str` hands sphinx the annotation string
    /// `*Ts`, and `_parse_annotation` splits the `*` off into a
    /// `desc_sig_operator`.
    ///
    // oracle: `_parse_arglist('*args: *Ts', env)` under sphinx 9.1.0 /
    // docutils 0.22.4 (scratchpad A/p7.py).
    #[test]
    fn pep_646_star_annotation_on_var_positional() {
        assert_eq!(
            parsed("*args: *Ts"),
            concat!(
                "<desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_operator classes=\"o\">\n",
                "            *\n",
                "        <desc_sig_name classes=\"n\">\n",
                "            args\n",
                "        <desc_sig_punctuation classes=\"p\">\n",
                "            :\n",
                "        <desc_sig_space classes=\"w\">\n",
                "             \n",
                "        <desc_sig_name classes=\"n\">\n",
                "            <desc_sig_operator classes=\"o\">\n",
                "                *\n",
                "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"Ts\" reftype=\"class\">\n",
                "                Ts\n",
            )
        );
    }

    /// The bracketed spelling autodoc emits. Before the
    /// `star_annotation` slot existed this fell all the way through to
    /// `pseudo_parse_arglist`, whose naive comma split then broke on the
    /// inner comma and produced an attribute-less parameter list.
    ///
    // oracle: `_parse_arglist('*args: *tuple[int, ...]', env)`
    // (scratchpad A/p7.py).
    #[test]
    fn pep_646_star_annotation_over_a_bracketed_unpack() {
        assert_eq!(
            parsed("*args: *tuple[int, ...]"),
            concat!(
                "<desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_operator classes=\"o\">\n",
                "            *\n",
                "        <desc_sig_name classes=\"n\">\n",
                "            args\n",
                "        <desc_sig_punctuation classes=\"p\">\n",
                "            :\n",
                "        <desc_sig_space classes=\"w\">\n",
                "             \n",
                "        <desc_sig_name classes=\"n\">\n",
                "            <desc_sig_operator classes=\"o\">\n",
                "                *\n",
                "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"tuple\" reftype=\"class\">\n",
                "                tuple\n",
                "            <desc_sig_punctuation classes=\"p\">\n",
                "                [\n",
                "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                "                int\n",
                "            <desc_sig_punctuation classes=\"p\">\n",
                "                ,\n",
                "            <desc_sig_space classes=\"w\">\n",
                "                 \n",
                "            <desc_sig_punctuation classes=\"p\">\n",
                "                ...\n",
                "            <desc_sig_punctuation classes=\"p\">\n",
                "                ]\n",
            )
        );
    }

    /// The rest of the list is unaffected by the starred annotation.
    ///
    // oracle: `_parse_arglist('a, *args: *Ts, b', env)` (scratchpad
    // A/p7.py).
    #[test]
    fn star_annotation_leaves_its_neighbours_alone() {
        assert_eq!(
            parsed("a, *args: *Ts, b"),
            concat!(
                "<desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_name classes=\"n\">\n",
                "            a\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_operator classes=\"o\">\n",
                "            *\n",
                "        <desc_sig_name classes=\"n\">\n",
                "            args\n",
                "        <desc_sig_punctuation classes=\"p\">\n",
                "            :\n",
                "        <desc_sig_space classes=\"w\">\n",
                "             \n",
                "        <desc_sig_name classes=\"n\">\n",
                "            <desc_sig_operator classes=\"o\">\n",
                "                *\n",
                "            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"Ts\" reftype=\"class\">\n",
                "                Ts\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_name classes=\"n\">\n",
                "            b\n",
            )
        );
    }

    /// `visit_Constant` recovers numeric source text by AST position, so
    /// the callee's literals keep their own spellings. `NumPool` hands
    /// tokens out in render order instead, so the `Call` arm must render
    /// the callee BEFORE the arguments — otherwise `a(0x10).b(16)` comes
    /// back as `a(16).b(0x10)` (both re-parse to 16, so the pool's
    /// value check cannot catch the swap).
    ///
    // oracle: `_parse_arglist('x=a(0x10).b(16)', env)` (scratchpad
    // A/p7.py); `signature_from_str` gives the default 'a(0x10).b(16)'.
    #[test]
    fn numeric_source_recovery_follows_a_chained_call() {
        assert_eq!(
            parsed("x=a(0x10).b(16)"),
            concat!(
                "<desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_name classes=\"n\">\n",
                "            x\n",
                "        <desc_sig_operator classes=\"o\">\n",
                "            =\n",
                "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                "            a(0x10).b(16)\n",
            )
        );
    }

    /// The same inversion with the callee itself a call.
    ///
    // oracle: `_parse_arglist('x=g(0xFF)(255)', env)` (scratchpad
    // A/p7.py).
    #[test]
    fn numeric_source_recovery_follows_a_called_call() {
        assert_eq!(
            parsed("x=g(0xFF)(255)"),
            concat!(
                "<desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_name classes=\"n\">\n",
                "            x\n",
                "        <desc_sig_operator classes=\"o\">\n",
                "            =\n",
                "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                "            g(0xFF)(255)\n",
            )
        );
    }

    /// The other failure mode: mismatching values used to abandon the
    /// whole fragment to `repr` form, printing `P(493).mask(18)`.
    ///
    // oracle: `_parse_arglist('x=P(0o755).mask(0o022)', env)`
    // (scratchpad A/p7.py).
    #[test]
    fn numeric_source_recovery_keeps_octal_spellings_in_a_chain() {
        assert_eq!(
            parsed("x=P(0o755).mask(0o022)"),
            concat!(
                "<desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_name classes=\"n\">\n",
                "            x\n",
                "        <desc_sig_operator classes=\"o\">\n",
                "            =\n",
                "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                "            P(0o755).mask(0o022)\n",
            )
        );
    }

    /// `sphinx.pycode.ast` has a first-class `visit_BoolOp`, so an
    /// `and`/`or` default renders through the AST path — it must not drop
    /// the whole list into `pseudo_parse_arglist`, which would replace the
    /// PEP 3102 separator's `abbreviation` with a bare `desc_sig_name`.
    ///
    // oracle: `_parse_arglist('a, *, x=A or B', env)` (scratchpad
    // A/p7.py).
    #[test]
    fn boolop_default_keeps_the_keyword_only_separator() {
        assert_eq!(
            parsed("a, *, x=A or B"),
            concat!(
                "<desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_name classes=\"n\">\n",
                "            a\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_operator classes=\"keyword-only-separator o\">\n",
                "            <abbreviation explanation=\"Keyword-only parameters separator (PEP 3102)\">\n",
                "                *\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_name classes=\"n\">\n",
                "            x\n",
                "        <desc_sig_operator classes=\"o\">\n",
                "            =\n",
                "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                "            A or B\n",
            )
        );
    }

    /// The PEP 570 half of the same shape.
    ///
    // oracle: `_parse_arglist('a=A and B, /', env)` (scratchpad
    // A/p7.py).
    #[test]
    fn boolop_default_keeps_the_positional_only_separator() {
        assert_eq!(
            parsed("a=A and B, /"),
            concat!(
                "<desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_name classes=\"n\">\n",
                "            a\n",
                "        <desc_sig_operator classes=\"o\">\n",
                "            =\n",
                "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                "            A and B\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_operator classes=\"positional-only-separator o\">\n",
                "            <abbreviation explanation=\"Positional-only parameter separator (PEP 570)\">\n",
                "                /\n",
            )
        );
    }

    /// `visit_BoolOp` is a plain `' and '`/`' or '` join with no
    /// precedence table, so a mixed chain keeps its flat source spelling
    /// (which `ast.unparse` would parenthesize as `a and b or c`).
    ///
    // oracle: `_parse_arglist('x=a and b or c', env)` (scratchpad
    // A/p7.py).
    #[test]
    fn boolop_default_renders_without_parentheses() {
        assert_eq!(
            parsed("x=a and b or c"),
            concat!(
                "<desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "    <desc_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_name classes=\"n\">\n",
                "            x\n",
                "        <desc_sig_operator classes=\"o\">\n",
                "            =\n",
                "        <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                "            a and b or c\n",
            )
        );
    }

    /// PEP 695 `f[T:]`: `_parse_annotation('')` is the empty node list, so
    /// `if not annotation: continue` (`_annotations.py:428-430`) drops the
    /// whole type parameter — default included. We used to render an
    /// empty-target `pending_xref` there, which also reached the resolver.
    ///
    // oracle: `_parse_type_list(tp, env)` for each spelling under sphinx
    // 9.1.0 / docutils 0.22.4 (scratchpad A/p7.py).
    #[test]
    fn an_empty_type_parameter_bound_drops_the_parameter() {
        assert_eq!(
            tp("T:"),
            "<desc_type_parameter_list multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
            "empty bound in T:"
        );
        assert_eq!(
            tp("T: "),
            "<desc_type_parameter_list multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
            "empty bound in T: "
        );
        assert_eq!(
            tp("T:, U"),
            concat!(
                "<desc_type_parameter_list multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "    <desc_type_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_name classes=\"n\">\n",
                "            U\n",
            ),
            "empty bound in T:, U"
        );
        assert_eq!(
            tp("T, U:"),
            concat!(
                "<desc_type_parameter_list multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "    <desc_type_parameter xml:space=\"preserve\">\n",
                "        <desc_sig_name classes=\"n\">\n",
                "            T\n",
            ),
            "empty bound in T, U:"
        );
        assert_eq!(
            tp("T: = int"),
            "<desc_type_parameter_list multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
            "empty bound in T: = int"
        );
    }
}
