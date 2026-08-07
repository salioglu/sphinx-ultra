//! Line preprocessing: docutils `statemachine.string2lines(tab_width=8,
//! convert_whitespace=True)` equivalent, with byte-span mapping back to the
//! original source (docutils loses this; we keep it for node spans).
//!
//! Probe-verified semantics (docutils 0.22.4):
//! - `\v` / `\f` become single spaces (convert_whitespace).
//! - Lines split on `\n`, `\r\n`, `\r` (plus Python `splitlines` exotics:
//!   `\x1c`-`\x1e`, `\u{85}`, `\u{2028}`, `\u{2029}`).
//! - Tabs expand to the next multiple-of-8 column (character columns).
//! - Trailing whitespace stripped per line.
//! - One processed line == one source line, so message line numbers are
//!   `processed index + 1`.

use std::ops::Deref;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessedLine {
    pub text: String,
    /// Byte range of the original line content (terminator excluded).
    pub src_start: u32,
    pub src_end: u32,
}

impl ProcessedLine {
    /// Leading-space count (text is already tab-expanded and rstripped).
    pub fn indent(&self) -> usize {
        self.text.len() - self.text.trim_start_matches(' ').len()
    }

    pub fn is_blank(&self) -> bool {
        self.text.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Lines(Vec<ProcessedLine>);

impl Deref for Lines {
    type Target = [ProcessedLine];

    fn deref(&self) -> &[ProcessedLine] {
        &self.0
    }
}

fn is_line_boundary(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

fn process_line(raw: &str) -> String {
    // convert_whitespace (\v, \f -> space), then expandtabs(8), then rstrip.
    let mut expanded = String::with_capacity(raw.len());
    let mut col = 0usize;
    for c in raw.chars() {
        match c {
            '\t' => {
                let next_stop = (col / 8 + 1) * 8;
                for _ in col..next_stop {
                    expanded.push(' ');
                }
                col = next_stop;
            }
            '\x0b' | '\x0c' => {
                expanded.push(' ');
                col += 1;
            }
            _ => {
                expanded.push(c);
                col += 1;
            }
        }
    }
    expanded.trim_end().to_string()
}

impl Lines {
    pub fn new(source: &str) -> Lines {
        let mut lines = Vec::new();
        let bytes_len = source.len();
        let mut line_start = 0usize;
        let mut chars = source.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if is_line_boundary(c) {
                lines.push(ProcessedLine {
                    text: process_line(&source[line_start..i]),
                    src_start: line_start as u32,
                    src_end: i as u32,
                });
                // \r\n is one boundary
                if c == '\r' {
                    if let Some(&(_, '\n')) = chars.peek() {
                        chars.next();
                    }
                }
                line_start = match chars.peek() {
                    Some(&(j, _)) => j,
                    None => bytes_len,
                };
            }
        }
        if line_start < bytes_len {
            lines.push(ProcessedLine {
                text: process_line(&source[line_start..]),
                src_start: line_start as u32,
                src_end: bytes_len as u32,
            });
        }
        Lines(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_expand_to_8_col_stops() {
        assert_eq!(Lines::new("a\tb")[0].text, "a       b");
        assert_eq!(Lines::new("\ta")[0].text, "        a");
        assert_eq!(Lines::new("ab\tc")[0].text, "ab      c");
        assert_eq!(Lines::new("abcdefgh\tz")[0].text, "abcdefgh        z");
        assert_eq!(Lines::new("x\ty\tz")[0].text, "x       y       z");
    }

    #[test]
    fn trailing_whitespace_stripped_and_spans_map_to_source() {
        let src = "one  \ntwo";
        let l = Lines::new(src);
        assert_eq!(l[0].text, "one");
        assert_eq!(
            &src[l[0].src_start as usize..l[0].src_end as usize],
            "one  "
        );
        assert_eq!(l[1].text, "two");
        assert_eq!(&src[l[1].src_start as usize..l[1].src_end as usize], "two");
    }

    #[test]
    fn vertical_tab_and_formfeed_become_spaces() {
        assert_eq!(Lines::new("a\x0bb\x0cc")[0].text, "a b c");
    }

    #[test]
    fn crlf_and_cr_split_without_stray_cr() {
        let l = Lines::new("one\r\ntwo\rthree");
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].text, "one");
        assert_eq!(l[1].text, "two");
        assert_eq!(l[2].text, "three");
    }

    #[test]
    fn trailing_newline_produces_no_empty_last_line() {
        assert_eq!(Lines::new("a\n").len(), 1);
        let l = Lines::new("a\n\n");
        assert_eq!(l.len(), 2);
        assert!(l[1].is_blank());
    }

    #[test]
    fn indent_counts_leading_spaces() {
        let l = Lines::new("    four\n\tone-tab");
        assert_eq!(l[0].indent(), 4);
        assert_eq!(l[1].indent(), 8);
    }
}
