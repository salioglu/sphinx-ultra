//! `sphinx.pycode` — the `ModuleAnalyzer`/`DefinitionFinder` surface the
//! `literalinclude` `:pyobject:` filter consumes.
//!
//! [`find_tags`] is the port of `ModuleAnalyzer.find_tags()`
//! (`SP/pycode/__init__.py:167-170` → `Parser.parse_definition`,
//! `SP/pycode/parser.py:623-627` → `DefinitionFinder`,
//! `SP/pycode/parser.py:514-588`, sphinx 9.1.0). Ground truth: research spec
//! §3.5 (cited as [INC §3.5]),
//! `docs/plans/2026-09-01-m2-wave4.5-research-spec-include-literalinclude.md`.
//!
//! `DefinitionFinder` is a **token-stream scanner, not an AST walk**: it reads
//! the `tokenize` stream and keeps two stacks — a dotted `context` of
//! class/def names and an `indents` stack of open blocks. That mechanic, not
//! Python's grammar, is what decides every line number, so the port keeps the
//! same shape:
//!
//! - a definition's start line is the `@` line of its FIRST decorator when one
//!   is pending, else the line of the NAME after `class`/`def` (`:563-567`);
//! - a one-liner (`def f(): return 1`) ends at the NAME's line — even when its
//!   signature spans continuation lines (`:573-576`, probe `oneliner_multiline_sig`);
//! - a block definition ends at `DEDENT.end_row - 1`, walked back over every
//!   line matching `emptyline_re = ^\s*(#.*)?$` — blank lines AND comment-only
//!   lines (`:578-588`, probe `comment_trap`);
//! - `add_definition` drops a `def` whose immediately enclosing open block is
//!   also a `def` (`:526-532`), so nested functions vanish but methods survive
//!   as `Class.method` — and a `def` nested in a `def` through an intervening
//!   `if`/`for`/`with` block SURVIVES, because that block pushed an `'other'`
//!   entry (probe `def_in_if_in_def`: `outer.inner` is recorded);
//! - the context stack tracks only class/def names, so a `def` inside a
//!   module-level `if` is recorded under its bare name (probe `def_in_if`);
//! - `async def` needs no special case: `async` is an ordinary NAME token and
//!   `def` drives the scan (probe `deco_async`).
//!
//! ## The tokenizer simplification and its bounds
//!
//! Sphinx runs CPython's `tokenize`; this module runs a line-oriented scanner
//! that produces only what `DefinitionFinder` consumes: `INDENT`/`DEDENT` with
//! CPython's two-column (tabs-as-8 and tabs-as-1) bookkeeping, `NEWLINE` vs
//! `NL`, `COMMENT`, `NAME`, `NUMBER`, `STRING` and maximal-munch `OP`. It is
//! deliberately NOT a Python parser. Tested bounds (each pinned below against
//! the real `ModuleAnalyzer`):
//!
//! - logical vs physical lines: a bracketed continuation emits `NL`, not
//!   `NEWLINE`, and suppresses indentation processing (probe `continuation_sig`);
//!   so does a backslash continuation, which makes `def f(a): \` + `return a` a
//!   ONE-LINER (probe `backslash_continuation`);
//! - blank and comment-only lines never produce `INDENT`/`DEDENT`, which is why
//!   a `# comment` at column 0 inside a class does not close it (probe
//!   `def_then_dedent_to_comment_col0`);
//! - strings (all prefixes, triple quotes, escapes) are skipped whole, so
//!   `def `/`class ` inside a docstring creates no tag (probe
//!   `triple_quoted_trap`);
//! - `@` only starts a decorator when the previous token is `NEWLINE`/`NL`/
//!   `INDENT`/`DEDENT` or the stream just started — the matrix-multiplication
//!   operator inside a statement is not a decorator (probe `matmul_at`), but
//!   one after an in-bracket `NL` IS mistaken for one by sphinx, and this port
//!   reproduces that (probe `at_after_nl_in_parens`: start line 2, not 3);
//! - `filter_whitespace` (`parser.py:31-32`) replaces form feeds with spaces
//!   BEFORE lines are split, and line splitting is `str.splitlines(True)`, both
//!   mirrored here so line numbers agree (probe `formfeed`).
//!
//! Three places where the scanner is knowingly coarser than `tokenize`, none of
//! which can change a tag in a file that is valid Python:
//!
//! - number munching is greedy over alphanumerics, so `1if x else 2` becomes
//!   ONE `NUMBER` where CPython emits `NUMBER 1` + `NAME if`. Nothing the
//!   finder keys on (`def`/`class`/`@`/`:`) can be swallowed that way, since a
//!   number never precedes them at a header's top level;
//! - a name of 1-2 letters from `rRbBuUfF` immediately before a quote is taken
//!   for a string prefix, so an INVALID combination (`bb"x"`, `uu'y'`) is lexed
//!   as one `STRING` where CPython emits `NAME` + `STRING`. Such a file is a
//!   syntax error in Python, so sphinx warns on it anyway (the
//!   tokenizes-but-does-not-parse class below);
//! - the dedent branch checks CPython's two conditions in CPython's order —
//!   unindent-matches-no-outer-level first, tab inconsistency second — so which
//!   failure fires is faithful; only the rendered detail differs, since sphinx
//!   surfaces them wrapped in `IndentationError`/`TabError` reprs (below).
//!
//! ## Errors, and the divergence they carry
//!
//! Sphinx's `Parser.parse()` runs `ast.parse` BEFORE `DefinitionFinder`
//! (`parser.py:607-621`), so `find_tags` fails for EVERY file that is not
//! valid Python, with `PycodeError(f'parsing {srcname!r} failed: {exc!r}')`
//! (`__init__.py:158-160`) — rendered, probed verbatim, as
//! `parsing '/abs/broken.py' failed: SyntaxError('invalid syntax',
//! ('<unknown>', 1, 7, 'def f(:\n', 1, 8))`. That tail is a CPython
//! `SyntaxError` repr: version-specific wording, `'<unknown>'` from
//! `ast.parse`'s default filename, and byte offsets. This port has no Python
//! parser, so it cannot reproduce either the tail or the full failure SET.
//! The decision (task 15, evidence above):
//!
//! - our failures are a strict SUBSET of sphinx's — only what the scanner
//!   itself cannot get past (unterminated string, unclosed bracket at EOF,
//!   unindent that matches no outer level, inconsistent tabs/spaces), each of
//!   which also fails `ast.parse`;
//! - the caller ([`crate::rst`]'s literalinclude reader) keeps sphinx's
//!   `parsing %r failed: ` prefix and substitutes this error's [`Display`] for
//!   the un-reproducible `{exc!r}` tail;
//! - a file that tokenizes but does not PARSE (`x = = 1` after a clean `def`)
//!   yields tags here and a warning in sphinx. Documented divergence, not a
//!   bug to fix without a Python parser.
//!
//! Other documented divergences from the sphinx path:
//!
//! - sphinx reads the file itself with `tokenize.open` (BOM + PEP 263 coding
//!   cookie); this interface takes the reader's already-decoded text, so a
//!   non-UTF-8 file with a coding cookie and no `:encoding:` option differs;
//! - the reader has already applied `:tab-width:` expansion when it hands the
//!   text over (`read_file`, `SP/directives/code.py:221-240`), while sphinx's
//!   analyzer re-reads the RAW file. Expansion never changes the line COUNT,
//!   but at a `tab-width` other than 8 it can change the indentation COLUMNS
//!   this scanner measures, and with them the block structure and every end
//!   line derived from it: a `\t` is column 8 to CPython's tokenizer but four
//!   spaces after `expandtabs(4)`, so against a six-space line it flips from
//!   deeper to shallower — an `INDENT` where sphinx sees a `DEDENT`, or a
//!   clean parse here where sphinx raises `IndentationError`. Triggering it
//!   takes all three of `:tab-width:` (≠ 8), `:pyobject:`, and a file that
//!   mixes tab and space indentation;
//! - PEP 701 f-strings that reuse the outer quote inside a replacement field
//!   (`f"{"a"}"`) are lexed here as pre-3.12 string literals.

use std::collections::BTreeMap;

/// The first member of a `find_tags` entry — sphinx's `'class'` / `'def'`
/// tag strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKind {
    Class,
    Def,
}

impl TagKind {
    /// The sphinx tag string this kind renders as.
    pub fn as_str(self) -> &'static str {
        match self {
            TagKind::Class => "class",
            TagKind::Def => "def",
        }
    }
}

/// `sphinx.errors.PycodeError` — every analyzer failure funnels through
/// `LiteralInclude.run()`'s broad `except` into a reporter warning whose
/// text is this error's `Display`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PycodeError(pub String);

impl std::fmt::Display for PycodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PycodeError {}

/// `ModuleAnalyzer.find_tags()` over already-decoded source text:
/// `dotted_name -> (kind, start_line, end_line)`, both line numbers
/// 1-based inclusive, exactly as `lines[start - 1:end]` expects.
///
/// Total: any input either yields a map or a [`PycodeError`]; nothing panics.
pub fn find_tags(source: &str) -> Result<BTreeMap<String, (TagKind, u32, u32)>, PycodeError> {
    // `Parser.__init__` (`parser.py:597-598`) filters form feeds, and
    // `parse_definition` (`:625`) splits with `str.splitlines(True)`; the
    // tokenizer then reads those buffers back one line at a time, so BOTH
    // steps decide the line numbering the tags carry.
    let code = filter_whitespace(source);
    let lines = splitlines_keepends(&code);
    let tokens = tokenize(&lines)?;
    DefinitionFinder::new(&tokens, &lines).parse()
}

/// `filter_whitespace` (`parser.py:31-32`): FF becomes a space. Runs before
/// line splitting, so a form feed never ends a line (unlike bare
/// `str.splitlines`).
fn filter_whitespace(code: &str) -> String {
    code.replace('\x0c', " ")
}

/// Python `str.splitlines(keepends=True)`: the full boundary set, `\r\n` kept
/// as one ending. (Mirrors the literalinclude reader's helper; kept local so
/// `py::` does not depend on `rst::`.)
fn splitlines_keepends(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if is_line_boundary(c) {
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(j, '\n')) = chars.peek() {
                    chars.next();
                    end = j + 1;
                }
            }
            out.push(text[start..end].to_string());
            start = end;
        }
    }
    if start < text.len() {
        out.push(text[start..].to_string());
    }
    out
}

fn is_line_boundary(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\x0b'
            | '\x0c'
            | '\x1c'
            | '\x1d'
            | '\x1e'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// `emptyline_re = ^\s*(#.*)?$` (`parser.py:28`) applied with `re.match` to a
/// keepends line: whitespace only, or whitespace then a comment.
fn is_emptyline(line: &str) -> bool {
    let body = line.strip_suffix('\n').unwrap_or(line);
    let rest = body.trim_start_matches(char::is_whitespace);
    rest.is_empty() || rest.starts_with('#')
}

// ---------------------------------------------------------------------------
// The tokenizer (the `tokenize` stand-in; see the module docs for its bounds)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Name,
    Number,
    Str,
    Op,
    Comment,
    /// End of a logical line.
    Newline,
    /// Non-logical newline: blank line, comment-only line, or a line break
    /// inside brackets.
    Nl,
    Indent,
    Dedent,
}

#[derive(Debug, Clone)]
struct Tok {
    kind: Kind,
    /// Token text — only `Name` and `Op` are ever compared by value, so the
    /// other kinds carry an empty string.
    text: String,
    start_row: u32,
    end_row: u32,
}

const OPS3: &[&str] = &["**=", "//=", "<<=", ">>=", "..."];
const OPS2: &[&str] = &[
    "**", "//", "<<", ">>", "<=", ">=", "==", "!=", "->", ":=", "+=", "-=", "*=", "/=", "%=", "@=",
    "&=", "|=", "^=",
];

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_name_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Python string-literal prefixes: 1-2 letters from `rRbBuUfF`.
fn is_string_prefix(word: &str) -> bool {
    !word.is_empty() && word.len() <= 2 && word.chars().all(|c| "rRbBuUfF".contains(c))
}

struct Tokenizer {
    /// Lines with their terminators stripped, as char vectors.
    body: Vec<Vec<char>>,
    toks: Vec<Tok>,
    /// CPython's `indstack`/`altindstack` pair: column with tabs rounded up to
    /// multiples of 8, and column with tabs counted as 1. A mismatch between
    /// the two is CPython's `TabError`.
    indents: Vec<(usize, usize)>,
    /// Open brackets and the row each was opened on.
    brackets: Vec<(char, u32)>,
    continued: bool,
    line_has_tokens: bool,
    row: usize,
    col: usize,
}

fn tokenize(lines: &[String]) -> Result<Vec<Tok>, PycodeError> {
    let body: Vec<Vec<char>> = lines
        .iter()
        .map(|l| line_body(l).chars().collect())
        .collect();
    let mut tk = Tokenizer {
        body,
        toks: Vec::new(),
        indents: vec![(0, 0)],
        brackets: Vec::new(),
        continued: false,
        line_has_tokens: false,
        row: 0,
        col: 0,
    };
    tk.run()?;
    Ok(tk.toks)
}

/// Drop the line terminator `splitlines_keepends` left on.
fn line_body(line: &str) -> &str {
    if let Some(rest) = line.strip_suffix("\r\n") {
        rest
    } else {
        line.strip_suffix(is_line_boundary).unwrap_or(line)
    }
}

impl Tokenizer {
    fn push(&mut self, kind: Kind, text: &str, start_row: usize, end_row: usize) {
        self.toks.push(Tok {
            kind,
            text: text.to_string(),
            start_row: start_row as u32 + 1,
            end_row: end_row as u32 + 1,
        });
    }

    fn cur(&self) -> Option<char> {
        self.body[self.row].get(self.col).copied()
    }

    fn at(&self, offset: usize) -> Option<char> {
        self.body[self.row].get(self.col + offset).copied()
    }

    fn line_len(&self) -> usize {
        self.body[self.row].len()
    }

    fn run(&mut self) -> Result<(), PycodeError> {
        let mut at_line_start = true;
        while self.row < self.body.len() {
            if at_line_start {
                at_line_start = false;
                if self.brackets.is_empty() && !self.continued {
                    if self.start_of_logical_line()? {
                        // Blank or comment-only line: no indentation
                        // processing, no tokens beyond the COMMENT/NL pair.
                        self.row += 1;
                        self.col = 0;
                        at_line_start = true;
                        continue;
                    }
                } else {
                    self.col = 0;
                }
                self.continued = false;
            }
            if self.col >= self.line_len() {
                self.end_of_physical_line();
                self.row += 1;
                self.col = 0;
                at_line_start = true;
                continue;
            }
            self.scan_token()?;
        }
        self.finish()
    }

    /// Indentation processing for a fresh logical line. Returns `true` when
    /// the line is blank or comment-only, which CPython's tokenizer skips
    /// entirely (no `INDENT`/`DEDENT`).
    fn start_of_logical_line(&mut self) -> Result<bool, PycodeError> {
        let (col, altcol, first) = self.measure_indent();
        self.col = first;
        match self.body[self.row].get(first) {
            None => {
                self.push(Kind::Nl, "", self.row, self.row);
                return Ok(true);
            }
            Some('#') => {
                self.push(Kind::Comment, "", self.row, self.row);
                self.push(Kind::Nl, "", self.row, self.row);
                return Ok(true);
            }
            Some(_) => {}
        }
        // CPython `tokenizer.c`: compare against the top of the stack, with
        // the alternate (tabs-as-1) column policing tab/space consistency.
        let &(top, alttop) = self.indents.last().expect("indent stack is never empty");
        if col == top {
            if altcol != alttop {
                return Err(self.tab_error());
            }
        } else if col > top {
            if altcol <= alttop {
                return Err(self.tab_error());
            }
            self.indents.push((col, altcol));
            self.push(Kind::Indent, "", self.row, self.row);
        } else {
            while self.indents.len() > 1 && col < self.indents[self.indents.len() - 1].0 {
                self.indents.pop();
                self.push(Kind::Dedent, "", self.row, self.row);
            }
            let &(top, alttop) = self.indents.last().expect("indent stack is never empty");
            if col != top {
                return Err(PycodeError(
                    "unindent does not match any outer indentation level".to_string(),
                ));
            }
            if altcol != alttop {
                return Err(self.tab_error());
            }
        }
        Ok(false)
    }

    /// CPython's indentation measurement: a tab advances to the next multiple
    /// of 8 in `col` and by one in `altcol`. (Form feeds cannot appear —
    /// `filter_whitespace` turned them into spaces.)
    fn measure_indent(&self) -> (usize, usize, usize) {
        let (mut col, mut altcol, mut i) = (0usize, 0usize, 0usize);
        let line = &self.body[self.row];
        while let Some(&c) = line.get(i) {
            match c {
                ' ' => {
                    col += 1;
                    altcol += 1;
                }
                '\t' => {
                    col = (col / 8 + 1) * 8;
                    altcol += 1;
                }
                _ => break,
            }
            i += 1;
        }
        (col, altcol, i)
    }

    fn tab_error(&self) -> PycodeError {
        PycodeError("inconsistent use of tabs and spaces in indentation".to_string())
    }

    fn end_of_physical_line(&mut self) {
        if self.continued {
            return;
        }
        if !self.brackets.is_empty() {
            self.push(Kind::Nl, "", self.row, self.row);
        } else if self.line_has_tokens {
            self.push(Kind::Newline, "", self.row, self.row);
            self.line_has_tokens = false;
        } else {
            self.push(Kind::Nl, "", self.row, self.row);
        }
    }

    fn scan_token(&mut self) -> Result<(), PycodeError> {
        let c = self.cur().expect("caller checked the column");
        if c == ' ' || c == '\t' || c == '\r' {
            self.col += 1;
            return Ok(());
        }
        if c == '#' {
            self.push(Kind::Comment, "", self.row, self.row);
            self.col = self.line_len();
            return Ok(());
        }
        if c == '\\' && self.col + 1 >= self.line_len() {
            self.continued = true;
            self.col = self.line_len();
            return Ok(());
        }
        if is_name_start(c) {
            let start = self.col;
            while self.cur().is_some_and(is_name_continue) {
                self.col += 1;
            }
            let word: String = self.body[self.row][start..self.col].iter().collect();
            if matches!(self.cur(), Some('\'' | '"')) && is_string_prefix(&word) {
                return self.lex_string();
            }
            self.line_has_tokens = true;
            self.push(Kind::Name, &word, self.row, self.row);
            return Ok(());
        }
        if c.is_ascii_digit() || (c == '.' && self.at(1).is_some_and(|d| d.is_ascii_digit())) {
            self.lex_number();
            return Ok(());
        }
        if c == '\'' || c == '"' {
            return self.lex_string();
        }
        self.lex_op();
        Ok(())
    }

    /// A numeric literal, consumed whole so that `1e-5` cannot leak a `-`
    /// operator into the stream. Boundaries mirror `arglist::lex_number`.
    fn lex_number(&mut self) {
        let line = &self.body[self.row];
        let start = self.col;
        let radix_prefixed = line.get(start) == Some(&'0')
            && matches!(line.get(start + 1), Some('x' | 'X' | 'b' | 'B' | 'o' | 'O'));
        let mut prev = line[start];
        self.col += 1;
        while let Some(d) = self.cur() {
            let continues = d.is_ascii_alphanumeric()
                || d == '_'
                || d == '.'
                || ((d == '+' || d == '-') && matches!(prev, 'e' | 'E') && !radix_prefixed);
            if !continues {
                break;
            }
            prev = d;
            self.col += 1;
        }
        self.line_has_tokens = true;
        self.push(Kind::Number, "", self.row, self.row);
    }

    /// A string literal starting at the opening quote (any prefix already
    /// consumed). Triple-quoted literals and backslash-escaped newlines walk
    /// to later rows; the token's `end_row` is where the closing quote sits.
    fn lex_string(&mut self) -> Result<(), PycodeError> {
        let start_row = self.row;
        let quote = self.cur().expect("caller peeked the quote");
        self.col += 1;
        let triple = self.cur() == Some(quote) && self.at(1) == Some(quote);
        if triple {
            self.col += 2;
        }
        loop {
            let Some(c) = self.cur() else {
                // End of a physical line inside the literal: legal for a
                // triple-quoted one, and for a backslash-escaped newline
                // (handled below), never otherwise.
                if !triple {
                    return Err(PycodeError(format!(
                        "unterminated string literal (detected at line {})",
                        start_row + 1
                    )));
                }
                if self.row + 1 >= self.body.len() {
                    return Err(PycodeError(format!(
                        "unterminated triple-quoted string literal (detected at line {})",
                        self.body.len()
                    )));
                }
                self.row += 1;
                self.col = 0;
                continue;
            };
            if c == '\\' {
                self.col += 1;
                if self.cur().is_none() {
                    if self.row + 1 >= self.body.len() {
                        return Err(PycodeError(format!(
                            "unterminated string literal (detected at line {})",
                            start_row + 1
                        )));
                    }
                    self.row += 1;
                    self.col = 0;
                } else {
                    self.col += 1;
                }
                continue;
            }
            if c == quote {
                if !triple {
                    self.col += 1;
                    break;
                }
                if self.at(1) == Some(quote) && self.at(2) == Some(quote) {
                    self.col += 3;
                    break;
                }
            }
            self.col += 1;
        }
        self.line_has_tokens = true;
        let end_row = self.row;
        self.push(Kind::Str, "", start_row, end_row);
        Ok(())
    }

    /// Maximal-munch operator, 3-2-1 characters, tracking bracket depth. An
    /// unmatched closer is ignored (CPython errors; such a file never parses,
    /// so sphinx warns instead — see the module docs).
    fn lex_op(&mut self) {
        let rest: String = self.body[self.row][self.col..].iter().collect();
        let text = OPS3
            .iter()
            .chain(OPS2.iter())
            .find(|cand| rest.starts_with(**cand))
            .map(|cand| (*cand).to_string())
            .unwrap_or_else(|| rest.chars().next().into_iter().collect());
        self.col += text.chars().count();
        match text.as_str() {
            "(" | "[" | "{" => self
                .brackets
                .push((text.chars().next().expect("one char"), self.row as u32 + 1)),
            ")" | "]" | "}" => {
                self.brackets.pop();
            }
            _ => {}
        }
        self.line_has_tokens = true;
        self.push(Kind::Op, &text, self.row, self.row);
    }

    /// EOF: CPython closes the last logical line, then emits one `DEDENT` per
    /// open level at row `len(lines) + 1` (probed: that row holds whether the
    /// file ends with a newline or not).
    fn finish(&mut self) -> Result<(), PycodeError> {
        if let Some(&(opener, row)) = self.brackets.first() {
            return Err(PycodeError(format!(
                "'{opener}' was never closed (opened at line {row})"
            )));
        }
        let eof_row = self.body.len();
        while self.indents.len() > 1 {
            self.indents.pop();
            self.push(Kind::Dedent, "", eof_row, eof_row);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// DefinitionFinder (`parser.py:514-588`)
// ---------------------------------------------------------------------------

/// An entry on `DefinitionFinder.indents`: `'other'` for a plain indented
/// block, `'class'`/`'def'` for a definition body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    Other,
    Class,
    Def,
}

impl Block {
    fn of(kind: TagKind) -> Self {
        match kind {
            TagKind::Class => Block::Class,
            TagKind::Def => Block::Def,
        }
    }
}

/// The `fetch_until` terminator: `[OP, ':']` or the `INDENT` kind.
#[derive(Debug, Clone, Copy)]
enum Cond {
    Colon,
    Indent,
}

struct DefinitionFinder<'a> {
    toks: &'a [Tok],
    lines: &'a [String],
    next: usize,
    current: Option<usize>,
    previous: Option<usize>,
    decorator: Option<u32>,
    context: Vec<String>,
    indents: Vec<(Block, String, u32)>,
    definitions: BTreeMap<String, (TagKind, u32, u32)>,
}

impl<'a> DefinitionFinder<'a> {
    fn new(toks: &'a [Tok], lines: &'a [String]) -> Self {
        Self {
            toks,
            lines,
            next: 0,
            current: None,
            previous: None,
            decorator: None,
            context: Vec::new(),
            indents: Vec::new(),
            definitions: BTreeMap::new(),
        }
    }

    fn tok(&self, i: usize) -> &'a Tok {
        &self.toks[i]
    }

    /// `TokenProcessor.fetch_token` (`parser.py:156-167`): `previous` shifts
    /// even on exhaustion, when `current` becomes `None`.
    fn fetch(&mut self) -> Option<usize> {
        self.previous = self.current;
        self.current = (self.next < self.toks.len()).then(|| {
            let i = self.next;
            self.next += 1;
            i
        });
        self.current
    }

    fn parse(mut self) -> Result<BTreeMap<String, (TagKind, u32, u32)>, PycodeError> {
        while let Some(i) = self.fetch() {
            let t = self.tok(i);
            match t.kind {
                Kind::Comment => {}
                Kind::Op if t.text == "@" => {
                    if self.decorator.is_none() && self.previous_ends_a_line() {
                        self.decorator = Some(t.start_row);
                    }
                }
                Kind::Name if t.text == "class" => self.parse_definition(TagKind::Class)?,
                Kind::Name if t.text == "def" => self.parse_definition(TagKind::Def)?,
                Kind::Indent => self.indents.push((Block::Other, String::new(), 0)),
                Kind::Dedent => self.finalize_block()?,
                _ => {}
            }
        }
        Ok(self.definitions)
    }

    /// `self.previous is None or self.previous.match(NEWLINE, NL, INDENT,
    /// DEDENT)` (`:542-545`) — the guard that keeps `a @ b` from starting a
    /// definition.
    fn previous_ends_a_line(&self) -> bool {
        match self.previous {
            None => true,
            Some(i) => matches!(
                self.tok(i).kind,
                Kind::Newline | Kind::Nl | Kind::Indent | Kind::Dedent
            ),
        }
    }

    /// `add_definition` (`:526-532`): a `def` directly inside a `def` body is
    /// dropped — but the entry has already been popped by the caller, so the
    /// test looks at the ENCLOSING block.
    fn add_definition(&mut self, name: String, entry: (TagKind, u32, u32)) {
        if entry.0 == TagKind::Def && self.indents.last().map(|e| e.0) == Some(Block::Def) {
            return;
        }
        self.definitions.insert(name, entry);
    }

    /// `parse_definition` (`:557-576`).
    fn parse_definition(&mut self, typ: TagKind) -> Result<(), PycodeError> {
        let Some(ni) = self.fetch() else {
            return Err(PycodeError(
                "unexpected end of file after a definition keyword".to_string(),
            ));
        };
        let name = self.tok(ni);
        let (name_text, name_end) = (name.text.clone(), name.end_row);
        let start_pos = self.decorator.take().unwrap_or(name.start_row);
        self.context.push(name_text);
        let funcname = self.context.join(".");

        self.fetch_until(Cond::Colon);
        let Some(ti) = self.fetch() else {
            return Err(PycodeError(
                "unexpected end of file inside a definition header".to_string(),
            ));
        };
        if matches!(self.tok(ti).kind, Kind::Comment | Kind::Newline) {
            self.fetch_until(Cond::Indent);
            self.indents.push((Block::of(typ), funcname, start_pos));
        } else {
            // One-liner: ends at the NAME's line, however far the signature
            // ran (`:573-576`).
            self.add_definition(funcname, (typ, start_pos, name_end));
            let _ = self.context.pop();
        }
        Ok(())
    }

    /// `finalize_block` (`:578-588`).
    fn finalize_block(&mut self) -> Result<(), PycodeError> {
        let Some((block, funcname, start_pos)) = self.indents.pop() else {
            return Err(PycodeError(
                "unbalanced indentation while scanning definitions".to_string(),
            ));
        };
        if block == Block::Other {
            return Ok(());
        }
        let dedent_row = self.current.map_or(0, |i| self.tok(i).end_row);
        let mut end_pos = dedent_row.saturating_sub(1);
        while end_pos >= 1
            && (end_pos as usize) <= self.lines.len()
            && is_emptyline(&self.lines[end_pos as usize - 1])
        {
            end_pos -= 1;
        }
        let typ = if block == Block::Class {
            TagKind::Class
        } else {
            TagKind::Def
        };
        self.add_definition(funcname, (typ, start_pos, end_pos));
        let _ = self.context.pop();
        Ok(())
    }

    /// `fetch_until` (`:169-186`), iterative: the original recurses one level
    /// per bracket, an explicit closer stack keeps totality on adversarial
    /// nesting. The terminator is tested BEFORE the openers, exactly as the
    /// original does at each level.
    fn fetch_until(&mut self, cond: Cond) {
        let mut closers: Vec<&'static str> = Vec::new();
        while let Some(i) = self.fetch() {
            let t = self.tok(i);
            let hit = match closers.last() {
                Some(closer) => t.kind == Kind::Op && t.text == *closer,
                None => match cond {
                    Cond::Colon => t.kind == Kind::Op && t.text == ":",
                    Cond::Indent => t.kind == Kind::Indent,
                },
            };
            if hit {
                if closers.pop().is_none() {
                    return;
                }
                continue;
            }
            if t.kind == Kind::Op {
                match t.text.as_str() {
                    "(" => closers.push(")"),
                    "{" => closers.push("}"),
                    "[" => closers.push("]"),
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Every expected tag dict here is verbatim output of the REAL
    //! `sphinx.pycode.ModuleAnalyzer` (sphinx 9.1.0 on CPython 3.12.2), run
    //! in batch over these exact sources; the probe case name is cited on
    //! each test. Nothing here was written from memory.

    use super::*;

    /// The tag map as a sorted `(name, kind, start, end)` list — the same
    /// order and shape as the probe's `pprint(..., sort_dicts=True)`.
    fn tags(src: &str) -> Vec<(String, &'static str, u32, u32)> {
        find_tags(src)
            .expect("find_tags must succeed")
            .into_iter()
            .map(|(name, (kind, start, end))| (name, kind.as_str(), start, end))
            .collect()
    }

    fn t(name: &str, kind: &'static str, start: u32, end: u32) -> (String, &'static str, u32, u32) {
        (name.to_string(), kind, start, end)
    }

    const FIXTURE: &str = include_str!("../../tests/fixtures/literalinclude/example.py");

    /// probe `fixture`:
    /// `{'Foo': ('class', 11, 17), 'Foo.method': ('def', 16, 17),
    ///   'tail': ('def', 20, 21), 'top': ('def', 6, 8)}` — `Foo` ends at 17
    /// because the trailing blank lines are trimmed.
    #[test]
    fn fixture_module_tags_match_the_probe() {
        assert_eq!(
            tags(FIXTURE),
            vec![
                t("Foo", "class", 11, 17),
                t("Foo.method", "def", 16, 17),
                t("tail", "def", 20, 21),
                t("top", "def", 6, 8),
            ]
        );
    }

    // ---- decorators --------------------------------------------------

    /// probe `deco_plain`: `{'Weird': ('class', 9, 11), 'cached': ('def', 4, 6)}`
    /// — the `@` line, not the `def` line, is the start.
    #[test]
    fn a_decorator_line_becomes_the_start_for_defs_and_classes() {
        let src = "import functools\n\n\n@functools.cache\ndef cached(x):\n    return x\n\n\n\
                   @staticmethod\nclass Weird:\n    pass\n";
        assert_eq!(
            tags(src),
            vec![t("Weird", "class", 9, 11), t("cached", "def", 4, 6)]
        );
    }

    /// probes `deco_args_multiline` `{'f': ('def', 1, 6)}`, `deco_stacked`
    /// `{'f': ('def', 1, 5)}` (only the FIRST `@` counts) and
    /// `deco_blank_and_comment_between` `{'f': ('def', 1, 5)}`.
    #[test]
    fn argumented_stacked_and_detached_decorators_all_start_at_the_first_at() {
        assert_eq!(
            tags("@decorator(\n    \"a\",\n    \"b\",\n)\ndef f():\n    return 1\n"),
            vec![t("f", "def", 1, 6)]
        );
        assert_eq!(
            tags("@one\n@two(3)\n@three\ndef f():\n    pass\n"),
            vec![t("f", "def", 1, 5)]
        );
        assert_eq!(
            tags("@deco\n\n# a comment\ndef f():\n    pass\n"),
            vec![t("f", "def", 1, 5)]
        );
    }

    /// probe `deco_async`: `{'f': ('def', 1, 3)}` — `async` is a plain NAME,
    /// the `def` after it drives the scan.
    #[test]
    fn async_def_needs_no_special_case() {
        assert_eq!(
            tags("@deco\nasync def f(a, b):\n    await g()\n"),
            vec![t("f", "def", 1, 3)]
        );
        // probe `oneliner_async`: `{'f': ('def', 1, 1)}`
        assert_eq!(tags("async def f(): return 1\n"), vec![t("f", "def", 1, 1)]);
        // probe `async_block`: the inner `async def` is suppressed like any
        // nested def; the class between them is not.
        assert_eq!(
            tags(
                "async def outer():\n    async def inner():\n        pass\n\n\
                 \x20   class Inner:\n        async def m(self):\n            pass\n"
            ),
            vec![
                t("outer", "def", 1, 7),
                t("outer.Inner", "class", 5, 7),
                t("outer.Inner.m", "def", 6, 7),
            ]
        );
    }

    /// probe `matmul_at`: `{'f': ('def', 3, 4)}` — `c = a @ b` is not a
    /// decorator, because the previous token is a NAME.
    #[test]
    fn matrix_multiplication_is_not_a_decorator() {
        assert_eq!(
            tags("a = b\nc = a @ b\ndef f():\n    pass\n"),
            vec![t("f", "def", 3, 4)]
        );
    }

    /// probe `at_after_nl_in_parens`: `{'f': ('def', 2, 4)}`. Sphinx's guard
    /// accepts NL — the newline INSIDE brackets — so a line-leading `@`
    /// operator in a bracketed expression IS taken for a decorator and moves
    /// the next definition's start line. Faithfully reproduced.
    #[test]
    fn an_at_after_an_in_bracket_newline_is_mistaken_for_a_decorator() {
        assert_eq!(
            tags("x = (a\n@ b)\ndef f():\n    pass\n"),
            vec![t("f", "def", 2, 4)]
        );
        // probe `at_after_nl_in_parens_matrix`: `{'f': ('def', 3, 8)}`
        assert_eq!(
            tags("m = (\n    a\n    @ b\n)\n\n\ndef f():\n    pass\n"),
            vec![t("f", "def", 3, 8)]
        );
    }

    // ---- one-liners --------------------------------------------------

    /// probe `oneliner`: `{'C': ('class', 2, 2), 'f': ('def', 1, 1),
    /// 'g': ('def', 3, 4)}`.
    #[test]
    fn one_liners_end_at_their_header_line() {
        assert_eq!(
            tags("def f(): return 1\nclass C: pass\ndef g():\n    pass\n"),
            vec![
                t("C", "class", 2, 2),
                t("f", "def", 1, 1),
                t("g", "def", 3, 4),
            ]
        );
    }

    /// probe `oneliner_multiline_sig`: `{'f': ('def', 1, 1)}` — the end line
    /// is the NAME's line, so a one-liner whose signature wrapped ends
    /// BEFORE its own body. Sphinx quirk, kept.
    #[test]
    fn a_one_liner_with_a_wrapped_signature_ends_at_the_name_line() {
        assert_eq!(
            tags("def f(a,\n      b): return a + b\nx = 1\n"),
            vec![t("f", "def", 1, 1)]
        );
    }

    /// probe `backslash_continuation`: `{'f': ('def', 1, 1)}` — the
    /// backslash keeps the logical line open, so this is a one-liner.
    #[test]
    fn a_backslash_continued_body_is_still_a_one_liner() {
        assert_eq!(
            tags("def f(a): \\\n    return a\n\n\nx = 1\n"),
            vec![t("f", "def", 1, 1)]
        );
    }

    /// probe `oneliner_semicolons`: `{'f': ('def', 1, 1), 'g': ('def', 2, 2)}`.
    #[test]
    fn semicolon_bodies_stay_one_liners() {
        assert_eq!(
            tags("def f(): x = 1; return x\ndef g(): pass\n"),
            vec![t("f", "def", 1, 1), t("g", "def", 2, 2)]
        );
    }

    // ---- nesting -----------------------------------------------------

    /// probes `nested_def` `{'outer': ('def', 1, 4)}` and
    /// `nested_def_oneliner` `{'outer': ('def', 1, 3)}`.
    #[test]
    fn a_def_directly_inside_a_def_is_dropped() {
        assert_eq!(
            tags("def outer():\n    def inner():\n        pass\n    return inner\n"),
            vec![t("outer", "def", 1, 4)]
        );
        assert_eq!(
            tags("def outer():\n    def inner(): pass\n    return inner\n"),
            vec![t("outer", "def", 1, 3)]
        );
    }

    /// probes `def_in_if_in_def` `{'outer': ('def', 1, 5),
    /// 'outer.inner': ('def', 3, 4)}` and `oneliner_def_in_if_in_def`
    /// `{'outer': ('def', 1, 4), 'outer.inner': ('def', 3, 3)}`. The
    /// suppression test looks only at the IMMEDIATELY enclosing block, and
    /// the `if` pushed an `'other'` one — so the nested def SURVIVES, under
    /// its dotted name.
    #[test]
    fn an_intervening_if_block_defeats_the_nested_def_suppression() {
        assert_eq!(
            tags("def outer():\n    if True:\n        def inner():\n            pass\n    return 1\n"),
            vec![t("outer", "def", 1, 5), t("outer.inner", "def", 3, 4)]
        );
        assert_eq!(
            tags("def outer():\n    if True:\n        def inner(): pass\n    return 1\n"),
            vec![t("outer", "def", 1, 4), t("outer.inner", "def", 3, 3)]
        );
    }

    /// probe `def_in_if`: `{'g': ('def', 2, 3), 'h': ('def', 5, 6)}` — the
    /// context stack holds only class/def names, so a def inside a
    /// module-level `if` is recorded under its BARE name (no `if` prefix).
    /// probe `def_in_try_with_for`: `{'a': ('def', 2, 3), 'b': ('def', 8, 9),
    /// 'c': ('def', 12, 13)}` — same for `try`/`with`/`for`.
    #[test]
    fn a_def_inside_a_plain_block_keeps_its_bare_name() {
        assert_eq!(
            tags("if True:\n    def g():\n        pass\nelse:\n    def h():\n        pass\n"),
            vec![t("g", "def", 2, 3), t("h", "def", 5, 6)]
        );
        assert_eq!(
            tags(
                "try:\n    def a():\n        pass\nexcept Exception:\n    pass\n\n\
                 with open('x') as f:\n    def b():\n        pass\n\n\
                 for i in range(3):\n    def c():\n        pass\n"
            ),
            vec![
                t("a", "def", 2, 3),
                t("b", "def", 8, 9),
                t("c", "def", 12, 13),
            ]
        );
    }

    /// probe `class_in_class`: `{'Outer': ('class', 1, 7),
    /// 'Outer.Inner': ('class', 2, 6), 'Outer.Inner.method': ('def', 3, 4)}`.
    #[test]
    fn nested_classes_dot_their_names_and_trim_their_own_tails() {
        assert_eq!(
            tags(
                "class Outer:\n    class Inner:\n        def method(self):\n            pass\n\n\
                 \x20       attr = 1\n    x = 2\n"
            ),
            vec![
                t("Outer", "class", 1, 7),
                t("Outer.Inner", "class", 2, 6),
                t("Outer.Inner.method", "def", 3, 4),
            ]
        );
    }

    /// probe `class_in_def`: `{'outer': ('def', 1, 5),
    /// 'outer.Inner': ('class', 2, 4), 'outer.Inner.m': ('def', 3, 4)}` — a
    /// CLASS inside a def is kept (the suppression is def-in-def only), and
    /// its methods come with it.
    #[test]
    fn a_class_inside_a_def_survives_with_its_methods() {
        assert_eq!(
            tags("def outer():\n    class Inner:\n        def m(self):\n            pass\n    return Inner\n"),
            vec![
                t("outer", "def", 1, 5),
                t("outer.Inner", "class", 2, 4),
                t("outer.Inner.m", "def", 3, 4),
            ]
        );
    }

    /// probe `deep_nesting`: `{'A': ('class', 1, 7), 'A.B': ('class', 2, 7),
    /// 'A.B.C': ('class', 3, 7), 'A.B.C.m': ('def', 4, 7)}` — every EOF
    /// dedent lands on the same end line, and `inner` is dropped.
    #[test]
    fn eof_dedents_close_every_open_block_at_the_same_line() {
        assert_eq!(
            tags(
                "class A:\n    class B:\n        class C:\n            def m(self):\n\
                 \x20               def inner():\n                    pass\n                return inner\n"
            ),
            vec![
                t("A", "class", 1, 7),
                t("A.B", "class", 2, 7),
                t("A.B.C", "class", 3, 7),
                t("A.B.C.m", "def", 4, 7),
            ]
        );
    }

    // ---- string / comment traps --------------------------------------

    /// probe `triple_quoted_trap`: `{'real': ('def', 10, 16)}` — `def`/`class`
    /// text inside a module string and inside a docstring creates no tags.
    #[test]
    fn definitions_inside_strings_are_not_tags() {
        let src = "DOC = \"\"\"\ndef fake():\n    pass\n\nclass Fake:\n    pass\n\"\"\"\n\n\n\
                   def real():\n    \"\"\"Doc with def inside.\n\n    class AlsoFake:\n        pass\n    \"\"\"\n    return 1\n";
        assert_eq!(tags(src), vec![t("real", "def", 10, 16)]);
    }

    /// probe `string_prefixes`: `{'real': ('def', 7, 8)}` — f/r/b prefixed
    /// literals are skipped whole.
    #[test]
    fn prefixed_string_literals_are_skipped_whole() {
        let src = "s = f\"\"\"def nope():\n    pass\"\"\"\nr = r\"\"\"class Nope: pass\"\"\"\n\
                   b = b\"def nope2(): pass\"\n\n\ndef real():\n    pass\n";
        assert_eq!(tags(src), vec![t("real", "def", 7, 8)]);
    }

    /// probe `def_string_in_body`: `{'f': ('def', 1, 4)}`.
    #[test]
    fn single_quoted_strings_in_a_body_are_skipped() {
        assert_eq!(
            tags("def f():\n    s = \"def nope(): pass\"\n    t = 'class Nope: pass'\n    return s + t\n"),
            vec![t("f", "def", 1, 4)]
        );
    }

    /// probe `comment_trap`: `{'real': ('def', 3, 4)}` — commented-out defs
    /// create no tag, and the trailing comment line is trimmed off the end
    /// because `emptyline_re` matches comment-only lines too.
    #[test]
    fn comment_lines_make_no_tags_and_are_trimmed_from_block_ends() {
        assert_eq!(
            tags("# def commented():\n#     pass\ndef real():\n    pass\n# def trailing():\n"),
            vec![t("real", "def", 3, 4)]
        );
        // probe `trailing_comment_in_block`:
        // `{'f': ('def', 1, 2), 'g': ('def', 9, 10)}`
        assert_eq!(
            tags(
                "def f():\n    pass\n    # trailing comment\n    # another\n\n\
                 \x20   # after a blank\n\n\ndef g():\n    pass\n"
            ),
            vec![t("f", "def", 1, 2), t("g", "def", 9, 10)]
        );
    }

    /// probe `def_then_dedent_to_comment_col0`: `{'C': ('class', 1, 6),
    /// 'C.m': ('def', 2, 3), 'C.n': ('def', 5, 6)}` — a column-0 comment
    /// inside a class body does NOT dedent it.
    #[test]
    fn a_column_zero_comment_does_not_close_a_block() {
        assert_eq!(
            tags(
                "class C:\n    def m(self):\n        pass\n# comment at col 0\n\
                 \x20   def n(self):\n        pass\n"
            ),
            vec![
                t("C", "class", 1, 6),
                t("C.m", "def", 2, 3),
                t("C.n", "def", 5, 6),
            ]
        );
    }

    // ---- header parsing ----------------------------------------------

    /// probe `continuation_sig`: `{'C': ('class', 8, 11), 'f': ('def', 1, 5)}`
    /// — a wrapped signature ends where the block does, and the bracketed
    /// newlines never dedent anything.
    #[test]
    fn continuation_line_signatures_span_to_the_block_end() {
        assert_eq!(
            tags(
                "def f(\n    a,\n    b,\n):\n    return a\n\n\nclass C(\n    Base,\n):\n    pass\n"
            ),
            vec![t("C", "class", 8, 11), t("f", "def", 1, 5)]
        );
    }

    /// probes `annotations_colons` `{'f': ('def', 1, 2), 'g': ('def', 5, 6)}`,
    /// `lambda_default_colon` `{'f': ('def', 1, 2)}`,
    /// `walrus_and_dict_colon` `{'f': ('def', 1, 4)}`,
    /// `nested_parens_dict_set` `{'f': ('def', 1, 2)}` and
    /// `fstring_format_spec_default` `{'f': ('def', 1, 2)}`: only the
    /// TOP-LEVEL colon closes a header.
    #[test]
    fn only_the_top_level_colon_closes_a_definition_header() {
        assert_eq!(
            tags(
                "def f(a: int = 1, b: dict[str, int] = {}) -> dict[str, int]:\n    return b\n\n\n\
                 def g(h=lambda x: x):\n    return h\n"
            ),
            vec![t("f", "def", 1, 2), t("g", "def", 5, 6)]
        );
        assert_eq!(
            tags("def f(cb={'k': lambda x: x}):\n    pass\n"),
            vec![t("f", "def", 1, 2)]
        );
        assert_eq!(
            tags("def f():\n    d = {'a': 1}\n    if (n := len(d)) > 0:\n        return n\n"),
            vec![t("f", "def", 1, 4)]
        );
        assert_eq!(
            tags("def f(a={1: {2: 3}}, b=[1, 2], *, c: \"x\" = (1,)):\n    pass\n"),
            vec![t("f", "def", 1, 2)]
        );
        assert_eq!(
            tags("def f(x=f\"{1:>10}\"):\n    return x\n"),
            vec![t("f", "def", 1, 2)]
        );
    }

    /// probe `type_params_pep695`: `{'C': ('class', 5, 6), 'f': ('def', 1, 2)}`
    /// — the `[T]` type-parameter list is bracket-skipped like any other.
    #[test]
    fn pep695_type_parameter_lists_are_skipped() {
        assert_eq!(
            tags("def f[T](x: T) -> T:\n    return x\n\n\nclass C[T]:\n    pass\n"),
            vec![t("C", "class", 5, 6), t("f", "def", 1, 2)]
        );
    }

    /// probe `name_like_keywords`: `{'deffo': ('def', 6, 7)}` — `class_`,
    /// `define`, `defx` are ordinary names.
    #[test]
    fn keyword_prefixed_names_are_not_keywords() {
        assert_eq!(
            tags("class_ = 1\ndefine = 2\ndefx = 3\n\n\ndef deffo():\n    pass\n"),
            vec![t("deffo", "def", 6, 7)]
        );
    }

    /// probes `redefinition` `{'f': ('def', 5, 6)}` and
    /// `decorator_then_class_method` `{'C': ('class', 1, 8),
    /// 'C.p': ('def', 6, 8)}` — the dict keeps the LAST definition of a name.
    #[test]
    fn a_redefined_name_keeps_the_last_definition() {
        assert_eq!(
            tags("def f():\n    pass\n\n\ndef f():\n    return 2\n"),
            vec![t("f", "def", 5, 6)]
        );
        assert_eq!(
            tags(
                "class C:\n    @property\n    def p(self):\n        return 1\n\n\
                 \x20   @p.setter\n    def p(self, v):\n        self._p = v\n"
            ),
            vec![t("C", "class", 1, 8), t("C.p", "def", 6, 8)]
        );
    }

    /// probe `match_case`: `{'f': ('def', 1, 6)}` — soft keywords open plain
    /// `'other'` blocks.
    #[test]
    fn match_statements_are_plain_blocks() {
        assert_eq!(
            tags("def f(x):\n    match x:\n        case 1:\n            pass\n        case _:\n            pass\n"),
            vec![t("f", "def", 1, 6)]
        );
    }

    // ---- whitespace, endings, degenerate files ------------------------

    /// probes `tabs_indent` `{'f': ('def', 1, 2), 'g': ('def', 5, 6)}`,
    /// `tabs_mixed_8` `{'C': ('class', 1, 4), 'C.m': ('def', 2, 3)}` and
    /// `tab_and_spaces_same_block` `{'f': ('def', 1, 2)}` (an eight-space
    /// COMMENT line under a tab-indented body is fine — comment lines never
    /// take part in indentation).
    #[test]
    fn tab_indentation_works_like_cpythons() {
        assert_eq!(
            tags("def f():\n\treturn 1\n\n\ndef g():\n\tpass\n"),
            vec![t("f", "def", 1, 2), t("g", "def", 5, 6)]
        );
        assert_eq!(
            tags("class C:\n\tdef m(self):\n\t\tpass\n\tx = 1\n"),
            vec![t("C", "class", 1, 4), t("C.m", "def", 2, 3)]
        );
        assert_eq!(
            tags("def f():\n\tpass\n        # eight spaces comment\n"),
            vec![t("f", "def", 1, 2)]
        );
    }

    /// probe `ERR_taberror`: sphinx raises
    /// `TabError('inconsistent use of tabs and spaces in indentation', ...)`
    /// for a tab-indented class body continued with eight spaces — the two
    /// columns agree at tabsize 8 but disagree at tabsize 1.
    #[test]
    fn inconsistent_tabs_and_spaces_err() {
        let err =
            find_tags("class C:\n\tdef m(self):\n\t\tpass\n        x = 1\n").expect_err("TabError");
        assert_eq!(
            err.to_string(),
            "inconsistent use of tabs and spaces in indentation"
        );
    }

    /// probe `crlf`: `{'f': ('def', 1, 2), 'g': ('def', 5, 6)}` — identical
    /// to the LF file (the reader normalises endings before this anyway).
    #[test]
    fn crlf_line_endings_number_lines_the_same() {
        assert_eq!(
            tags("def f():\r\n    return 1\r\n\r\n\r\ndef g():\r\n    pass\r\n"),
            vec![t("f", "def", 1, 2), t("g", "def", 5, 6)]
        );
    }

    /// probes `no_trailing_newline` `{'f': ('def', 1, 2)}`,
    /// `no_trailing_newline_oneliner` `{'f': ('def', 1, 1)}` and
    /// `trailing_blank_at_eof` `{'f': ('def', 1, 2)}`.
    #[test]
    fn missing_and_surplus_trailing_newlines_both_land_on_the_last_code_line() {
        assert_eq!(tags("def f():\n    return 1"), vec![t("f", "def", 1, 2)]);
        assert_eq!(tags("def f(): return 1"), vec![t("f", "def", 1, 1)]);
        assert_eq!(
            tags("def f():\n    pass\n\n\n\n"),
            vec![t("f", "def", 1, 2)]
        );
    }

    /// probe `formfeed`: `{'f': ('def', 1, 2), 'g': ('def', 5, 6)}` —
    /// `filter_whitespace` turns the form feed into a space BEFORE the split,
    /// so line 3 is a blank line and gets trimmed.
    #[test]
    fn a_form_feed_becomes_a_blank_line_not_a_line_break() {
        assert_eq!(
            tags("def f():\n    pass\n\x0c\n\ndef g():\n    pass\n"),
            vec![t("f", "def", 1, 2), t("g", "def", 5, 6)]
        );
    }

    /// probes `no_definitions`, `empty`, `only_comments`, `only_blank_lines`
    /// — all `{}`.
    #[test]
    fn files_without_definitions_yield_no_tags() {
        assert!(tags("x = 1\ny = 2\nprint(x + y)\n").is_empty());
        assert!(tags("").is_empty());
        assert!(tags("# hello\n# world\n").is_empty());
        assert!(tags("\n\n\n").is_empty());
    }

    // ---- the error subset --------------------------------------------

    /// The failures this port DOES detect, each of which also fails sphinx's
    /// `ast.parse` (probes `ERR_unterminated_triple`, `ERR_unterminated_single`,
    /// `ERR_unclosed_bracket_then_def`, `ERR_inconsistent_dedent` — sphinx
    /// reports them as `parsing %r failed: SyntaxError(...)`, whose CPython
    /// repr tail this port cannot reproduce; see the module docs).
    #[test]
    fn scanner_level_failures_err_with_their_own_detail() {
        assert_eq!(
            find_tags("x = \"\"\"abc\ndef f():\n    pass\n")
                .expect_err("unterminated triple")
                .to_string(),
            "unterminated triple-quoted string literal (detected at line 3)"
        );
        assert_eq!(
            find_tags("x = 'abc\ndef f():\n    pass\n")
                .expect_err("unterminated string")
                .to_string(),
            "unterminated string literal (detected at line 1)"
        );
        assert_eq!(
            find_tags("x = [1,\ndef f():\n    pass\n")
                .expect_err("unclosed bracket")
                .to_string(),
            "'[' was never closed (opened at line 1)"
        );
        assert_eq!(
            find_tags("def f():\n        pass\n    x = 1\n")
                .expect_err("bad dedent")
                .to_string(),
            "unindent does not match any outer indentation level"
        );
    }

    /// The documented divergence, pinned so it cannot change silently: a file
    /// that TOKENIZES but does not PARSE yields tags here, while sphinx warns.
    /// probe `double_equals` — sphinx: `parsing '/abs/x.py' failed:
    /// SyntaxError('invalid syntax', ('<unknown>', 5, 5, 'x = = 1\n', 5, 6))`;
    /// here: the `f` tag. (`def f(:` is NOT such a case — its unclosed bracket
    /// fails both sides; probe `ERR_bad_syntax_tokenizable`.)
    #[test]
    fn a_tokenizable_but_unparsable_file_still_yields_tags_here() {
        assert_eq!(
            tags("def f():\n    return 1\n\n\nx = = 1\n"),
            vec![t("f", "def", 1, 2)]
        );
    }

    // ---- totality -----------------------------------------------------

    proptest::proptest! {
        /// Arbitrary text must never panic and must keep the tag invariants
        /// the reader's slice depends on: `1 <= start <= end`. `(?s)` is
        /// load-bearing — without it `.` excludes `\n` and the sweep never
        /// leaves a single logical line.
        #[test]
        fn arbitrary_source_never_panics(src in "(?s).{0,400}") {
            if let Ok(map) = find_tags(&src) {
                for (_, (_, start, end)) in map {
                    proptest::prop_assert!(start >= 1);
                    proptest::prop_assert!(start <= end);
                }
            }
        }

        /// The same, over sources built from Python-ish fragments, which reach
        /// far deeper into the scanner than random text does.
        #[test]
        fn python_shaped_fragments_never_panic(
            parts in proptest::collection::vec(
                proptest::sample::select(vec![
                    "def f():", "class C:", "@deco", "async def g(): pass", "    pass",
                    "\tpass", "x = (", ")", "\"\"\"", "'", "#", "\\", "  # c", "",
                    "def h(a,", "):", "if True:", "        deep", "\x0c", "\r",
                ]),
                0..24,
            )
        ) {
            let src = parts.join("\n");
            if let Ok(map) = find_tags(&src) {
                for (_, (_, start, end)) in map {
                    proptest::prop_assert!(start >= 1);
                    proptest::prop_assert!(start <= end);
                }
            }
        }
    }
}
