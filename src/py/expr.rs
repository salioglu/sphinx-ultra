//! Python expression parsing with `ast.unparse`-normalized output.
//!
//! Sphinx renders parameter defaults and (pieces of) annotations by running
//! them through `ast.parse` + `ast.unparse` (`sphinx/util/inspect.py` routes
//! them through `ast_unparse`). `parse_py_expr` + [`unparse`] reproduce that
//! round trip byte-for-byte for the expression subset the py domain needs;
//! anything outside the subset — `lambda`, comprehensions, f-strings,
//! walrus, `await`, `yield`, conditional expressions, comparisons, slices,
//! `**kwargs` in calls, complex literals — is a [`PyExprError`], which
//! routes callers into the same fallback paths Sphinx takes when
//! `ast.parse` raises `SyntaxError`.
//!
//! Sphinx does not use one parse mode everywhere, so neither do we:
//!
//! * [`parse_py_expr`] is `ast.parse(s, mode='eval')` — the mode reached
//!   through `signature_from_str`'s `def func(...)` wrapper, where a
//!   default value sits in an expression slot. A top-level `*a` is a
//!   `SyntaxError` there.
//! * [`parse_py_expr_stmt`] is plain `ast.parse(s)` (exec), the mode
//!   `_parse_annotation` uses (`_annotations.py:232`, `type_comments=True`).
//!   A top-level `Starred` is a legal `Expr` statement there, so PEP 646
//!   `*Ts` / `*tuple[int, ...]` annotations parse, and an empty (or
//!   blank) source is `Module(body=[])` rather than an error.
//! * [`parse_py_star_annotation`] is CPython's `star_annotation`
//!   production (`'*' bitwise_or | expression`), the only grammar slot
//!   where `*args: *Ts` is legal — i.e. the annotation of a var-positional
//!   parameter, which `signature_from_str` hands to Sphinx as `'*Ts'`.
//!
//! Every normalization rule implemented here is pinned by the unit-test
//! oracle battery below, generated with the REAL pinned toolchain
//! (`uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4'`),
//! never from memory. The paren-placement logic is a faithful port of
//! CPython 3.12 `ast._Unparser` (`Lib/ast.py`): its `_Precedence` table,
//! `require_parens`, `items_view`, and the per-node visitors for the subset.
//!
//! Known, documented divergences (all vanishingly rare in signatures, and
//! all *conservative* — we return `Err` and the caller falls back, we never
//! print something different from `ast.unparse`):
//!
//! * `\N{...}` escapes need the Unicode name database (no new deps) → `Err`.
//! * Lone-surrogate escapes (`'\ud800'`) cannot live in a Rust `String` →
//!   `Err`.
//! * Identifier characters are approximated as `char::is_alphabetic` + `_`
//!   (start) and additionally Nd digits via the wave-3
//!   [`crate::rst::digits`] tables (continue), instead of exact
//!   `XID_Start`/`XID_Continue`; identifiers are NFKC-normalized like
//!   CPython's tokenizer.
//! * `repr()`'s "printable" test for exotic non-ASCII characters inside
//!   string constants uses `char::is_control` plus a curated Zs/Zl/Zp/Cf/Co
//!   table rather than full Unicode category data. A full-codespace sweep
//!   against the pinned interpreter shows the table never over-escapes, and
//!   the only remaining under-escape class is the unassigned (Cn) code
//!   points, which render unescaped where CPython would escape them — a
//!   sanctioned, documented divergence.
//! * Exec mode only ever yields ONE statement here: a source holding
//!   several (`int;`, `int\nstr` — `Module(body=[Expr, Expr])`, which
//!   Sphinx's `functools.reduce` over `node.body` renders as the
//!   concatenation of both) is `Err`, so the caller falls back to a single
//!   whole-text xref where Sphinx renders each statement.
//! * [`MAX_DEPTH`] is a whole-expression complexity budget, not a nesting
//!   depth: trailers (`a.b.c`, `f(x)(y)`, `m[i][j]`) and binop folds each
//!   charge it too, so a FLAT chain of roughly 200 operations is `Err`
//!   where CPython parses it. Err-side and conservative like the rest —
//!   the caller falls back — and no realistic signature comes near it.

use std::fmt;

use unicode_normalization::UnicodeNormalization;

use crate::rst::digits::decimal_digit_value;

/// A parsed Python expression — the subset of `ast.expr` the py domain
/// walks (task-3 brief). Negative literals are `UnaryOp(USub, Constant)`,
/// exactly as in `ast`; dotted names are `Attribute` chains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PyExpr {
    /// `ast.Name`.
    Name(String),
    /// `ast.Attribute`: `value.attr`.
    Attribute(Box<PyExpr>, String),
    /// `ast.Subscript`. `x[a, b]` stores the slice as a `Tuple`, `x[a]` as
    /// the bare expression — mirroring `ast`.
    Subscript {
        value: Box<PyExpr>,
        slice: Box<PyExpr>,
    },
    /// `ast.BinOp`, including `BitOr` for PEP 604 unions.
    BinOp {
        left: Box<PyExpr>,
        op: PyOp,
        right: Box<PyExpr>,
    },
    /// `ast.UnaryOp`.
    UnaryOp { op: PyUnaryOp, operand: Box<PyExpr> },
    /// `ast.Constant`.
    Constant(PyConst),
    /// `ast.Tuple`.
    Tuple(Vec<PyExpr>),
    /// `ast.List`.
    List(Vec<PyExpr>),
    /// `ast.Set` (never empty when produced by the parser: `{}` is a dict).
    Set(Vec<PyExpr>),
    /// `ast.Dict`; a `None` key is a `**` unpack (PEP 448).
    Dict(Vec<(Option<PyExpr>, PyExpr)>),
    /// `ast.Call`. `args` may contain `Starred` entries; `keywords` with
    /// `arg=None` (`f(**kw)`) are unrepresentable and parse as `Err`.
    Call {
        func: Box<PyExpr>,
        args: Vec<PyExpr>,
        kwargs: Vec<(String, PyExpr)>,
    },
    /// `ast.Starred` — inside calls, displays and subscript tuples, plus
    /// the two top-level slots exec mode and `star_annotation` open up
    /// (see the module docs).
    Starred(Box<PyExpr>),
    /// `ast.BoolOp`: `a and b`, `a or b or c`. Chains of the *same*
    /// operator flatten into one node, exactly as CPython's parser builds
    /// them.
    BoolOp { op: PyBoolOp, values: Vec<PyExpr> },
}

/// `ast.boolop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyBoolOp {
    And,
    Or,
}

/// `ast.BinOp` operators, i.e. every Python binary operator that is an
/// `ast.operator` (comparisons are a different node kind and stay out of
/// scope; `and`/`or` are [`PyBoolOp`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyOp {
    Add,
    Sub,
    Mult,
    MatMult,
    Div,
    Mod,
    Pow,
    LShift,
    RShift,
    BitOr,
    BitXor,
    BitAnd,
    FloorDiv,
}

/// `ast.unaryop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyUnaryOp {
    Invert,
    Not,
    UAdd,
    USub,
}

/// `ast.Constant` values. Numeric payloads are kept as **normalized
/// strings** (what `repr(value)` prints), so arbitrary-precision ints
/// survive and floats carry Python's shortest-repr text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PyConst {
    None,
    True,
    False,
    Ellipsis,
    /// Normalized decimal digits, no sign, no underscores (`0xFF` → `255`).
    Int(String),
    /// Python `repr(float)` text (`1e-3` → `0.001`, `1e16` → `1e+16`).
    Float(String),
    /// A str constant. `value` is the *decoded* content; `quote` is the
    /// quote character `repr()` chooses for it (recomputed by [`unparse`],
    /// stored here so doctree consumers agree with the rendered text);
    /// `u_prefix` preserves `ast.Constant.kind == 'u'` (`u'x'` unparses as
    /// `u'x'`).
    Str {
        value: String,
        quote: char,
        u_prefix: bool,
    },
    /// A bytes constant: decoded bytes.
    Bytes(Vec<u8>),
}

/// Opaque parse error; callers only branch on `Err` (the message exists for
/// diagnostics and tests, not for dispatch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyExprError {
    msg: String,
}

impl PyExprError {
    fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }
}

impl fmt::Display for PyExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid python expression: {}", self.msg)
    }
}

impl std::error::Error for PyExprError {}

/// Parse a whole string as one Python expression (`ast.parse(s,
/// mode='eval')` for the supported subset). Trailing garbage is an error;
/// so is anything outside the subset. Never panics.
pub fn parse_py_expr(s: &str) -> Result<PyExpr, PyExprError> {
    let mut parser = Parser::new(tokenize(s)?, TopLevel::Eval);
    if parser.peek().is_none() {
        return Err(PyExprError::new("empty expression"));
    }
    parser.parse_top()
}

/// Parse a whole string the way `_parse_annotation` does — plain
/// `ast.parse(s)`, i.e. exec mode (`_annotations.py:232`).
///
/// `Ok(None)` is `Module(body=[])`: an empty or blank source holds no
/// statement at all, and Sphinx's `functools.reduce` over `node.body`
/// then yields the empty node list (`_annotations.py:150-151`).
/// `Ok(Some(e))` is the single `Expr` statement case, which is what every
/// annotation in the subset is. A top-level `Starred` — bare (`*Ts`) or
/// inside a bare tuple (`*a, b`) — is legal here and is *not* legal in
/// [`parse_py_expr`].
pub fn parse_py_expr_stmt(s: &str) -> Result<Option<PyExpr>, PyExprError> {
    if has_leading_indent(s) {
        // `ast.parse('  int')` raises IndentationError — a SyntaxError
        // subclass, so `_parse_annotation` takes its `except SyntaxError`
        // arm and xrefs the *unstripped* text.
        return Err(PyExprError::new("unexpected indent"));
    }
    let mut parser = Parser::new(tokenize(s)?, TopLevel::Exec);
    if parser.peek().is_none() {
        return Ok(None);
    }
    parser.parse_top().map(Some)
}

/// Parse the annotation of a var-positional parameter — CPython's
/// `star_annotation` production (`'*' bitwise_or | expression`), the only
/// place a PEP 646 unpack may appear in a signature.
pub fn parse_py_star_annotation(s: &str) -> Result<PyExpr, PyExprError> {
    let mut parser = Parser::new(tokenize(s)?, TopLevel::StarAnnotation);
    if parser.peek().is_none() {
        return Err(PyExprError::new("empty expression"));
    }
    parser.parse_top()
}

/// `ast.parse`'s IndentationError test, narrowed to the shapes an
/// annotation string can take: the first line that carries a token must
/// not start with a space or a tab. A form feed is not indentation
/// (`'\x0cint'` parses; `' \x0c int'` does not, because of the space), and
/// wholly blank lines are skipped.
fn has_leading_indent(s: &str) -> bool {
    for line in s.split('\n') {
        if line
            .chars()
            .all(|c| matches!(c, ' ' | '\t' | '\x0c' | '\r' | '\x0b'))
        {
            continue;
        }
        return line.starts_with([' ', '\t']);
    }
    false
}

/// Render an expression exactly as CPython 3.12 `ast.unparse` would render
/// the equivalent `ast` tree (a port of `ast._Unparser` for the subset).
pub fn unparse(e: &PyExpr) -> String {
    let mut out = String::new();
    // _Unparser.get_precedence defaults to _Precedence.TEST for any node
    // that never had set_precedence called on it — the root included.
    write_expr(&mut out, e, Prec::Test);
    out
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

/// Combined complexity budget: bounds active parser recursion (nesting)
/// *and*, via [`Parser::charge_node`], the left-extending chains built
/// iteratively (attribute/call/subscript trailers, binop folds). Together
/// they guarantee any `Ok` tree has height O(`MAX_DEPTH`), keeping the
/// recursive [`unparse`] walk and the derived recursive `Drop` of nested
/// `Box<PyExpr>` stack-safe. Hostile input hits `Err` instead of a stack
/// overflow (CPython raises `SyntaxError: too many nested parentheses`
/// similarly).
const MAX_DEPTH: u32 = 200;

/// Python's hard keywords (`keyword.kwlist`, 3.12). Soft keywords
/// (`match`, `case`, `type`, `_`) are ordinary names in expressions.
const KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Name(String),
    /// Normalized decimal digits (see [`PyConst::Int`]).
    Int(String),
    /// Normalized `repr(float)` text (see [`PyConst::Float`]).
    Float(String),
    /// Decoded str-literal content.
    Str {
        value: String,
        u_prefix: bool,
    },
    /// Decoded bytes-literal content.
    Bytes(Vec<u8>),
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Dot,
    Ellipsis,
    Eq,
    Plus,
    Minus,
    Star,
    DoubleStar,
    Slash,
    DoubleSlash,
    Percent,
    At,
    Pipe,
    Caret,
    Amp,
    Tilde,
    LShift,
    RShift,
}

/// Identifier start: `_` or `Alphabetic` — a conservative stand-in for
/// `XID_Start` (module docs list the divergence).
fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

/// Identifier continue: start characters plus Nd digits, the latter via the
/// wave-3 generated tables (`unicodedata.decimal`-defined = category Nd),
/// so `x²` is rejected exactly like CPython ("invalid character '²'").
fn is_ident_continue(c: char) -> bool {
    is_ident_start(c) || decimal_digit_value(c).is_some()
}

struct Tokenizer {
    chars: Vec<char>,
    pos: usize,
}

impl Tokenizer {
    fn cur(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn at(&self, i: usize) -> Option<char> {
        self.chars.get(i).copied()
    }

    fn take_while(&mut self, pred: impl Fn(char) -> bool) -> String {
        let mut out = String::new();
        while let Some(c) = self.cur() {
            if !pred(c) {
                break;
            }
            out.push(c);
            self.pos += 1;
        }
        out
    }

    /// Skip whitespace, comments and backslash line continuations. (Slightly
    /// looser than CPython, which only allows ASCII whitespace; exotic
    /// Unicode spaces are accepted here where CPython errors.)
    fn skip_trivia(&mut self) {
        loop {
            match self.cur() {
                Some(c) if c.is_whitespace() => self.pos += 1,
                Some('#') => {
                    while !matches!(self.cur(), None | Some('\n')) {
                        self.pos += 1;
                    }
                }
                Some('\\') if matches!(self.at(self.pos + 1), Some('\n' | '\r')) => {
                    self.pos += 2;
                }
                _ => break,
            }
        }
    }

    fn scan_name_or_prefixed_string(&mut self) -> Result<Tok, PyExprError> {
        let name = self.take_while(is_ident_continue);
        if matches!(self.cur(), Some('\'' | '"')) {
            match name.to_ascii_lowercase().as_str() {
                "r" => return self.scan_string(true, false, false),
                "b" => return self.scan_string(false, true, false),
                "u" => return self.scan_string(false, false, true),
                "rb" | "br" => return self.scan_string(true, true, false),
                "f" | "rf" | "fr" => {
                    return Err(PyExprError::new(
                        "f-strings are not part of the supported expression subset",
                    ));
                }
                // Any other identifier directly before a quote is invalid
                // syntax in Python too; the parser reports the adjacency.
                _ => {}
            }
        }
        // CPython NFKC-normalizes identifiers (PEP 3131).
        let name = if name.is_ascii() {
            name
        } else {
            name.nfkc().collect()
        };
        Ok(Tok::Name(name))
    }

    fn scan_string(&mut self, raw: bool, bytes: bool, u_prefix: bool) -> Result<Tok, PyExprError> {
        let Some(quote) = self.cur() else {
            return Err(PyExprError::new("expected string quote"));
        };
        self.pos += 1;
        let triple = self.cur() == Some(quote) && self.at(self.pos + 1) == Some(quote);
        if triple {
            self.pos += 2;
        }
        let mut body = String::new();
        loop {
            let Some(c) = self.cur() else {
                return Err(PyExprError::new("unterminated string literal"));
            };
            if c == '\\' {
                // Keep the escape pair raw; decoding happens below. In raw
                // strings a backslash still shields a quote from
                // terminating the literal (and both characters survive).
                let Some(next) = self.at(self.pos + 1) else {
                    return Err(PyExprError::new("unterminated string literal"));
                };
                body.push('\\');
                body.push(next);
                self.pos += 2;
                continue;
            }
            if c == quote {
                if !triple {
                    self.pos += 1;
                    break;
                }
                if self.at(self.pos + 1) == Some(quote) && self.at(self.pos + 2) == Some(quote) {
                    self.pos += 3;
                    break;
                }
            } else if c == '\n' && !triple {
                return Err(PyExprError::new("EOL inside string literal"));
            }
            body.push(c);
            self.pos += 1;
        }
        if bytes {
            let value = if raw {
                raw_bytes(&body)?
            } else {
                decode_bytes_escapes(&body)?
            };
            Ok(Tok::Bytes(value))
        } else {
            let value = if raw {
                body
            } else {
                decode_str_escapes(&body)?
            };
            Ok(Tok::Str { value, u_prefix })
        }
    }

    fn scan_number(&mut self) -> Result<Tok, PyExprError> {
        if self.cur() == Some('0') {
            let base = match self.at(self.pos + 1) {
                Some('x' | 'X') => Some(16),
                Some('o' | 'O') => Some(8),
                Some('b' | 'B') => Some(2),
                _ => None,
            };
            if let Some(base) = base {
                self.pos += 2;
                let run = self.take_while(|c| c == '_' || c.is_digit(base));
                let digits = strip_underscores(&run, true)?;
                if digits.is_empty() || self.cur().is_some_and(|c| c.is_ascii_digit()) {
                    return Err(PyExprError::new("invalid digit in numeric literal"));
                }
                return Ok(Tok::Int(based_digits_to_decimal(&digits, base)));
            }
        }
        let int_run = self.take_while(|c| c.is_ascii_digit() || c == '_');
        let int_digits = strip_underscores(&int_run, false)?;
        let mut is_float = false;
        let mut frac_digits = String::new();
        if self.cur() == Some('.') {
            is_float = true;
            self.pos += 1;
            let frac_run = self.take_while(|c| c.is_ascii_digit() || c == '_');
            frac_digits = strip_underscores(&frac_run, false)?;
        }
        let mut exp_part: Option<String> = None;
        if matches!(self.cur(), Some('e' | 'E')) {
            let mut look = self.pos + 1;
            let mut negative = false;
            if let Some(sign @ ('+' | '-')) = self.at(look) {
                negative = sign == '-';
                look += 1;
            }
            // Only a digit makes it an exponent; otherwise the `e` is the
            // start of the next (invalid-here) identifier, as in CPython.
            if self.at(look).is_some_and(|c| c.is_ascii_digit()) {
                self.pos = look;
                let run = self.take_while(|c| c.is_ascii_digit() || c == '_');
                let digits = strip_underscores(&run, false)?;
                is_float = true;
                exp_part = Some(if negative {
                    format!("-{digits}")
                } else {
                    digits
                });
            }
        }
        if matches!(self.cur(), Some('j' | 'J')) {
            return Err(PyExprError::new(
                "complex literals are not part of the supported expression subset",
            ));
        }
        if is_float {
            let mut text = String::new();
            text.push_str(if int_digits.is_empty() {
                "0"
            } else {
                &int_digits
            });
            text.push('.');
            text.push_str(if frac_digits.is_empty() {
                "0"
            } else {
                &frac_digits
            });
            if let Some(exp) = &exp_part {
                text.push('e');
                text.push_str(exp);
            }
            let value: f64 = text
                .parse()
                .map_err(|_| PyExprError::new("invalid float literal"))?;
            return Ok(Tok::Float(py_float_repr(value)));
        }
        if int_digits.is_empty() {
            return Err(PyExprError::new("invalid numeric literal"));
        }
        if int_digits.len() > 1
            && int_digits.starts_with('0')
            && int_digits.bytes().any(|b| b != b'0')
        {
            return Err(PyExprError::new(
                "leading zeros in decimal integer literals are not permitted",
            ));
        }
        let trimmed = int_digits.trim_start_matches('0');
        Ok(Tok::Int(if trimmed.is_empty() {
            "0".to_string()
        } else {
            trimmed.to_string()
        }))
    }

    fn scan_operator(&mut self) -> Result<Tok, PyExprError> {
        let Some(c) = self.cur() else {
            return Err(PyExprError::new("unexpected end of input"));
        };
        let next = self.at(self.pos + 1);
        let (tok, len) = match (c, next) {
            ('*', Some('*')) => (Tok::DoubleStar, 2),
            ('/', Some('/')) => (Tok::DoubleSlash, 2),
            ('<', Some('<')) => (Tok::LShift, 2),
            ('>', Some('>')) => (Tok::RShift, 2),
            ('<' | '>', _) | ('=', Some('=')) | ('!', Some('=')) => {
                return Err(PyExprError::new(
                    "comparison operators are not part of the supported expression subset",
                ));
            }
            ('(', _) => (Tok::LParen, 1),
            (')', _) => (Tok::RParen, 1),
            ('[', _) => (Tok::LBracket, 1),
            (']', _) => (Tok::RBracket, 1),
            ('{', _) => (Tok::LBrace, 1),
            ('}', _) => (Tok::RBrace, 1),
            (',', _) => (Tok::Comma, 1),
            (':', _) => (Tok::Colon, 1),
            ('=', _) => (Tok::Eq, 1),
            ('+', _) => (Tok::Plus, 1),
            ('-', _) => (Tok::Minus, 1),
            ('*', _) => (Tok::Star, 1),
            ('/', _) => (Tok::Slash, 1),
            ('%', _) => (Tok::Percent, 1),
            ('@', _) => (Tok::At, 1),
            ('|', _) => (Tok::Pipe, 1),
            ('^', _) => (Tok::Caret, 1),
            ('&', _) => (Tok::Amp, 1),
            ('~', _) => (Tok::Tilde, 1),
            _ => {
                return Err(PyExprError::new(format!(
                    "unsupported character {c:?} in expression"
                )));
            }
        };
        self.pos += len;
        Ok(tok)
    }
}

fn tokenize(src: &str) -> Result<Vec<Tok>, PyExprError> {
    let mut t = Tokenizer {
        chars: src.chars().collect(),
        pos: 0,
    };
    let mut toks = Vec::new();
    loop {
        t.skip_trivia();
        let Some(c) = t.cur() else { break };
        let tok = if is_ident_start(c) {
            t.scan_name_or_prefixed_string()?
        } else if c.is_ascii_digit() {
            t.scan_number()?
        } else if c == '\'' || c == '"' {
            t.scan_string(false, false, false)?
        } else if c == '.' {
            if t.at(t.pos + 1) == Some('.') && t.at(t.pos + 2) == Some('.') {
                t.pos += 3;
                Tok::Ellipsis
            } else if t.at(t.pos + 1).is_some_and(|d| d.is_ascii_digit()) {
                t.scan_number()?
            } else {
                t.pos += 1;
                Tok::Dot
            }
        } else {
            t.scan_operator()?
        };
        toks.push(tok);
    }
    Ok(toks)
}

/// Validate PEP 515 underscore placement and strip them. `allow_leading`
/// is the base-prefix case (`0x_FF` is legal, `0x__FF`/`0x_` are not).
fn strip_underscores(run: &str, allow_leading: bool) -> Result<String, PyExprError> {
    let mut out = String::with_capacity(run.len());
    let mut last_was_underscore = false;
    for (i, c) in run.chars().enumerate() {
        if c == '_' {
            let legal = if i == 0 {
                allow_leading
            } else {
                !last_was_underscore
            };
            if !legal {
                return Err(PyExprError::new("invalid underscore in numeric literal"));
            }
            last_was_underscore = true;
        } else {
            out.push(c);
            last_was_underscore = false;
        }
    }
    if last_was_underscore {
        return Err(PyExprError::new("invalid underscore in numeric literal"));
    }
    Ok(out)
}

/// Convert base-2/8/16 digits to decimal text with unbounded precision
/// (multiply-and-add over a little-endian decimal digit vector), because
/// `repr(int)` — hence `ast.unparse` — prints every literal in decimal.
fn based_digits_to_decimal(digits: &str, base: u32) -> String {
    let mut dec: Vec<u8> = vec![0];
    for c in digits.chars() {
        let mut carry = c.to_digit(base).unwrap_or(0);
        for slot in dec.iter_mut() {
            let v = u32::from(*slot) * base + carry;
            *slot = (v % 10) as u8;
            carry = v / 10;
        }
        while carry > 0 {
            dec.push((carry % 10) as u8);
            carry /= 10;
        }
    }
    dec.iter().rev().map(|d| char::from(b'0' + d)).collect()
}

/// `ast.unparse`'s text of a float constant (`_Unparser._write_constant`):
/// Python's `repr()` of it ([`crate::pyformat::float_repr`]), an infinity
/// — only reachable via overflowing literals like `1e400` — written as
/// `_INFSTR`, `1e309`, and a NaN as `(1e309-1e309)`.
fn py_float_repr(v: f64) -> String {
    crate::pyformat::float_repr(v)
        .replace("inf", "1e309")
        .replace("nan", "(1e309-1e309)")
}

fn take_hex(chars: &[char], i: &mut usize, n: usize, kind: &str) -> Result<u32, PyExprError> {
    let mut val: u32 = 0;
    for _ in 0..n {
        let digit = chars.get(*i).and_then(|c| c.to_digit(16));
        let Some(digit) = digit else {
            return Err(PyExprError::new(format!("truncated {kind} escape")));
        };
        val = val * 16 + digit;
        *i += 1;
    }
    Ok(val)
}

fn take_octal(chars: &[char], i: &mut usize, first: char) -> u32 {
    let mut val = first.to_digit(8).unwrap_or(0);
    for _ in 0..2 {
        let Some(digit) = chars.get(*i).and_then(|c| c.to_digit(8)) else {
            break;
        };
        val = val * 8 + digit;
        *i += 1;
    }
    val
}

/// CPython str-literal escape decoding. Unknown escapes keep the backslash
/// literally (CPython emits a `SyntaxWarning` but accepts them); `\N{...}`
/// needs the Unicode name database and is a documented `Err`; surrogate
/// `\u`/`\U` values cannot exist in a Rust `String` and are `Err` too.
fn decode_str_escapes(body: &str) -> Result<String, PyExprError> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < chars.len() {
        let Some(&c) = chars.get(i) else { break };
        if c != '\\' {
            out.push(c);
            i += 1;
            continue;
        }
        let Some(&esc) = chars.get(i + 1) else {
            return Err(PyExprError::new("trailing backslash in string literal"));
        };
        i += 2;
        match esc {
            '\n' => {}
            '\\' => out.push('\\'),
            '\'' => out.push('\''),
            '"' => out.push('"'),
            'a' => out.push('\u{7}'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'v' => out.push('\u{b}'),
            '0'..='7' => {
                let val = take_octal(&chars, &mut i, esc);
                // <= 0o777 = 511, always a valid scalar.
                out.push(char::from_u32(val).unwrap_or('\u{fffd}'));
            }
            'x' => {
                let val = take_hex(&chars, &mut i, 2, "\\xXX")?;
                out.push(char::from_u32(val).unwrap_or('\u{fffd}'));
            }
            'u' => {
                let val = take_hex(&chars, &mut i, 4, "\\uXXXX")?;
                let Some(decoded) = char::from_u32(val) else {
                    return Err(PyExprError::new("surrogate escapes are not supported"));
                };
                out.push(decoded);
            }
            'U' => {
                let val = take_hex(&chars, &mut i, 8, "\\UXXXXXXXX")?;
                let Some(decoded) = char::from_u32(val) else {
                    return Err(PyExprError::new("invalid \\U escape value"));
                };
                out.push(decoded);
            }
            'N' => {
                return Err(PyExprError::new(
                    "\\N{...} escapes are not supported (no unicodedata)",
                ));
            }
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    Ok(out)
}

/// Bytes-literal escape decoding. Literal characters must be ASCII
/// (CPython: "bytes can only contain ASCII literal characters"); octal
/// escape values wrap to a byte, matching CPython (`b'\401'` → `b'\x01'`).
fn decode_bytes_escapes(body: &str) -> Result<Vec<u8>, PyExprError> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < chars.len() {
        let Some(&c) = chars.get(i) else { break };
        if c != '\\' {
            if !c.is_ascii() {
                return Err(PyExprError::new(
                    "bytes can only contain ASCII literal characters",
                ));
            }
            out.push(c as u8);
            i += 1;
            continue;
        }
        let Some(&esc) = chars.get(i + 1) else {
            return Err(PyExprError::new("trailing backslash in bytes literal"));
        };
        i += 2;
        match esc {
            '\n' => {}
            '\\' => out.push(b'\\'),
            '\'' => out.push(b'\''),
            '"' => out.push(b'"'),
            'a' => out.push(0x07),
            'b' => out.push(0x08),
            'f' => out.push(0x0c),
            'n' => out.push(b'\n'),
            'r' => out.push(b'\r'),
            't' => out.push(b'\t'),
            'v' => out.push(0x0b),
            '0'..='7' => out.push(take_octal(&chars, &mut i, esc) as u8),
            'x' => out.push(take_hex(&chars, &mut i, 2, "\\xXX")? as u8),
            // \u, \U, \N are not escapes in bytes literals: the backslash
            // stays literal, like any other unknown escape.
            other => {
                if !other.is_ascii() {
                    return Err(PyExprError::new(
                        "bytes can only contain ASCII literal characters",
                    ));
                }
                out.push(b'\\');
                out.push(other as u8);
            }
        }
    }
    Ok(out)
}

fn raw_bytes(body: &str) -> Result<Vec<u8>, PyExprError> {
    if !body.is_ascii() {
        return Err(PyExprError::new(
            "bytes can only contain ASCII literal characters",
        ));
    }
    Ok(body.bytes().collect())
}

// ---------------------------------------------------------------------------
// Parser (recursive descent over Python's expression precedence ladder)
// ---------------------------------------------------------------------------
//
// Grammar subset, loosest to tightest (the real Python levels for the ops
// we support; excluded levels — comparisons, conditional expressions,
// lambda — are `Err`):
//
//   top      := star_or_expr (',' star_or_expr)* [',']       (bare tuple)
//   expr     := bool_and ('or' bool_and)*
//   bool_and := not_expr ('and' not_expr)*
//   not_expr := 'not' not_expr | bitor
//   bitor    := bitxor ('|' bitxor)*
//   bitxor   := bitand ('^' bitand)*
//   bitand   := shift ('&' shift)*
//   shift    := arith (('<<' | '>>') arith)*
//   arith    := term (('+' | '-') term)*
//   term     := factor (('*' | '/' | '//' | '%' | '@') factor)*
//   factor   := ('+' | '-' | '~') factor | power
//   power    := postfix ['**' factor]                        (right assoc)
//   postfix  := atom ('.' NAME | '(' args ')' | '[' items ']')*
//   star_or_expr := '*' bitor | expr                         (PEP 448/646)

/// Which of `ast.parse`'s grammar entry points the *top level* of this
/// parse follows. Nothing below the top level differs between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopLevel {
    /// `mode='eval'`: no starred expression at the top level, not even
    /// inside a bare tuple (`*a`, `*a, b`, `*a,` are all SyntaxError).
    Eval,
    /// Plain `ast.parse` (exec): the top level is an `Expr` statement, so
    /// `*a` and `*a, b` are legal.
    Exec,
    /// `star_annotation` (`'*' bitwise_or | expression`): one optional
    /// leading `*`, never a starred tuple element.
    StarAnnotation,
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    depth: u32,
    top: TopLevel,
}

/// The binary operator accepted at each precedence level of `parse_binop`.
fn level_op(level: usize, tok: &Tok) -> Option<PyOp> {
    match (level, tok) {
        (0, Tok::Pipe) => Some(PyOp::BitOr),
        (1, Tok::Caret) => Some(PyOp::BitXor),
        (2, Tok::Amp) => Some(PyOp::BitAnd),
        (3, Tok::LShift) => Some(PyOp::LShift),
        (3, Tok::RShift) => Some(PyOp::RShift),
        (4, Tok::Plus) => Some(PyOp::Add),
        (4, Tok::Minus) => Some(PyOp::Sub),
        (5, Tok::Star) => Some(PyOp::Mult),
        (5, Tok::Slash) => Some(PyOp::Div),
        (5, Tok::DoubleSlash) => Some(PyOp::FloorDiv),
        (5, Tok::Percent) => Some(PyOp::Mod),
        (5, Tok::At) => Some(PyOp::MatMult),
        _ => None,
    }
}

impl Parser {
    fn new(toks: Vec<Tok>, top: TopLevel) -> Self {
        Self {
            toks,
            pos: 0,
            depth: 0,
            top,
        }
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    /// `and` / `or` / `not` lex as `Tok::Name`; this is the keyword test
    /// the boolean levels branch on.
    fn peek_keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Name(n)) if n == kw)
    }

    fn eat_keyword(&mut self, kw: &str) -> bool {
        if self.peek_keyword(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &Tok, what: &str) -> Result<(), PyExprError> {
        if self.eat(t) {
            Ok(())
        } else {
            Err(PyExprError::new(format!("expected {what}")))
        }
    }

    /// Charge one unit of the [`MAX_DEPTH`] budget for a node built by an
    /// *iterative* loop (postfix trailers, binop folds). Unlike the
    /// recursion guards in `parse_expr`/`parse_factor` this charge is never
    /// refunded: those loops deepen the tree without deepening the parser
    /// stack, so `a` + `.b` × N would otherwise return an `Ok` tree whose
    /// recursive `unparse`/`Drop` aborts the process. The permanent charge
    /// makes `MAX_DEPTH` a whole-expression complexity budget that bounds
    /// the height of every `Ok` tree.
    fn charge_node(&mut self) -> Result<(), PyExprError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(PyExprError::new("expression is too deeply nested"));
        }
        Ok(())
    }

    fn parse_top(&mut self) -> Result<PyExpr, PyExprError> {
        let first = self.parse_star_or_expr()?;
        let mut elts = vec![first];
        let mut tuple = false;
        while self.eat(&Tok::Comma) {
            tuple = true;
            if self.peek().is_none() {
                break;
            }
            elts.push(self.parse_star_or_expr()?);
        }
        if self.peek().is_some() {
            return Err(PyExprError::new("unexpected trailing input"));
        }
        // `ast.parse(mode='eval')` rejects starred expressions at the top
        // level even inside a bare tuple (`*a, b`, `b, *a`, `*a,` are all
        // SyntaxError, oracle-verified) — only displays, calls and
        // subscripts take them. Exec mode (what `_parse_annotation` uses)
        // and `star_annotation` do accept them; see the module docs.
        let starred = elts.iter().any(|e| matches!(e, PyExpr::Starred(_)));
        let star_ok = match self.top {
            TopLevel::Eval => false,
            TopLevel::Exec => true,
            TopLevel::StarAnnotation => !tuple,
        };
        if starred && !star_ok {
            return Err(PyExprError::new("cannot use starred expression here"));
        }
        if tuple {
            return Ok(PyExpr::Tuple(elts));
        }
        match elts.into_iter().next() {
            Some(e) => Ok(e),
            None => Err(PyExprError::new("empty expression")),
        }
    }

    /// `'*' bitor | expr` — starred items are only reachable from the
    /// display/call/subscript element positions that call this.
    fn parse_star_or_expr(&mut self) -> Result<PyExpr, PyExprError> {
        if self.eat(&Tok::Star) {
            Ok(PyExpr::Starred(Box::new(self.parse_binop(0)?)))
        } else {
            self.parse_expr()
        }
    }

    fn parse_expr(&mut self) -> Result<PyExpr, PyExprError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err(PyExprError::new("expression is too deeply nested"));
        }
        let result = self.parse_expr_inner();
        self.depth -= 1;
        result
    }

    fn parse_expr_inner(&mut self) -> Result<PyExpr, PyExprError> {
        self.parse_bool_op(PyBoolOp::Or)
    }

    /// `or` and `and` share one shape; `Or` recurses into `And`, `And` into
    /// `not_expr`. A chain of the same operator flattens into one
    /// `ast.BoolOp` with N values, as CPython's parser builds it.
    fn parse_bool_op(&mut self, op: PyBoolOp) -> Result<PyExpr, PyExprError> {
        let kw = match op {
            PyBoolOp::Or => "or",
            PyBoolOp::And => "and",
        };
        let first = self.parse_bool_operand(op)?;
        if !self.peek_keyword(kw) {
            return Ok(first);
        }
        let mut values = vec![first];
        while self.eat_keyword(kw) {
            self.charge_node()?;
            values.push(self.parse_bool_operand(op)?);
        }
        Ok(PyExpr::BoolOp { op, values })
    }

    fn parse_bool_operand(&mut self, op: PyBoolOp) -> Result<PyExpr, PyExprError> {
        match op {
            PyBoolOp::Or => self.parse_bool_op(PyBoolOp::And),
            PyBoolOp::And => self.parse_not(),
        }
    }

    /// `not_expr := 'not' not_expr | bitor`. `not` binds tighter than
    /// `and`/`or`, so `not a or b` is `(not a) or b`.
    fn parse_not(&mut self) -> Result<PyExpr, PyExprError> {
        if !self.peek_keyword("not") {
            return self.parse_binop(0);
        }
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err(PyExprError::new("expression is too deeply nested"));
        }
        self.pos += 1;
        let operand = self.parse_not();
        self.depth -= 1;
        Ok(PyExpr::UnaryOp {
            op: PyUnaryOp::Not,
            operand: Box::new(operand?),
        })
    }

    fn parse_binop(&mut self, level: usize) -> Result<PyExpr, PyExprError> {
        if level == 6 {
            return self.parse_factor();
        }
        let mut left = self.parse_binop(level + 1)?;
        while let Some(op) = self.peek().and_then(|t| level_op(level, t)) {
            self.pos += 1;
            self.charge_node()?;
            let right = self.parse_binop(level + 1)?;
            left = PyExpr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_factor(&mut self) -> Result<PyExpr, PyExprError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err(PyExprError::new("expression is too deeply nested"));
        }
        let result = self.parse_factor_inner();
        self.depth -= 1;
        result
    }

    fn parse_factor_inner(&mut self) -> Result<PyExpr, PyExprError> {
        let op = match self.peek() {
            Some(Tok::Plus) => Some(PyUnaryOp::UAdd),
            Some(Tok::Minus) => Some(PyUnaryOp::USub),
            Some(Tok::Tilde) => Some(PyUnaryOp::Invert),
            _ => None,
        };
        if let Some(op) = op {
            self.pos += 1;
            let operand = self.parse_factor()?;
            return Ok(PyExpr::UnaryOp {
                op,
                operand: Box::new(operand),
            });
        }
        self.parse_power()
    }

    fn parse_power(&mut self) -> Result<PyExpr, PyExprError> {
        let base = self.parse_postfix()?;
        if self.eat(&Tok::DoubleStar) {
            // Right-hand side is a factor: `2 ** -x ** 3` nests rightward.
            let right = self.parse_factor()?;
            return Ok(PyExpr::BinOp {
                left: Box::new(base),
                op: PyOp::Pow,
                right: Box::new(right),
            });
        }
        Ok(base)
    }

    fn parse_postfix(&mut self) -> Result<PyExpr, PyExprError> {
        let mut e = self.parse_atom()?;
        loop {
            if !matches!(self.peek(), Some(Tok::Dot | Tok::LParen | Tok::LBracket)) {
                return Ok(e);
            }
            self.charge_node()?;
            if self.eat(&Tok::Dot) {
                let attr = match self.peek() {
                    Some(Tok::Name(n)) => n.clone(),
                    _ => return Err(PyExprError::new("expected attribute name after '.'")),
                };
                if KEYWORDS.contains(&attr.as_str()) {
                    return Err(PyExprError::new("keyword cannot be an attribute name"));
                }
                self.pos += 1;
                e = PyExpr::Attribute(Box::new(e), attr);
            } else if self.eat(&Tok::LParen) {
                e = self.parse_call(e)?;
            } else if self.eat(&Tok::LBracket) {
                e = self.parse_subscript(e)?;
            } else {
                return Ok(e);
            }
        }
    }

    fn parse_atom(&mut self) -> Result<PyExpr, PyExprError> {
        let Some(tok) = self.peek() else {
            return Err(PyExprError::new("unexpected end of expression"));
        };
        match tok {
            Tok::Name(n) => {
                let name = n.clone();
                self.pos += 1;
                match name.as_str() {
                    "True" => Ok(PyExpr::Constant(PyConst::True)),
                    "False" => Ok(PyExpr::Constant(PyConst::False)),
                    "None" => Ok(PyExpr::Constant(PyConst::None)),
                    _ if KEYWORDS.contains(&name.as_str()) => Err(PyExprError::new(format!(
                        "keyword {name:?} is not part of the supported expression subset"
                    ))),
                    _ => Ok(PyExpr::Name(name)),
                }
            }
            Tok::Int(digits) => {
                let digits = digits.clone();
                self.pos += 1;
                Ok(PyExpr::Constant(PyConst::Int(digits)))
            }
            Tok::Float(text) => {
                let text = text.clone();
                self.pos += 1;
                Ok(PyExpr::Constant(PyConst::Float(text)))
            }
            Tok::Str { .. } | Tok::Bytes(_) => self.parse_string_concat(),
            Tok::Ellipsis => {
                self.pos += 1;
                Ok(PyExpr::Constant(PyConst::Ellipsis))
            }
            Tok::LParen => {
                self.pos += 1;
                self.parse_paren()
            }
            Tok::LBracket => {
                self.pos += 1;
                self.parse_list()
            }
            Tok::LBrace => {
                self.pos += 1;
                self.parse_brace()
            }
            other => Err(PyExprError::new(format!("unexpected token {other:?}"))),
        }
    }

    /// Adjacent string-literal concatenation. The `u` kind follows the
    /// first piece (`u'a' 'b'` → `u'ab'`, `'a' u'b'` → `'ab'` — oracle);
    /// mixing bytes and str is the same `SyntaxError` CPython raises.
    fn parse_string_concat(&mut self) -> Result<PyExpr, PyExprError> {
        enum Acc {
            Str { value: String, u_prefix: bool },
            Bytes(Vec<u8>),
        }
        let mut acc = match self.peek() {
            Some(Tok::Str { value, u_prefix }) => Acc::Str {
                value: value.clone(),
                u_prefix: *u_prefix,
            },
            Some(Tok::Bytes(b)) => Acc::Bytes(b.clone()),
            _ => return Err(PyExprError::new("expected string literal")),
        };
        self.pos += 1;
        loop {
            match (self.peek(), &mut acc) {
                (Some(Tok::Str { value, .. }), Acc::Str { value: v, .. }) => {
                    v.push_str(value);
                    self.pos += 1;
                }
                (Some(Tok::Bytes(b)), Acc::Bytes(v)) => {
                    v.extend_from_slice(b);
                    self.pos += 1;
                }
                (Some(Tok::Str { .. }), Acc::Bytes(_)) | (Some(Tok::Bytes(_)), Acc::Str { .. }) => {
                    return Err(PyExprError::new("cannot mix bytes and nonbytes literals"));
                }
                _ => break,
            }
        }
        Ok(match acc {
            Acc::Str { value, u_prefix } => {
                let quote = repr_quote(&value);
                PyExpr::Constant(PyConst::Str {
                    value,
                    quote,
                    u_prefix,
                })
            }
            Acc::Bytes(b) => PyExpr::Constant(PyConst::Bytes(b)),
        })
    }

    fn parse_paren(&mut self) -> Result<PyExpr, PyExprError> {
        if self.eat(&Tok::RParen) {
            return Ok(PyExpr::Tuple(Vec::new()));
        }
        let first = self.parse_star_or_expr()?;
        let mut elts = vec![first];
        let mut tuple = false;
        while self.eat(&Tok::Comma) {
            tuple = true;
            if self.peek() == Some(&Tok::RParen) {
                break;
            }
            elts.push(self.parse_star_or_expr()?);
        }
        self.expect(&Tok::RParen, "')'")?;
        if tuple {
            return Ok(PyExpr::Tuple(elts));
        }
        match elts.into_iter().next() {
            // `(*a)` without a comma is CPython's "can't use starred
            // expression here".
            Some(PyExpr::Starred(_)) => Err(PyExprError::new("cannot use starred expression here")),
            // Plain parentheses group; they leave no node behind.
            Some(e) => Ok(e),
            None => Err(PyExprError::new("empty parentheses")),
        }
    }

    fn parse_list(&mut self) -> Result<PyExpr, PyExprError> {
        let mut elts = Vec::new();
        if !self.eat(&Tok::RBracket) {
            loop {
                elts.push(self.parse_star_or_expr()?);
                if self.eat(&Tok::Comma) {
                    if self.eat(&Tok::RBracket) {
                        break;
                    }
                    continue;
                }
                self.expect(&Tok::RBracket, "']'")?;
                break;
            }
        }
        Ok(PyExpr::List(elts))
    }

    fn parse_brace(&mut self) -> Result<PyExpr, PyExprError> {
        if self.eat(&Tok::RBrace) {
            return Ok(PyExpr::Dict(Vec::new()));
        }
        if self.peek() == Some(&Tok::DoubleStar) {
            return self.parse_dict_items(None);
        }
        let first = self.parse_star_or_expr()?;
        if !matches!(first, PyExpr::Starred(_)) && self.peek() == Some(&Tok::Colon) {
            return self.parse_dict_items(Some(first));
        }
        self.parse_set_items(first)
    }

    fn parse_dict_items(&mut self, first_key: Option<PyExpr>) -> Result<PyExpr, PyExprError> {
        let mut items = Vec::new();
        if let Some(key) = first_key {
            self.expect(&Tok::Colon, "':'")?;
            items.push((Some(key), self.parse_expr()?));
        } else {
            self.expect(&Tok::DoubleStar, "'**'")?;
            // `'**' or_expr` — the unpacked value sits at bitor level.
            items.push((None, self.parse_binop(0)?));
        }
        loop {
            if self.eat(&Tok::Comma) {
                if self.eat(&Tok::RBrace) {
                    break;
                }
                if self.eat(&Tok::DoubleStar) {
                    items.push((None, self.parse_binop(0)?));
                } else {
                    let key = self.parse_expr()?;
                    self.expect(&Tok::Colon, "':'")?;
                    items.push((Some(key), self.parse_expr()?));
                }
                continue;
            }
            self.expect(&Tok::RBrace, "'}'")?;
            break;
        }
        Ok(PyExpr::Dict(items))
    }

    fn parse_set_items(&mut self, first: PyExpr) -> Result<PyExpr, PyExprError> {
        let mut elts = vec![first];
        loop {
            if self.eat(&Tok::Comma) {
                if self.eat(&Tok::RBrace) {
                    break;
                }
                elts.push(self.parse_star_or_expr()?);
                continue;
            }
            self.expect(&Tok::RBrace, "'}'")?;
            break;
        }
        Ok(PyExpr::Set(elts))
    }

    /// Call arguments; the opening `(` is already consumed. `*args` may
    /// follow keywords (and `ast` reorders the rendering — `f(1, x=2, *a)`
    /// unparses as `f(1, *a, x=2)`); a plain positional after a keyword is
    /// CPython's "positional argument follows keyword argument"; `**kw` is
    /// unrepresentable in [`PyExpr::Call`] and therefore `Err`.
    fn parse_call(&mut self, func: PyExpr) -> Result<PyExpr, PyExprError> {
        let mut args = Vec::new();
        let mut kwargs: Vec<(String, PyExpr)> = Vec::new();
        if !self.eat(&Tok::RParen) {
            loop {
                if self.peek() == Some(&Tok::DoubleStar) {
                    return Err(PyExprError::new(
                        "** unpacking in a call is not representable",
                    ));
                }
                if self.eat(&Tok::Star) {
                    args.push(PyExpr::Starred(Box::new(self.parse_binop(0)?)));
                } else {
                    let e = self.parse_expr()?;
                    if self.peek() == Some(&Tok::Eq) {
                        let PyExpr::Name(name) = e else {
                            return Err(PyExprError::new(
                                "keyword argument name must be an identifier",
                            ));
                        };
                        self.pos += 1;
                        kwargs.push((name, self.parse_expr()?));
                    } else {
                        if !kwargs.is_empty() {
                            return Err(PyExprError::new(
                                "positional argument follows keyword argument",
                            ));
                        }
                        args.push(e);
                    }
                }
                if self.eat(&Tok::Comma) {
                    if self.eat(&Tok::RParen) {
                        break;
                    }
                    continue;
                }
                self.expect(&Tok::RParen, "')'")?;
                break;
            }
        }
        Ok(PyExpr::Call {
            func: Box::new(func),
            args,
            kwargs,
        })
    }

    /// Subscript items; the opening `[` is already consumed. Mirrors `ast`:
    /// `x[a]` keeps the bare expression as the slice, any comma (or a lone
    /// starred item, PEP 646) makes it a `Tuple`. Slices (`:`) are outside
    /// the subset and fail here.
    fn parse_subscript(&mut self, value: PyExpr) -> Result<PyExpr, PyExprError> {
        if self.eat(&Tok::RBracket) {
            return Err(PyExprError::new("empty subscript"));
        }
        let first = self.parse_star_or_expr()?;
        let mut elts = vec![first];
        let mut tuple = false;
        while self.eat(&Tok::Comma) {
            tuple = true;
            if self.peek() == Some(&Tok::RBracket) {
                break;
            }
            elts.push(self.parse_star_or_expr()?);
        }
        self.expect(&Tok::RBracket, "']'")?;
        let slice = if tuple {
            PyExpr::Tuple(elts)
        } else {
            match elts.into_iter().next() {
                Some(starred @ PyExpr::Starred(_)) => PyExpr::Tuple(vec![starred]),
                Some(e) => e,
                None => return Err(PyExprError::new("empty subscript")),
            }
        };
        Ok(PyExpr::Subscript {
            value: Box::new(value),
            slice: Box::new(slice),
        })
    }
}

// ---------------------------------------------------------------------------
// Unparser (port of CPython 3.12 ast._Unparser for the subset)
// ---------------------------------------------------------------------------

/// `ast._Precedence`, verbatim — the derived `Ord` is the enum's
/// declaration order, matching the Python `IntEnum` values (`BOR` is an
/// alias of `EXPR` there; here `Expr` plays both roles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(dead_code)] // full table kept verbatim; unsupported nodes never construct some levels
enum Prec {
    NamedExpr,
    Tuple,
    Yield,
    Test,
    Or,
    And,
    Not,
    Cmp,
    Expr,
    BXor,
    BAnd,
    Shift,
    Arith,
    Term,
    Factor,
    Power,
    Await,
    Atom,
}

impl Prec {
    /// `_Precedence.next()` (saturating, like the Python `ValueError`
    /// fallback).
    fn next(self) -> Prec {
        match self {
            Prec::NamedExpr => Prec::Tuple,
            Prec::Tuple => Prec::Yield,
            Prec::Yield => Prec::Test,
            Prec::Test => Prec::Or,
            Prec::Or => Prec::And,
            Prec::And => Prec::Not,
            Prec::Not => Prec::Cmp,
            Prec::Cmp => Prec::Expr,
            Prec::Expr => Prec::BXor,
            Prec::BXor => Prec::BAnd,
            Prec::BAnd => Prec::Shift,
            Prec::Shift => Prec::Arith,
            Prec::Arith => Prec::Term,
            Prec::Term => Prec::Factor,
            Prec::Factor => Prec::Power,
            Prec::Power => Prec::Await,
            Prec::Await | Prec::Atom => Prec::Atom,
        }
    }
}

impl PyOp {
    fn symbol(self) -> &'static str {
        match self {
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

    /// `_Unparser.binop_precedence`.
    fn prec(self) -> Prec {
        match self {
            PyOp::Add | PyOp::Sub => Prec::Arith,
            PyOp::Mult | PyOp::MatMult | PyOp::Div | PyOp::Mod | PyOp::FloorDiv => Prec::Term,
            PyOp::LShift | PyOp::RShift => Prec::Shift,
            PyOp::BitOr => Prec::Expr,
            PyOp::BitXor => Prec::BXor,
            PyOp::BitAnd => Prec::BAnd,
            PyOp::Pow => Prec::Power,
        }
    }
}

/// `_Unparser.items_view`: comma-separated, with a trailing comma when
/// there is exactly one item (tuple views).
fn items_view(out: &mut String, elts: &[PyExpr]) {
    if let [single] = elts {
        write_expr(out, single, Prec::Test);
        out.push(',');
    } else {
        write_joined(out, elts);
    }
}

fn write_joined(out: &mut String, elts: &[PyExpr]) {
    for (i, e) in elts.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_expr(out, e, Prec::Test);
    }
}

/// One node of `_Unparser.traverse`. `ctx` is the precedence the parent
/// `set_precedence`d onto this node (default `TEST`); a node whose own
/// precedence is lower gets parenthesized (`require_parens`).
fn write_expr(out: &mut String, e: &PyExpr, ctx: Prec) {
    match e {
        PyExpr::Name(n) => out.push_str(n),
        PyExpr::Constant(c) => write_const(out, c),
        PyExpr::Attribute(value, attr) => {
            write_expr(out, value, Prec::Atom);
            // "3.__abs__()" is a syntax error, so int constants get a
            // separating space: `(1).bit_length()` → `1 .bit_length()`.
            // bool is an int subclass in Python, so True/False qualify.
            if matches!(
                value.as_ref(),
                PyExpr::Constant(PyConst::Int(_) | PyConst::True | PyConst::False)
            ) {
                out.push(' ');
            }
            out.push('.');
            out.push_str(attr);
        }
        PyExpr::Subscript { value, slice } => {
            write_expr(out, value, Prec::Atom);
            out.push('[');
            match slice.as_ref() {
                // Parentheses can be omitted when the slice tuple isn't
                // empty; items_view keeps `x[1,]` and `x[*a,]` faithful.
                PyExpr::Tuple(elts) if !elts.is_empty() => items_view(out, elts),
                other => write_expr(out, other, Prec::Test),
            }
            out.push(']');
        }
        PyExpr::Call { func, args, kwargs } => {
            write_expr(out, func, Prec::Atom);
            out.push('(');
            let mut comma = false;
            for arg in args {
                if comma {
                    out.push_str(", ");
                }
                comma = true;
                write_expr(out, arg, Prec::Test);
            }
            for (name, value) in kwargs {
                if comma {
                    out.push_str(", ");
                }
                comma = true;
                out.push_str(name);
                out.push('=');
                write_expr(out, value, Prec::Test);
            }
            out.push(')');
        }
        PyExpr::Tuple(elts) => {
            let parens = elts.is_empty() || ctx > Prec::Tuple;
            if parens {
                out.push('(');
            }
            items_view(out, elts);
            if parens {
                out.push(')');
            }
        }
        PyExpr::List(elts) => {
            out.push('[');
            write_joined(out, elts);
            out.push(']');
        }
        PyExpr::Set(elts) => {
            if elts.is_empty() {
                // `{}` would be a dict and `set` might be shadowed —
                // _Unparser writes this (unreachable from the parser).
                out.push_str("{*()}");
            } else {
                out.push('{');
                write_joined(out, elts);
                out.push('}');
            }
        }
        PyExpr::Dict(items) => {
            out.push('{');
            for (i, (key, value)) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                match key {
                    Some(k) => {
                        write_expr(out, k, Prec::Test);
                        out.push_str(": ");
                        write_expr(out, value, Prec::Test);
                    }
                    None => {
                        out.push_str("**");
                        write_expr(out, value, Prec::Expr);
                    }
                }
            }
            out.push('}');
        }
        PyExpr::Starred(value) => {
            out.push('*');
            write_expr(out, value, Prec::Expr);
        }
        PyExpr::BoolOp { op, values } => {
            // `_Unparser.visit_BoolOp`: `operator_precedence` is bumped
            // once per value and the bump is CUMULATIVE (`nonlocal`), so
            // value 0 is rendered at `own.next()`, value 1 at
            // `own.next().next()`, and so on — which is why
            // `a or (b and c)` keeps its parentheses while
            // `a and b or c` does not.
            let (own, sep) = match op {
                PyBoolOp::Or => (Prec::Or, " or "),
                PyBoolOp::And => (Prec::And, " and "),
            };
            let parens = ctx > own;
            if parens {
                out.push('(');
            }
            let mut child = own;
            for (i, value) in values.iter().enumerate() {
                if i > 0 {
                    out.push_str(sep);
                }
                child = child.next();
                write_expr(out, value, child);
            }
            if parens {
                out.push(')');
            }
        }
        PyExpr::BinOp { left, op, right } => {
            let op_prec = op.prec();
            let parens = ctx > op_prec;
            if parens {
                out.push('(');
            }
            // `**` is the one right-associative operator: its left operand
            // needs the bumped precedence, everyone else bumps the right.
            let (left_prec, right_prec) = if matches!(op, PyOp::Pow) {
                (op_prec.next(), op_prec)
            } else {
                (op_prec, op_prec.next())
            };
            write_expr(out, left, left_prec);
            out.push(' ');
            out.push_str(op.symbol());
            out.push(' ');
            write_expr(out, right, right_prec);
            if parens {
                out.push(')');
            }
        }
        PyExpr::UnaryOp { op, operand } => {
            let (symbol, op_prec) = match op {
                PyUnaryOp::Invert => ("~", Prec::Factor),
                PyUnaryOp::Not => ("not", Prec::Not),
                PyUnaryOp::UAdd => ("+", Prec::Factor),
                PyUnaryOp::USub => ("-", Prec::Factor),
            };
            let parens = ctx > op_prec;
            if parens {
                out.push('(');
            }
            out.push_str(symbol);
            // Factor prefixes stick to their operand (`-1`, not `- 1`).
            if op_prec != Prec::Factor {
                out.push(' ');
            }
            write_expr(out, operand, op_prec);
            if parens {
                out.push(')');
            }
        }
    }
}

fn write_const(out: &mut String, c: &PyConst) {
    match c {
        PyConst::None => out.push_str("None"),
        PyConst::True => out.push_str("True"),
        PyConst::False => out.push_str("False"),
        PyConst::Ellipsis => out.push_str("..."),
        PyConst::Int(digits) => out.push_str(digits),
        PyConst::Float(text) => out.push_str(text),
        PyConst::Str {
            value, u_prefix, ..
        } => {
            if *u_prefix {
                out.push('u');
            }
            // Recompute the quote from the value so hand-built constants
            // can't render inconsistently; the parser stores the same char.
            write_str_repr(out, value, repr_quote(value));
        }
        PyConst::Bytes(bytes) => write_bytes_repr(out, bytes),
    }
}

// ---------------------------------------------------------------------------
// repr() for str and bytes
// ---------------------------------------------------------------------------

/// CPython's quote selection: `'` unless the value contains `'` and no `"`.
fn repr_quote(value: &str) -> char {
    if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    }
}

/// Approximation of the non-`Cc` part of Python's `str.isprintable() ==
/// False` set (Zs/Zl/Zp except space, common Cf, Co). `char::is_control`
/// handles Cc separately; unassigned (Cn) code points are not covered —
/// the module docs carry that caveat.
fn is_nonprintable_nonascii(c: char) -> bool {
    matches!(u32::from(c),
        0xa0 | 0xad
        | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x890..=0x891 | 0x8e2
        | 0x1680 | 0x180e
        | 0x2000..=0x200f | 0x2028..=0x202f | 0x205f..=0x2064 | 0x2066..=0x206f
        | 0x3000 | 0xfeff | 0xfff9..=0xfffb
        | 0xe000..=0xf8ff
        | 0x110bd | 0x110cd | 0x13430..=0x1343f | 0x1bca0..=0x1bca3
        | 0x1d173..=0x1d17a | 0xe0001 | 0xe0020..=0xe007f
        | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
}

/// CPython `unicode_repr`: escape backslash and the chosen quote; `\n`,
/// `\r`, `\t` mnemonically; other non-printables as `\xXX`/`\uXXXX`/
/// `\UXXXXXXXX` (lowercase hex).
fn write_str_repr(out: &mut String, value: &str, quote: char) {
    use fmt::Write as _;
    out.push(quote);
    for c in value.chars() {
        if c == quote || c == '\\' {
            out.push('\\');
            out.push(c);
        } else if c == '\n' {
            out.push_str("\\n");
        } else if c == '\r' {
            out.push_str("\\r");
        } else if c == '\t' {
            out.push_str("\\t");
        } else if c.is_control() || (!c.is_ascii() && is_nonprintable_nonascii(c)) {
            let u = u32::from(c);
            let _ = if u < 0x100 {
                write!(out, "\\x{u:02x}")
            } else if u < 0x10000 {
                write!(out, "\\u{u:04x}")
            } else {
                write!(out, "\\U{u:08x}")
            };
        } else {
            out.push(c);
        }
    }
    out.push(quote);
}

/// CPython `bytes.__repr__` — fully deterministic ASCII output.
fn write_bytes_repr(out: &mut String, bytes: &[u8]) {
    use fmt::Write as _;
    let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    out.push('b');
    out.push(quote);
    for &b in bytes {
        if b == quote as u8 || b == b'\\' {
            out.push('\\');
            out.push(char::from(b));
        } else if b == b'\t' {
            out.push_str("\\t");
        } else if b == b'\n' {
            out.push_str("\\n");
        } else if b == b'\r' {
            out.push_str("\\r");
        } else if (0x20..0x7f).contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "\\x{b:02x}");
        }
    }
    out.push(quote);
}

#[cfg(test)]
mod tests {
    use super::{parse_py_expr, parse_py_expr_stmt, parse_py_star_annotation, unparse};

    /// The oracle battery. Every `(source, expected)` pair below is
    /// probe-verified:
    // oracle: ast.unparse, python 3.12 / sphinx 9.1.0 / docutils 0.22.4 pin
    // (scratchpad oracle_expr.py; `uv run --python 3.12 --with
    // 'sphinx==9.1.0' --with 'docutils==0.22.4' python -c "import ast;
    // print(ast.unparse(ast.parse('<expr>', mode='eval')))"`).
    const ORACLE: &[(&str, &str)] = &[
        // brief-mandated normalization pins
        ("1+2", "1 + 2"),
        ("dict[str,int]", "dict[str, int]"),
        ("\"x\"", "'x'"),
        ("[1 ,2]", "[1, 2]"),
        ("( 1, )", "(1,)"),
        ("{'a':1}", "{'a': 1}"),
        ("-1", "-1"),
        ("x [ 1 ]", "x[1]"),
        // nesting / typing shapes
        ("dict[str, list[int]]", "dict[str, list[int]]"),
        ("tuple[int, ...]", "tuple[int, ...]"),
        ("Callable[[int], str]", "Callable[[int], str]"),
        ("int | None", "int | None"),
        ("Optional[int] | str", "Optional[int] | str"),
        ("a.b.c.d", "a.b.c.d"),
        ("a . b . c", "a.b.c"),
        // integers
        ("0xFF", "255"),
        ("1_000", "1000"),
        ("0o777", "511"),
        ("0b1010", "10"),
        ("0x_FF", "255"),
        ("000", "0"),
        ("0_0", "0"),
        ("0x0", "0"),
        ("999999999999999999999999", "999999999999999999999999"),
        ("0xFFFFFFFFFFFFFFFFFFFF", "1208925819614629174706175"),
        // floats (repr(float) semantics)
        ("1e-3", "0.001"),
        ("1E3", "1000.0"),
        ("1e+3", "1000.0"),
        (".5", "0.5"),
        ("5.", "5.0"),
        ("5.0", "5.0"),
        ("0.1", "0.1"),
        ("2.675", "2.675"),
        ("10_000_000.0", "10000000.0"),
        ("1e15", "1000000000000000.0"),
        ("1e16", "1e+16"),
        ("1e300", "1e+300"),
        ("1e-4", "0.0001"),
        ("0.0001", "0.0001"),
        ("0.00001", "1e-05"),
        ("1e-5", "1e-05"),
        ("1.5e10", "15000000000.0"),
        ("1e400", "1e309"),
        ("0e0", "0.0"),
        ("00.0", "0.0"),
        ("9007199254740993.0", "9007199254740992.0"),
        ("123456789123456789.0", "1.2345678912345678e+17"),
        ("1.7976931348623157e308", "1.7976931348623157e+308"),
        ("5e-324", "5e-324"),
        ("-1.0", "-1.0"),
        ("-0.0", "-0.0"),
        // strings and bytes
        ("\"a'b\"", "\"a'b\""),
        ("'a\"b'", "'a\"b'"),
        ("'both\\'\\\"'", "'both\\'\"'"),
        ("'don\\'t \"quote\"'", "'don\\'t \"quote\"'"),
        ("'ab' 'cd'", "'abcd'"),
        ("'x' 'y' 'z'", "'xyz'"),
        ("b'x'", "b'x'"),
        ("B\"y\"", "b'y'"),
        ("u'x'", "u'x'"),
        ("u''", "u''"),
        ("b''", "b''"),
        ("''", "''"),
        ("u'a' 'b'", "u'ab'"),
        ("'a' u'b'", "'ab'"),
        ("b'a' b'b'", "b'ab'"),
        ("r'\\d'", "'\\\\d'"),
        ("rb'\\x00'", "b'\\\\x00'"),
        ("'\\n'", "'\\n'"),
        ("'\\x41'", "'A'"),
        ("'\\t\\\\'", "'\\t\\\\'"),
        ("'\\''", "\"'\""),
        ("\"\\\"\"", "'\"'"),
        ("'\\u00e9'", "'é'"),
        ("'é'", "'é'"),
        ("'\\x85'", "'\\x85'"),
        ("'\\xa0'", "'\\xa0'"),
        ("'\\u2028'", "'\\u2028'"),
        ("'\u{202f}'", "'\\u202f'"),
        ("'\\x7f'", "'\\x7f'"),
        ("'\\x00'", "'\\x00'"),
        ("'\\v'", "'\\x0b'"),
        ("'\\a'", "'\\x07'"),
        ("'\\401'", "'ā'"),
        ("'\\x9F'", "'\\x9f'"),
        ("b'\\x00\\xff'", "b'\\x00\\xff'"),
        ("b'a\\'b'", "b\"a'b\""),
        ("b'\\''", "b\"'\""),
        ("b'\"'", "b'\"'"),
        ("b'both\\'\\\"'", "b'both\\'\"'"),
        ("b'\\401'", "b'\\x01'"),
        // triple-quoted literals
        ("'''x'''", "'x'"),
        ("\"\"\"a'b\"\"\"", "\"a'b\""),
        ("'''a\nb'''", "'a\\nb'"),
        ("'''it's'''", "\"it's\""),
        ("u'''k'''", "u'k'"),
        ("b'''q'''", "b'q'"),
        ("'''trip''' 'le'", "'triple'"),
        // containers
        ("[]", "[]"),
        ("{}", "{}"),
        ("()", "()"),
        ("(1,)", "(1,)"),
        ("(1, 2)", "(1, 2)"),
        ("1, 2", "(1, 2)"),
        ("1,", "(1,)"),
        ("{1, 2}", "{1, 2}"),
        ("{'a': 1, 'b': 2}", "{'a': 1, 'b': 2}"),
        ("{**base, 'a': 1}", "{**base, 'a': 1}"),
        ("{**a}", "{**a}"),
        ("{**a | b}", "{**a | b}"),
        ("{1: (2, 3)}", "{1: (2, 3)}"),
        ("[*a, 1]", "[*a, 1]"),
        ("(*a, 1)", "(*a, 1)"),
        ("{*a, 1}", "{*a, 1}"),
        ("[[1, 2], [3]]", "[[1, 2], [3]]"),
        ("[(1, 2)]", "[(1, 2)]"),
        ("((1, 2),)", "((1, 2),)"),
        // subscripts
        ("x[1,]", "x[1,]"),
        ("x[()]", "x[()]"),
        ("x[1, 2]", "x[1, 2]"),
        ("x[(1, 2)]", "x[1, 2]"),
        ("x[*a]", "x[*a,]"),
        ("x[a, *b]", "x[a, *b]"),
        ("x[a, *b, c]", "x[a, *b, c]"),
        ("x[a][b]", "x[a][b]"),
        ("x[-1]", "x[-1]"),
        ("x[...]", "x[...]"),
        // calls
        ("f()", "f()"),
        ("f(1, x=2)", "f(1, x=2)"),
        ("f(*args)", "f(*args)"),
        ("f(x=1)", "f(x=1)"),
        ("f(1, *a, x=2)", "f(1, *a, x=2)"),
        ("f(1, x=2, *a, y=3)", "f(1, *a, x=2, y=3)"),
        ("f(a)(b)[c].d", "f(a)(b)[c].d"),
        ("f(a, *b)(c)", "f(a, *b)(c)"),
        ("(a + b).method(x)", "(a + b).method(x)"),
        ("(1).bit_length()", "1 .bit_length()"),
        ("f(g(x), y=h(z))", "f(g(x), y=h(z))"),
        ("f((1, 2))", "f((1, 2))"),
        ("f(-1)", "f(-1)"),
        // constants
        ("...", "..."),
        ("True", "True"),
        ("False", "False"),
        ("None", "None"),
        ("True | False", "True | False"),
        // operators / precedence (ports of ast._Unparser paren rules)
        ("2**8", "2 ** 8"),
        ("2 ** -1", "2 ** (-1)"),
        ("-2 ** 2", "-2 ** 2"),
        ("(-2) ** 2", "(-2) ** 2"),
        ("-(2 ** 2)", "-2 ** 2"),
        ("-x ** 2", "-x ** 2"),
        ("2 ** -x ** 3", "2 ** (-x ** 3)"),
        ("a ** b ** c", "a ** b ** c"),
        ("(a ** b) ** c", "(a ** b) ** c"),
        ("(1 + 2) * 3", "(1 + 2) * 3"),
        ("1 + (2 * 3)", "1 + 2 * 3"),
        ("1 + 2 * 3", "1 + 2 * 3"),
        ("a | b | c", "a | b | c"),
        ("a | (b | c)", "a | (b | c)"),
        ("a + b | c & d", "a + b | c & d"),
        ("a << 2 >> 1", "a << 2 >> 1"),
        ("x & y ^ z", "x & y ^ z"),
        ("x @ y", "x @ y"),
        ("(a @ b) @ c", "a @ b @ c"),
        ("a @ (b @ c)", "a @ (b @ c)"),
        ("x // y % z", "x // y % z"),
        ("a / b * c", "a / b * c"),
        ("a - (b - c)", "a - (b - c)"),
        ("(a - b) - c", "a - b - c"),
        ("a % (b % c)", "a % (b % c)"),
        ("~x", "~x"),
        ("+x", "+x"),
        ("not x", "not x"),
        ("not not x", "not not x"),
        ("- -x", "--x"),
        ("-(-x)", "--x"),
        ("-(1)", "-1"),
        ("-(a + b)", "-(a + b)"),
        ("~x | +y", "~x | +y"),
        ("not a | b", "not a | b"),
        ("(a | b)[c]", "(a | b)[c]"),
        ("-x[0]", "-x[0]"),
        ("(-x)[0]", "(-x)[0]"),
        // `ast.BoolOp`. `_Unparser.visit_BoolOp` bumps its precedence
        // CUMULATIVELY across the values, so the parenthesization is
        // asymmetric: `a and b or c` needs none, `a or b and c` does.
        ("a or b", "a or b"),
        ("a and b or c", "a and b or c"),
        ("a or b and c", "a or (b and c)"),
        ("(a or b) and c", "(a or b) and c"),
        ("not a or b", "not a or b"),
        ("a or not b", "a or not b"),
        ("a | b or c", "a | b or c"),
        ("a or b or c", "a or b or c"),
        ("a and b and c", "a and b and c"),
        ("(a or b) or c", "(a or b) or c"),
        ("a or (b or c)", "a or (b or c)"),
        ("-a or b", "-a or b"),
        ("f(a or b)", "f(a or b)"),
        ("[a or b]", "[a or b]"),
        ("a or b, c", "(a or b, c)"),
        ("x[a or b]", "x[a or b]"),
        ("(a or b).c", "(a or b).c"),
        ("(a and b)(c)", "(a and b)(c)"),
        ("not (a or b)", "not (a or b)"),
        ("a or b ** c", "a or b ** c"),
        ("(a or b)[0]", "(a or b)[0]"),
        ("{a or b}", "{a or b}"),
        ("{(a or b): 1}", "{a or b: 1}"),
        ("{'k': a or b}", "{'k': a or b}"),
        ("a or b | c", "a or b | c"),
        ("(a and b) or (c and d)", "a and b or (c and d)"),
        ("a and (b or c)", "a and (b or c)"),
        ("a or b or c or d", "a or b or c or d"),
        ("a and b or c and d", "a and b or (c and d)"),
        ("f(*(a or b))", "f(*(a or b))"),
        ("x[a or b, c]", "x[a or b, c]"),
        ("a  or   b", "a or b"),
        ("not not a or b", "not not a or b"),
        ("True or False", "True or False"),
        ("a or 1 or 'x'", "a or 1 or 'x'"),
    ];

    /// Inputs that must be `Err` — never a panic. Some are invalid Python;
    /// the rest are valid Python outside the supported subset (the brief's
    /// explicit exclusions), where `Err` routes callers into Sphinx's
    /// fallback paths.
    const ERR_CASES: &[&str] = &[
        // brief-pinned exclusions
        "lambda: 1",
        "x if y else z",
        "(x := 1)",
        "",
        "f(a",
        "1 +",
        "[1,, 2]",
        "await x",
        "f'{x}'",
        "x for x in y",
        "(x for x in y)",
        "[x for x in y]",
        // outside the enum's subset (valid Python, deliberate Err)
        "x < y",
        "x == y",
        "x[1:2]",
        "x[:]",
        "f(**kw)",
        "1j",
        "'\\N{DASH}'",
        "yield x",
        "1 if True else 2",
        "1if True else 2",
        // plain syntax errors (Python errs too)
        "*x",
        "(*x)",
        "*a, b",
        "b, *a",
        "*a,",
        "x = 1",
        "01",
        "09",
        "1_",
        "1__0",
        "0x",
        "0b2",
        "def f(): pass",
        "x²",
        "'unterminated",
        "b'unterminated",
        "f(x=1, 2)",
        "1 2",
        "x[]",
        "{*a: 1}",
        "'a' b'b'",
        "b'é'",
        "x.if",
        "f(if=1)",
        "x + not y",
        ")",
        "((((",
        "\\",
        "..",
        "@x",
        "x;",
        "x, = y",
        "'\\ud800'",
    ];

    #[test]
    fn oracle_battery_round_trips_ast_unparse() {
        for (src, want) in ORACLE {
            let parsed = parse_py_expr(src)
                .unwrap_or_else(|e| panic!("parse_py_expr({src:?}) unexpectedly failed: {e}"));
            let got = unparse(&parsed);
            assert_eq!(&got, want, "unparse mismatch for source {src:?}");
        }
    }

    /// `unparse` output must itself reparse to the same rendering —
    /// `ast.unparse(ast.parse(ast.unparse(t)))` is a fixed point.
    #[test]
    fn unparse_is_idempotent_over_reparse() {
        for (src, _) in ORACLE {
            let first = unparse(&parse_py_expr(src).unwrap());
            let reparsed = parse_py_expr(&first)
                .unwrap_or_else(|e| panic!("unparse output {first:?} must reparse: {e}"));
            assert_eq!(unparse(&reparsed), first, "not idempotent for {src:?}");
        }
    }

    /// Exec mode is what `_parse_annotation` uses, so a top-level starred
    /// expression is a legal `Expr` statement there and a blank source is
    /// `Module(body=[])`.
    ///
    // oracle (pinned toolchain, scratchpad A/p1.py + A/p3.py):
    //   ast.parse('*Ts')       -> Module(body=[Expr(Starred(Name('Ts')))])
    //   ast.parse('*a, b')     -> Module(body=[Expr(Tuple([Starred(a), b]))])
    //   ast.parse('')/' '/'\n' -> Module(body=[])
    //   ast.parse('  int')     -> IndentationError: unexpected indent
    //   ast.parse('\x0cint')   -> Module(body=[Expr(Name('int'))])
    #[test]
    fn exec_mode_accepts_top_level_starred_and_empty_sources() {
        for (src, want) in [
            ("*Ts", "*Ts"),
            ("* Ts", "*Ts"),
            ("*tuple[int, ...]", "*tuple[int, ...]"),
            ("*a.b", "*a.b"),
            ("*a | b", "*a | b"),
            ("*-a", "*-a"),
            ("*a, b", "(*a, b)"),
            ("a, *b", "(a, *b)"),
            ("*a,", "(*a,)"),
            ("int", "int"),
            ("\nint", "int"),
            ("  \nint", "int"),
            ("\u{c}int", "int"),
            ("int ", "int"),
        ] {
            let parsed = parse_py_expr_stmt(src)
                .unwrap_or_else(|e| panic!("parse_py_expr_stmt({src:?}) failed: {e}"))
                .unwrap_or_else(|| panic!("parse_py_expr_stmt({src:?}) yielded no statement"));
            assert_eq!(unparse(&parsed), want, "exec-mode unparse for {src:?}");
        }
        for src in ["", " ", "  ", "\n", "\u{c}", "\t\n \n"] {
            assert_eq!(
                parse_py_expr_stmt(src),
                Ok(None),
                "{src:?} must be Module(body=[])"
            );
        }
        // IndentationError is a SyntaxError subclass, so the caller keeps
        // the unstripped text — never a silently stripped parse.
        for src in ["  int", " int", "\tint", " \u{c} int", "int\n  str"] {
            assert!(
                parse_py_expr_stmt(src).is_err(),
                "{src:?} must raise IndentationError"
            );
        }
        // Eval mode still rejects every top-level star (ledger ruling).
        for src in ["*Ts", "*a, b", "*a,", "a, *b"] {
            assert!(parse_py_expr(src).is_err(), "eval mode must reject {src:?}");
        }
    }

    /// CPython's `star_annotation` production: one leading `*`, never a
    /// starred tuple element.
    ///
    // oracle: ast.parse('def f(*args: *Ts): pass') is legal;
    // 'def f(x: *Ts)' and 'def f(**k: *Ts)' are SyntaxErrors, and
    // signature_from_str('(*args: *tuple[int, ...])') gives the annotation
    // string '*tuple[int, ...]' (scratchpad A/p2.py).
    #[test]
    fn star_annotation_mode_takes_one_leading_star_only() {
        for (src, want) in [
            ("*Ts", "*Ts"),
            ("*tuple[int, ...]", "*tuple[int, ...]"),
            ("int", "int"),
            ("int | None", "int | None"),
        ] {
            let parsed = parse_py_star_annotation(src)
                .unwrap_or_else(|e| panic!("parse_py_star_annotation({src:?}) failed: {e}"));
            assert_eq!(unparse(&parsed), want);
        }
        for src in ["*a, b", "a, *b", "*a,", ""] {
            assert!(
                parse_py_star_annotation(src).is_err(),
                "star_annotation must reject {src:?}"
            );
        }
    }

    #[test]
    fn unsupported_and_invalid_inputs_are_errors_not_panics() {
        for src in ERR_CASES {
            assert!(
                parse_py_expr(src).is_err(),
                "expected Err for source {src:?}"
            );
        }
    }

    /// Pathologically nested input must hit the depth limit (Err), not
    /// overflow the stack. The right-extending shapes stress the recursion
    /// guards; the left-extending shapes (trailer loops, binop folds)
    /// stress the `charge_node` budget — without it they would return an
    /// `Ok` tree whose recursive `unparse`/`Drop` aborts the process.
    #[test]
    fn deep_nesting_is_an_error_not_a_stack_overflow() {
        for (open, close) in [("(", ")"), ("[", "]"), ("-", "")] {
            let src = format!("{}1{}", open.repeat(5000), close.repeat(5000));
            assert!(parse_py_expr(&src).is_err(), "expected Err for deep {open}");
        }
        for src in [
            format!("a{}", ".b".repeat(5000)),
            format!("x{}", "[1]".repeat(5000)),
            format!("f{}", "()".repeat(5000)),
            format!("1{}", "+1".repeat(5000)),
        ] {
            assert!(
                parse_py_expr(&src).is_err(),
                "expected Err for left-deep input starting {:?}",
                &src[..8]
            );
        }
    }

    /// The stored `quote` on a str constant agrees with what `unparse`
    /// renders (repr's quote-selection rule).
    #[test]
    fn stored_quote_matches_rendered_quote() {
        for (src, quote) in [("'x'", '\''), ("\"a'b\"", '"'), ("'a\"b'", '\'')] {
            let parsed = parse_py_expr(src).unwrap();
            match parsed {
                super::PyExpr::Constant(super::PyConst::Str { quote: q, .. }) => {
                    assert_eq!(q, quote, "quote for {src:?}");
                }
                other => panic!("expected Str constant for {src:?}, got {other:?}"),
            }
        }
    }

    mod proptests {
        use super::super::parse_py_expr;
        use proptest::prelude::*;

        proptest! {
            #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

            /// Totality: arbitrary input never panics (T16 extends this).
            #[test]
            fn parse_never_panics_on_arbitrary_input(s in "\\PC*") {
                let _ = parse_py_expr(&s);
            }

            /// Expression-shaped fragments never panic, and successful
            /// parses unparse to a reparse fixed point.
            #[test]
            fn parse_never_panics_on_expr_shaped_input(
                s in proptest::collection::vec(
                    prop_oneof![
                        Just("x".to_string()),
                        Just("1".to_string()),
                        Just("1.5".to_string()),
                        Just("'s'".to_string()),
                        Just("b'q'".to_string()),
                        Just("(".to_string()),
                        Just(")".to_string()),
                        Just("[".to_string()),
                        Just("]".to_string()),
                        Just("{".to_string()),
                        Just("}".to_string()),
                        Just(",".to_string()),
                        Just(":".to_string()),
                        Just(".".to_string()),
                        Just("...".to_string()),
                        Just("**".to_string()),
                        Just("*".to_string()),
                        Just("|".to_string()),
                        Just("-".to_string()),
                        Just("=".to_string()),
                        Just("not ".to_string()),
                        Just(" ".to_string()),
                    ],
                    0..24,
                ).prop_map(|v| v.concat())
            ) {
                if let Ok(parsed) = parse_py_expr(&s) {
                    let out = super::super::unparse(&parsed);
                    let reparsed = parse_py_expr(&out).expect("unparse output reparses");
                    prop_assert_eq!(super::super::unparse(&reparsed), out);
                }
            }
        }
    }
}
