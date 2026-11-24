//! Sphinx's build tags and the conditions `.. only::` names
//! (`SP/sphinx/util/tags.py`).
//!
//! Sphinx evaluates a condition with Jinja2: its lexer in the `variable`
//! state (`SP/jinja2/lexer.py`, Jinja2 3.1.6) and its expression parser
//! with the comparison level replaced (`BooleanParser.parse_compare`), so
//! that a condition is names, `and`, `or`, `not`, `if`/`else` and
//! parentheses. What a condition Sphinx cannot evaluate prints is the text
//! of the exception it raises, which the only directive's warning carries
//! ([`super::post`]); the port keeps those texts. Jinja2 lexes lazily, one
//! token ahead of the parser, so an error in the text after the point
//! where the parser stops is never raised: the port's lexer runs the same
//! way.
//!
//! Exact for ASCII. A non-ASCII name is read with Rust's alphanumeric and
//! alphabetic properties and the combining-mark table where Jinja2 reads
//! `\w` plus its list of other identifier characters and Python's
//! `str.isidentifier()`; the two differ only for rare characters, such as
//! superscript digits. A string's `\N{...}` escape is taken as valid
//! whatever the name.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::char::is_combining_mark;

use crate::utils::{py_isspace, py_repr_str};

/// The build's tags (`app.tags`): the `-t` tags and the builder's own
/// ([`crate::builder::BuilderKind::tags`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tags {
    names: BTreeSet<String>,
}

impl Tags {
    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        Tags {
            names: names.into_iter().collect(),
        }
    }

    /// `Tags.eval_condition` (`SP/sphinx/util/tags.py:66-83`): whether
    /// `expr` holds, a name holding when it is one of the tags. The error
    /// is `str()` of the exception Sphinx raises for an expression it
    /// cannot evaluate: a Jinja2 syntax error (its message, then `  line
    /// N`), `chunk after expression` when the parser stops before the end,
    /// or `invalid node, check parsing` for a constant, or for a
    /// conditional without `else` whose test fails. Sphinx caches each
    /// condition's value, which changes nothing: evaluation is
    /// deterministic.
    pub fn eval_condition(&self, expr: &str) -> Result<bool, String> {
        let mut parser = Parser {
            stream: TokenStream::new(expr).map_err(SyntaxError::into_text)?,
        };
        let parsed = parser
            .parse_expression(EVAL_CONDITION_FRAME + 1)
            .map_err(SyntaxError::into_text)?;
        if parser.stream.current.kind != TokenKind::Eof {
            return Err("chunk after expression".to_string());
        }
        self.eval(&parsed, EVAL_CONDITION_FRAME + 1)
    }

    /// `Tags._eval_node` (`:85-101`): `and` and `or` short-circuit, a
    /// name is a tag test, and anything else (a constant, a missing
    /// `else`) is an invalid node. `depth` is the Python frame the call
    /// runs in ([`RECURSION_LIMIT`]).
    fn eval(&self, expr: &Expr, depth: usize) -> Result<bool, String> {
        if depth > RECURSION_LIMIT {
            return Err(RECURSION_ERROR.to_string());
        }
        let deeper = depth + 1;
        match expr {
            Expr::Cond { test, then, orelse } => {
                if self.eval(test, deeper)? {
                    self.eval(then, deeper)
                } else {
                    match orelse {
                        Some(orelse) => self.eval(orelse, deeper),
                        None => Err(INVALID_NODE.to_string()),
                    }
                }
            }
            Expr::And(left, right) => Ok(self.eval(left, deeper)? && self.eval(right, deeper)?),
            Expr::Or(left, right) => Ok(self.eval(left, deeper)? || self.eval(right, deeper)?),
            Expr::Not(operand) => Ok(!self.eval(operand, deeper)?),
            Expr::Name(name) => Ok(self.names.contains(name)),
            Expr::Const => Err(INVALID_NODE.to_string()),
        }
    }
}

const INVALID_NODE: &str = "invalid node, check parsing";

/// Python's recursion limit (`sys.getrecursionlimit()`, 1000): a call
/// that would run in a deeper frame raises `RecursionError`, which a
/// condition too deep for Jinja2's recursive parser or Sphinx's recursive
/// evaluator raises, and the only directive warns with its text
/// ([`RECURSION_ERROR`]). The port counts the frames those calls would
/// stand in, so it fails where Sphinx does, and recurses at most as deep.
const RECURSION_LIMIT: usize = 1000;

/// The frame `Tags.eval_condition` runs in when Sphinx 9.1.0's
/// `OnlyNodeTransform` calls it, under `-b html` and `-b dummy` alike
/// (probed: the thresholds in
/// `a_condition_past_pythons_recursion_limit_fails_as_sphinx_does`). The
/// toctree's call stands elsewhere in Sphinx's stack; it is counted the
/// same.
const EVAL_CONDITION_FRAME: usize = 21;

/// The frames `next(stream)` adds while the lexer reads the next token:
/// `TokenStream.__next__`, `Lexer.wrap` and `Lexer.tokeniter`, the last
/// two generators it resumes.
const LEXER_FRAMES: usize = 3;

/// `str()` of Python 3.12's `RecursionError`.
const RECURSION_ERROR: &str = "maximum recursion depth exceeded";

/// `jinja2.TemplateSyntaxError(message, lineno)`, the error lexing or
/// parsing a condition raises.
struct SyntaxError {
    message: String,
    /// `None` for Python's `RecursionError`, whose text has no line.
    lineno: Option<usize>,
}

impl SyntaxError {
    fn new(message: impl Into<String>, lineno: usize) -> SyntaxError {
        SyntaxError {
            message: message.into(),
            lineno: Some(lineno),
        }
    }

    /// The `RecursionError` a call past [`RECURSION_LIMIT`] raises.
    fn recursion() -> SyntaxError {
        SyntaxError {
            message: RECURSION_ERROR.to_string(),
            lineno: None,
        }
    }

    /// Fails with [`Self::recursion`] when a frame at `depth` would pass
    /// [`RECURSION_LIMIT`].
    fn check_depth(depth: usize) -> Result<(), SyntaxError> {
        if depth > RECURSION_LIMIT {
            Err(SyntaxError::recursion())
        } else {
            Ok(())
        }
    }

    /// `str()` of the error: the message, then `  line <lineno>` on a line
    /// of its own (the condition has no template name and no source).
    fn into_text(self) -> String {
        match self.lineno {
            Some(lineno) => format!("{}\n  line {}", self.message, lineno),
            None => self.message,
        }
    }
}

/// A token of Jinja2's `variable` state, as the parser sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenKind {
    Name(String),
    /// An operator, `(` and `)` included, spelled as written.
    Operator(&'static str),
    Integer,
    Float,
    String,
    /// `}}` or `-}}`: the end of the variable block.
    VariableEnd,
    Eof,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    lineno: usize,
}

impl Token {
    /// `jinja2.lexer.describe_token`: a name's value, an operator's
    /// spelling, or the token type's description.
    fn describe(&self) -> &str {
        match &self.kind {
            TokenKind::Name(name) => name,
            TokenKind::Operator(operator) => operator,
            TokenKind::Integer => "integer",
            TokenKind::Float => "float",
            TokenKind::String => "string",
            TokenKind::VariableEnd => "end of print statement",
            TokenKind::Eof => "end of template",
        }
    }

    fn is_name(&self, keyword: &str) -> bool {
        matches!(&self.kind, TokenKind::Name(name) if name == keyword)
    }
}

/// Jinja2's operators, longest first, as `operator_re` tries them.
const OPERATORS: [&str; 26] = [
    "//", "**", "==", "!=", ">=", "<=", "+", "-", "/", "*", "%", "~", "[", "]", "(", ")", "{", "}",
    ">", "<", "=", ".", ":", "|", ",", ";",
];

/// `float_re` (`SP/jinja2/lexer.py:44-56`), anchored, less its
/// `(?<!\.)`, which [`Lexer::next_token`] checks.
static FLOAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?i:(?:\d+_)*\d+(?:(?:\.(?:\d+_)*\d+)?e[+\-]?(?:\d+_)*\d+|\.(?:\d+_)*\d+))")
        .expect("the float pattern compiles")
});

/// `integer_re` (`:30-43`), anchored.
static INTEGER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?i:0b(?:_?[0-1])+|0o(?:_?[0-7])+|0x(?:_?[\da-f])+|[1-9](?:_?\d)*|0(?:_?0)*)")
        .expect("the integer pattern compiles")
});

/// The characters besides `\w` and the combining marks that Jinja2's name
/// pattern takes (`SP/jinja2/_identifier.py`).
const NAME_EXTRAS: [char; 15] = [
    '\u{b7}', '\u{387}', '\u{1cf2}', '\u{1cf3}', '\u{203f}', '\u{2040}', '\u{2054}', '\u{2118}',
    '\u{212e}', '\u{fe33}', '\u{fe34}', '\u{fe4d}', '\u{fe4e}', '\u{fe4f}', '\u{ff3f}',
];

/// A character of Jinja2's name pattern, `[\w<extras>]`.
fn is_name_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric() || is_combining_mark(c) || NAME_EXTRAS.contains(&c)
}

/// Python's `str.isidentifier()` over a run of name characters: the first
/// one must start an identifier (a letter, `_`, or one of the two symbols
/// Unicode lets start one).
fn is_identifier(name: &str) -> bool {
    name.chars().next().is_some_and(|first| {
        first == '_'
            || (first.is_alphabetic() && !is_combining_mark(first))
            || matches!(first, '\u{2118}' | '\u{212e}')
    })
}

/// Jinja2's lexer in the `variable` state (`Lexer.tokeniter`,
/// `SP/jinja2/lexer.py:600-800`, with `Lexer.wrap`'s checks), producing
/// one token per call.
struct Lexer {
    /// The condition with its line breaks normalized to `\n` and a final
    /// one dropped (`keep_trailing_newline = False`).
    source: String,
    /// Byte offset of the next token.
    pos: usize,
    lineno: usize,
    /// The closing brackets the open ones expect, innermost last.
    balancing: Vec<&'static str>,
    /// The variable block ended: Jinja2 is back in its root state, which
    /// the parser, stopped at the block's end, never reads.
    ended: bool,
}

impl Lexer {
    fn new(expr: &str) -> Lexer {
        let mut lines: Vec<&str> = Vec::new();
        let mut rest = expr;
        while let Some(at) = rest.find(['\r', '\n']) {
            lines.push(&rest[..at]);
            let skip = if rest[at..].starts_with("\r\n") { 2 } else { 1 };
            rest = &rest[at + skip..];
        }
        lines.push(rest);
        if lines.last() == Some(&"") {
            lines.pop();
        }
        Lexer {
            source: lines.join("\n"),
            pos: 0,
            lineno: 1,
            balancing: Vec::new(),
            ended: false,
        }
    }

    /// The next token, `None` at the end of the condition, or the error
    /// Jinja2 raises where its rules match nothing or a bracket or name is
    /// invalid.
    fn next_token(&mut self) -> Result<Option<Token>, SyntaxError> {
        loop {
            let rest = &self.source[self.pos..];
            if self.ended || rest.is_empty() {
                return Ok(None);
            }
            let lineno = self.lineno;
            let token = |kind| Ok(Some(Token { kind, lineno }));
            // The block's end, which Jinja2 matches only while every
            // bracket is closed: `-}}` with the whitespace after it, or
            // `}}`.
            if self.balancing.is_empty() {
                let end = if let Some(after) = rest.strip_prefix("-}}") {
                    let blank = after.len() - after.trim_start_matches(py_isspace).len();
                    Some(3 + blank)
                } else {
                    rest.starts_with("}}").then_some(2)
                };
                if let Some(len) = end {
                    self.advance(len);
                    self.ended = true;
                    return token(TokenKind::VariableEnd);
                }
            }
            let blank = rest.len() - rest.trim_start_matches(py_isspace).len();
            if blank > 0 {
                self.advance(blank);
                continue;
            }
            let after_dot = self.source[..self.pos].ends_with('.');
            if let Some(found) = FLOAT.find(rest).filter(|_| !after_dot) {
                self.advance(found.end());
                return token(TokenKind::Float);
            }
            if let Some(found) = INTEGER.find(rest) {
                self.advance(found.end());
                return token(TokenKind::Integer);
            }
            let name_len: usize = rest
                .chars()
                .take_while(|&c| is_name_char(c))
                .map(char::len_utf8)
                .sum();
            if name_len > 0 {
                let name = rest[..name_len].to_string();
                self.advance(name_len);
                if !is_identifier(&name) {
                    return Err(SyntaxError::new("Invalid character in identifier", lineno));
                }
                return token(TokenKind::Name(name));
            }
            if let Some(len) = string_len(rest) {
                let error = unicode_escape_error(&rest[1..len - 1]);
                self.advance(len);
                if let Some(message) = error {
                    return Err(SyntaxError::new(message, lineno));
                }
                return token(TokenKind::String);
            }
            if let Some(operator) = OPERATORS
                .iter()
                .find(|operator| rest.starts_with(**operator))
            {
                self.balance(operator)?;
                self.advance(operator.len());
                return token(TokenKind::Operator(operator));
            }
            let at = self.source[..self.pos].chars().count();
            let char = rest.chars().next().expect("the rest is not empty");
            return Err(SyntaxError::new(
                format!("unexpected char {} at {at}", py_repr_str(&char.to_string())),
                lineno,
            ));
        }
    }

    /// Moves past `len` bytes, counting their line breaks.
    fn advance(&mut self, len: usize) {
        let taken = &self.source[self.pos..self.pos + len];
        self.lineno += taken.matches('\n').count();
        self.pos += len;
    }

    /// Jinja2's bracket balance: an opening bracket expects its closing
    /// one; a closing bracket must close the innermost open one.
    fn balance(&mut self, operator: &'static str) -> Result<(), SyntaxError> {
        let closer = match operator {
            "{" => Some("}"),
            "(" => Some(")"),
            "[" => Some("]"),
            _ => None,
        };
        if let Some(closer) = closer {
            self.balancing.push(closer);
            return Ok(());
        }
        if !matches!(operator, "}" | ")" | "]") {
            return Ok(());
        }
        match self.balancing.pop() {
            None => Err(SyntaxError::new(
                format!("unexpected '{operator}'"),
                self.lineno,
            )),
            Some(expected) if expected != operator => Err(SyntaxError::new(
                format!("unexpected '{operator}', expected '{expected}'"),
                self.lineno,
            )),
            Some(_) => Ok(()),
        }
    }
}

/// The byte length of the quoted string `rest` opens with, quotes
/// included (`string_re`, `SP/jinja2/lexer.py:27-29`): a backslash
/// escapes the character after it, line breaks included.
fn string_len(rest: &str) -> Option<usize> {
    let quote = rest.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let mut chars = rest.char_indices().skip(1);
    while let Some((at, c)) = chars.next() {
        if c == '\\' {
            chars.next()?;
        } else if c == quote {
            return Some(at + 1);
        }
    }
    None
}

/// The error `Lexer.wrap` raises for a string whose escapes Python's
/// `unicode-escape` codec rejects: the codec's message after its last `:`.
/// The codec reads the string after `backslashreplace`, which spells a
/// non-ASCII character as an escape of its own, so after a backslash such
/// a character is an escaped backslash and in a hex escape it is not a
/// digit.
fn unicode_escape_error(body: &str) -> Option<&'static str> {
    let chars: Vec<char> = body.chars().collect();
    let hex_digits = |from: usize, count: usize| {
        chars
            .get(from..from + count)
            .filter(|digits| digits.iter().all(char::is_ascii_hexdigit))
            .map(|digits| {
                let digits: String = digits.iter().collect();
                u32::from_str_radix(&digits, 16).expect("hex digits")
            })
    };
    let mut at = 0;
    while at < chars.len() {
        if chars[at] != '\\' {
            at += 1;
            continue;
        }
        // The string pattern pairs every backslash with what follows it.
        let Some(&escape) = chars.get(at + 1) else {
            break;
        };
        at += 2;
        match escape {
            'x' => match hex_digits(at, 2) {
                Some(_) => at += 2,
                None => return Some("truncated \\xXX escape"),
            },
            'u' => match hex_digits(at, 4) {
                Some(_) => at += 4,
                None => return Some("truncated \\uXXXX escape"),
            },
            'U' => match hex_digits(at, 8) {
                Some(code) if code > 0x10ffff => return Some("illegal Unicode character"),
                Some(_) => at += 8,
                None => return Some("truncated \\UXXXXXXXX escape"),
            },
            'N' => {
                let close = chars[at..].iter().position(|&c| c == '}');
                match close {
                    Some(close) if chars.get(at) == Some(&'{') && close > 1 => at += close + 1,
                    _ => return Some("malformed \\N character escape"),
                }
            }
            _ => {}
        }
    }
    None
}

/// Jinja2's `TokenStream` (`SP/jinja2/lexer.py:290-380`): the current
/// token, one ahead of what the parser has taken; the next is lexed only
/// when the parser moves on.
struct TokenStream {
    lexer: Lexer,
    current: Token,
}

impl TokenStream {
    /// The stream at its first token, which Jinja2 lexes as it makes the
    /// stream: the end of the template at line 1 for an empty condition.
    fn new(expr: &str) -> Result<TokenStream, SyntaxError> {
        let mut lexer = Lexer::new(expr);
        let current = lexer.next_token()?.unwrap_or(Token {
            kind: TokenKind::Eof,
            lineno: 1,
        });
        Ok(TokenStream { lexer, current })
    }

    /// `next(stream)`: past the end, the end of the template, at the line
    /// of the token before it.
    fn advance(&mut self) -> Result<(), SyntaxError> {
        if self.current.kind == TokenKind::Eof {
            return Ok(());
        }
        self.current = match self.lexer.next_token()? {
            Some(token) => token,
            None => Token {
                kind: TokenKind::Eof,
                lineno: self.current.lineno,
            },
        };
        Ok(())
    }

    /// `stream.skip_if('name:<keyword>')`.
    fn skip_name(&mut self, keyword: &str) -> Result<bool, SyntaxError> {
        if !self.current.is_name(keyword) {
            return Ok(false);
        }
        self.advance()?;
        Ok(true)
    }

    /// `stream.expect('rparen')`.
    fn expect_rparen(&mut self) -> Result<(), SyntaxError> {
        let current = &self.current;
        match &current.kind {
            TokenKind::Operator(")") => self.advance(),
            TokenKind::Eof => Err(SyntaxError::new(
                "unexpected end of template, expected ')'.",
                current.lineno,
            )),
            _ => Err(SyntaxError::new(
                format!("expected token ')', got '{}'", current.describe()),
                current.lineno,
            )),
        }
    }
}

/// The nodes a condition parses into.
enum Expr {
    /// `then if test else orelse`, `orelse` absent without `else`.
    Cond {
        test: Box<Expr>,
        then: Box<Expr>,
        orelse: Option<Box<Expr>>,
    },
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Name(String),
    /// `true`, `false` or `none`, in either case: a `Const`, which the
    /// evaluator rejects.
    Const,
}

/// Dropped without recursion: a chain of `and`s or `or`s nests one node a
/// link however long it is, deeper than any stack.
impl Drop for Expr {
    fn drop(&mut self) {
        let mut pending: Vec<Expr> = Vec::new();
        self.take_children(&mut pending);
        while let Some(mut node) = pending.pop() {
            node.take_children(&mut pending);
        }
    }
}

impl Expr {
    /// Moves the node's children out onto `into`, leaving it a leaf.
    fn take_children(&mut self, into: &mut Vec<Expr>) {
        let mut take = |child: &mut Box<Expr>| {
            into.push(std::mem::replace(child.as_mut(), Expr::Const));
        };
        match self {
            Expr::Cond { test, then, orelse } => {
                take(test);
                take(then);
                if let Some(orelse) = orelse {
                    take(orelse);
                }
            }
            Expr::And(left, right) | Expr::Or(left, right) => {
                take(left);
                take(right);
            }
            Expr::Not(operand) => take(operand),
            Expr::Name(_) | Expr::Const => {}
        }
    }
}

/// Jinja2's `Parser.parse_expression` (`SP/jinja2/parser.py:512-558`)
/// with Sphinx's `BooleanParser.parse_compare`.
struct Parser {
    stream: TokenStream,
}

/// Each parse method takes the Python frame its Jinja2 counterpart runs
/// in, `depth`, and fails as a call past [`RECURSION_LIMIT`] does: on
/// entry, and before reading a token, which `next(stream)` reads
/// [`LEXER_FRAMES`] deeper, or two deeper again through `skip_if`'s
/// `next_if` (`SP/jinja2/lexer.py:381-394`) and one through `expect`.
impl Parser {
    fn parse_expression(&mut self, depth: usize) -> Result<Expr, SyntaxError> {
        SyntaxError::check_depth(depth)?;
        self.parse_condexpr(depth + 1)
    }

    /// `stream.skip_if('name:<keyword>')`, from a method running in frame
    /// `depth`: `skip_if`, `next_if` and `Token.test` above it, and the
    /// lexer above `next_if` when it reads on.
    fn skip_name(&mut self, keyword: &str, depth: usize) -> Result<bool, SyntaxError> {
        SyntaxError::check_depth(depth + 3)?;
        if self.stream.current.is_name(keyword) {
            SyntaxError::check_depth(depth + 2 + LEXER_FRAMES)?;
        }
        self.stream.skip_name(keyword)
    }

    fn parse_condexpr(&mut self, depth: usize) -> Result<Expr, SyntaxError> {
        SyntaxError::check_depth(depth)?;
        let mut expr = self.parse_or(depth + 1)?;
        while self.skip_name("if", depth)? {
            let test = self.parse_or(depth + 1)?;
            let orelse = if self.skip_name("else", depth)? {
                Some(Box::new(self.parse_condexpr(depth + 1)?))
            } else {
                None
            };
            expr = Expr::Cond {
                test: Box::new(test),
                then: Box::new(expr),
                orelse,
            };
        }
        Ok(expr)
    }

    fn parse_or(&mut self, depth: usize) -> Result<Expr, SyntaxError> {
        SyntaxError::check_depth(depth)?;
        let mut left = self.parse_and(depth + 1)?;
        while self.skip_name("or", depth)? {
            left = Expr::Or(Box::new(left), Box::new(self.parse_and(depth + 1)?));
        }
        Ok(left)
    }

    fn parse_and(&mut self, depth: usize) -> Result<Expr, SyntaxError> {
        SyntaxError::check_depth(depth)?;
        let mut left = self.parse_not(depth + 1)?;
        while self.skip_name("and", depth)? {
            left = Expr::And(Box::new(left), Box::new(self.parse_not(depth + 1)?));
        }
        Ok(left)
    }

    /// `parse_not` tests the token itself and reads on with `next(stream)`.
    fn parse_not(&mut self, depth: usize) -> Result<Expr, SyntaxError> {
        SyntaxError::check_depth(depth)?;
        SyntaxError::check_depth(depth + 1)?;
        if self.stream.current.is_name("not") {
            SyntaxError::check_depth(depth + LEXER_FRAMES)?;
            self.stream.advance()?;
            return Ok(Expr::Not(Box::new(self.parse_not(depth + 1)?)));
        }
        self.parse_compare(depth + 1)
    }

    /// `BooleanParser.parse_compare` (`SP/sphinx/util/tags.py:17-38`): a
    /// name (a constant for `true`, `false`, `none` and their capitalized
    /// forms) or a parenthesized expression; anything else fails.
    fn parse_compare(&mut self, depth: usize) -> Result<Expr, SyntaxError> {
        SyntaxError::check_depth(depth)?;
        let token = self.stream.current.clone();
        match &token.kind {
            TokenKind::Name(name) => {
                let expr = match name.as_str() {
                    "true" | "True" | "false" | "False" | "none" | "None" => Expr::Const,
                    _ => Expr::Name(name.clone()),
                };
                SyntaxError::check_depth(depth + LEXER_FRAMES)?;
                self.stream.advance()?;
                Ok(expr)
            }
            TokenKind::Operator("(") => {
                SyntaxError::check_depth(depth + LEXER_FRAMES)?;
                self.stream.advance()?;
                let expr = self.parse_expression(depth + 1)?;
                // `expect` and the lexer above it.
                SyntaxError::check_depth(depth + 1 + LEXER_FRAMES)?;
                self.stream.expect_rparen()?;
                Ok(expr)
            }
            _ => Err(SyntaxError::new(
                format!("unexpected token '{}'", token.describe()),
                token.lineno,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Python's recursion limit (1000 frames) stops Sphinx's parse or its
    /// evaluation of a deep condition with `RecursionError`, whose text the
    /// only directive's warning prints. Probed on Sphinx 9.1.0
    /// (`-b html` and `-b dummy`, an `only` in `index.rst`): 161
    /// parentheses evaluate and 162 do not (Jinja2's parser spends six
    /// frames a level), 970 `not`s and not 971 (one parser frame each), and
    /// chains of 979 `and` or `or` operands and not 980 (the evaluator
    /// recurses down the left-deep chain). Far deeper ones fail the same
    /// way, without exhausting the stack.
    #[test]
    fn a_condition_past_pythons_recursion_limit_fails_as_sphinx_does() {
        let tags = Tags::new(["html".to_string()]);
        let recursion = Err("maximum recursion depth exceeded".to_string());
        let parens = |n: usize| format!("{}html{}", "(".repeat(n), ")".repeat(n));
        let nots = |n: usize| format!("{}html", "not ".repeat(n));
        let chain = |n: usize, op: &str| vec!["html"; n].join(op);
        let rows: Vec<(String, Result<bool, String>)> = vec![
            (parens(161), Ok(true)),
            (parens(162), recursion.clone()),
            (nots(970), Ok(true)),
            (nots(971), recursion.clone()),
            (chain(979, " and "), Ok(true)),
            (chain(980, " and "), recursion.clone()),
            (chain(979, " or "), Ok(true)),
            (chain(980, " or "), recursion.clone()),
            (parens(20_000), recursion.clone()),
            (nots(20_000), recursion.clone()),
            (chain(200_000, " and "), recursion.clone()),
        ];
        for (expr, expected) in rows {
            let head: String = expr.chars().take(40).collect();
            assert_eq!(
                tags.eval_condition(&expr),
                expected,
                "{head}… ({} bytes)",
                expr.len()
            );
        }
    }

    /// `Tags.eval_condition` over Sphinx 9.1.0's truth table, probed with
    /// `Tags(['html', 'format_html', 'builder_html', 'mytag'])`: each row is
    /// the condition and either its value or `str()` of the exception it
    /// raises, which the only directive's warning prints. A Jinja2 syntax
    /// error names its line; `chunk after expression` and `invalid node,
    /// check parsing` are the parser's and the evaluator's own errors.
    #[test]
    fn tags_eval_condition_matches_sphinx() {
        let tags = Tags::new(["html", "format_html", "builder_html", "mytag"].map(String::from));
        let syntax = |message: &str, line: u32| Err(format!("{message}\n  line {line}"));
        let plain = |message: &str| Err(message.to_string());
        let rows: &[(&str, Result<bool, String>)] = &[
            ("html", Ok(true)),
            ("latex", Ok(false)),
            ("not html", Ok(false)),
            ("not latex", Ok(true)),
            ("html and latex", Ok(false)),
            ("html or latex", Ok(true)),
            ("html and not latex", Ok(true)),
            ("not html or mytag", Ok(true)),
            ("(latex or html) and mytag", Ok(true)),
            ("not (html or latex)", Ok(false)),
            ("not not html", Ok(true)),
            ("((html))", Ok(true)),
            ("html if mytag else latex", Ok(true)),
            ("latex if mytag else html", Ok(false)),
            ("html if mytag", Ok(true)),
            ("x if y else z if w else v", Ok(false)),
            // `and` and `or` short-circuit before the constant.
            ("latex and true", Ok(false)),
            ("html or true", Ok(true)),
            // A keyword with nothing after it is a tag name.
            ("and", Ok(false)),
            ("Html", Ok(false)),
            ("_", Ok(false)),
            ("\u{e9}", Ok(false)),
            ("x\u{301}", Ok(false)),
            ("a\u{203f}b", Ok(false)),
            ("\u{2118}", Ok(false)),
            ("html\nor\nlatex", Ok(true)),
            ("html\r\nor latex", Ok(true)),
            ("html\tor\u{a0}latex", Ok(true)),
            ("html\u{2028}or latex", Ok(true)),
            // The evaluator knows names and the three operators only: a
            // constant, or a conditional without `else` whose test fails,
            // is an invalid node.
            ("True", plain("invalid node, check parsing")),
            ("none", plain("invalid node, check parsing")),
            ("html and true", plain("invalid node, check parsing")),
            ("html if latex", plain("invalid node, check parsing")),
            // The parser stops at what it cannot read.
            ("html latex", plain("chunk after expression")),
            ("or html", plain("chunk after expression")),
            ("html == latex", plain("chunk after expression")),
            ("a.b", plain("chunk after expression")),
            ("html -", plain("chunk after expression")),
            ("html {", plain("chunk after expression")),
            ("html }} x", plain("chunk after expression")),
            ("", syntax("unexpected token 'end of template'", 1)),
            ("html and", syntax("unexpected token 'end of template'", 1)),
            ("not\n", syntax("unexpected token 'end of template'", 1)),
            (
                "html and\n\n",
                syntax("unexpected token 'end of template'", 1),
            ),
            (
                "html\n\nand",
                syntax("unexpected token 'end of template'", 3),
            ),
            (
                "html or\n  (",
                syntax("unexpected token 'end of template'", 2),
            ),
            (
                "html if mytag else",
                syntax("unexpected token 'end of template'", 1),
            ),
            (
                "(html",
                syntax("unexpected end of template, expected ')'.", 1),
            ),
            (
                "(\nhtml",
                syntax("unexpected end of template, expected ')'.", 2),
            ),
            ("(html foo)", syntax("expected token ')', got 'foo'", 1)),
            ("()", syntax("unexpected token ')'", 1)),
            ("1", syntax("unexpected token 'integer'", 1)),
            ("html or 0x1f", syntax("unexpected token 'integer'", 1)),
            ("0b2", syntax("unexpected token 'integer'", 1)),
            ("1.5", syntax("unexpected token 'float'", 1)),
            ("1e3", syntax("unexpected token 'float'", 1)),
            (".5", syntax("unexpected token '.'", 1)),
            ("\"html\"", syntax("unexpected token 'string'", 1)),
            ("html or [", syntax("unexpected token '['", 1)),
            ("{{ html }}", syntax("unexpected token '{'", 1)),
            ("}}", syntax("unexpected token 'end of print statement'", 1)),
            (
                "-}}",
                syntax("unexpected token 'end of print statement'", 1),
            ),
            // Jinja2's lexer errors, raised when the parser reaches them.
            ("html)", syntax("unexpected ')'", 1)),
            ("html\n)", syntax("unexpected ')'", 2)),
            ("html ]", syntax("unexpected ']'", 1)),
            ("(html]", syntax("unexpected ']', expected ')'", 1)),
            ("html or (latex}", syntax("unexpected '}', expected ')'", 1)),
            ("(html }} )", syntax("unexpected '}', expected ')'", 1)),
            ("html & latex", syntax("unexpected char '&' at 5", 1)),
            ("html $", syntax("unexpected char '$' at 5", 1)),
            ("html or \"x", syntax("unexpected char '\"' at 8", 1)),
            ("\u{b7}x", syntax("Invalid character in identifier", 1)),
            ("html or \"a\\x\"", syntax("truncated \\xXX escape", 1)),
            ("html or \"a\\u12\"", syntax("truncated \\uXXXX escape", 1)),
            (
                "html or \"a\\U0011ffff\"",
                syntax("illegal Unicode character", 1),
            ),
            (
                "html or \"a\\N{x\"",
                syntax("malformed \\N character escape", 1),
            ),
        ];
        let mismatches: Vec<String> = rows
            .iter()
            .filter_map(|(expr, expected)| {
                let actual = tags.eval_condition(expr);
                (&actual != expected)
                    .then(|| format!("{expr:?}: expected {expected:?}, got {actual:?}"))
            })
            .collect();
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
