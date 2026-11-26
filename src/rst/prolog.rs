//! Sphinx's `rst_prolog` and `rst_epilog` ([RC §4.1, §7.5]).
//!
//! Sphinx's reStructuredText parser edits a document's input lines before
//! the state machine runs (`RSTParser.decorate`,
//! `SP/sphinx/parsers.py:102-105`): `_prepend_prologue` puts the prolog's
//! lines after the document's leading field-marker lines, and
//! `_append_epilogue` puts the epilog's after its last line
//! (`SP/sphinx/util/rst.py:80-115`). Each inserted line keeps a source of
//! its own, `<rst_prologue>`, `<rst_epilogue>` or `<generated>`, with its
//! offset there, so a reporter message about one prints that source and
//! line, and the document's own lines keep their numbers. Only what the
//! state machine counts itself, its `abs_line_number()`, moves: a
//! directive's `self.lineno`, which a few Sphinx warnings print as is
//! ([`super::block`]'s `abs_line_number`), and a hyperlink target's line.
//!
//! Sphinx inserts the prolog's lines as `str.splitlines()` gives them,
//! without docutils' `string2lines` (tab expansion, right-stripping), and
//! so does this module ([`Sources::add_source`]): a tab stays a tab, which
//! indents nothing, and trailing whitespace stays in the text.

use super::lines::LineRec;

/// The source docutils names a prolog line by (`rst.py:99`).
const PROLOGUE_SOURCE: &str = "<rst_prologue>";
/// The source docutils names an epilog line by (`rst.py:115`).
const EPILOGUE_SOURCE: &str = "<rst_epilogue>";
/// The source of the blank lines `_prepend_prologue` adds, and of the one
/// `_append_epilogue` adds to an empty input (`rst.py:93`, `:101`, `:111`).
const GENERATED_SOURCE: &str = "<generated>";

/// The text Sphinx's reader adds around a document: the configuration's
/// `rst_prolog` and `rst_epilog`. An empty string adds nothing, as `None`
/// does (`if not prologue: return`).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Decoration<'a> {
    pub(crate) prolog: Option<&'a str>,
    pub(crate) epilog: Option<&'a str>,
}

impl<'a> Decoration<'a> {
    /// The decoration `read` configures.
    pub(crate) fn of(read: &'a super::ReadConfig) -> Self {
        Decoration {
            prolog: read.rst_prolog.as_deref(),
            epilog: read.rst_epilog.as_deref(),
        }
    }
}

/// What the decoration reads and grows: the parser's source table.
pub(crate) trait Sources {
    /// The text of `line` in its source.
    fn line_text(&self, line: LineRec) -> &str;
    /// Adds a source named `path` whose lines are `lines`, as they are
    /// (not processed as a document's lines are), numbered from 1: its id
    /// and one record per line, or `None` once the table cannot take
    /// another source.
    fn add_source(&mut self, path: &str, lines: &[&str]) -> Option<(u16, Vec<LineRec>)>;
}

/// `RSTParser.decorate` over `top`, the document's line stream, adding the
/// sources the inserted lines come from to `sources`.
pub(crate) fn decorate(top: &mut Vec<LineRec>, sources: &mut impl Sources, decoration: Decoration) {
    // A blank line's record: a zero-width view, valid at offset 0 of any
    // text (the `<generated>` source's is empty) or at a line's end.
    let blank = |source: u16, lineno: u32, at: u32| LineRec::new(source, lineno, at, at, "");

    if let Some(prolog) = decoration.prolog.filter(|text| !text.is_empty()) {
        // `_prepend_prologue`: after the leading lines that match the
        // field-marker pattern (`rst.py:28`, `Body.patterns
        // ['field_marker']`); a field body's continuation line ends them.
        let pos = top
            .iter()
            .take_while(|line| super::block::field_marker(sources.line_text(**line)).is_some())
            .count();
        let Some((generated, _)) = sources.add_source(GENERATED_SOURCE, &[]) else {
            return;
        };
        let Some((_, lines)) =
            sources.add_source(PROLOGUE_SOURCE, &crate::utils::py_splitlines(prolog))
        else {
            return;
        };
        let mut inserted = Vec::with_capacity(lines.len() + 2);
        // `content.insert(pos, '', '<generated>', 0)` after docinfo.
        if pos > 0 {
            inserted.push(blank(generated, 1, 0));
        }
        inserted.extend(lines);
        inserted.push(blank(generated, 1, 0));
        top.splice(pos..pos, inserted);
    }

    if let Some(epilog) = decoration.epilog.filter(|text| !text.is_empty()) {
        // `_append_epilogue`: a blank line numbered after the last line,
        // in its source, or after a `<generated>` offset 0 when there is
        // no line at all (a prologue leaves one).
        let after_last = match top.last() {
            Some(last) => blank(last.source, last.lineno + 1, last.end),
            None => match sources.add_source(GENERATED_SOURCE, &[]) {
                Some((generated, _)) => blank(generated, 2, 0),
                None => return,
            },
        };
        let Some((_, lines)) =
            sources.add_source(EPILOGUE_SOURCE, &crate::utils::py_splitlines(epilog))
        else {
            return;
        };
        top.push(after_last);
        top.extend(lines);
    }
}

#[cfg(test)]
mod tests {
    use crate::rst::{parse_rst_full, LogLocation, ParseOptions, ReadConfig};

    /// The decorated input of a Sphinx-mode read of `doc` under `prolog`
    /// and `epilog`, as `(source, offset)` pairs: the top-level input the
    /// parse leaves behind, each line named by its source's path and by
    /// docutils' 0-based offset in it, which is its line less one.
    fn decorated(doc: &str, prolog: Option<&str>, epilog: Option<&str>) -> Vec<(String, u32)> {
        let output = parse_rst_full(
            doc,
            &ParseOptions {
                source_path: "<doc>".to_string(),
                sphinx: true,
                read: ReadConfig {
                    rst_prolog: prolog.map(str::to_string),
                    rst_epilog: epilog.map(str::to_string),
                    ..ReadConfig::default()
                },
                ..ParseOptions::default()
            },
        );
        output
            .state
            .input_lines
            .iter()
            .map(|&(source, line)| {
                (
                    output.doctree.sources[usize::from(source)].clone(),
                    line - 1,
                )
            })
            .collect()
    }

    fn rows(rows: &[(&str, u32)]) -> Vec<(String, u32)> {
        rows.iter()
            .map(|&(source, offset)| (source.to_string(), offset))
            .collect()
    }

    /// `RSTParser.decorate` (`SP/sphinx/parsers.py:102-105`): the five
    /// decorated `StringList`s of [RC §4.1], probed on Sphinx 9.1.0 with
    /// `_prepend_prologue` and `_append_epilogue` over the document's
    /// `string2lines` (scratch `t17b/decorate.out`). The prologue goes
    /// after the leading field-marker lines, between `<generated>` blank
    /// lines (the first only after such lines); the epilogue after a blank
    /// line numbered after the document's last, in its source, or a
    /// `<generated>` line 1 when the input is empty. The document's own
    /// lines keep their numbers.
    #[test]
    fn prolog_epilog_splice_keeps_document_lines() {
        const PROLOG: &str = ".. |psub| replace:: prolog text\n\n*unclosed prolog emphasis\n";
        const EPILOG: &str = ".. |esub| replace:: epilog text\n\n*unclosed epilog emphasis\n";
        let prologue = [
            ("<rst_prologue>", 0),
            ("<rst_prologue>", 1),
            ("<rst_prologue>", 2),
        ];
        let epilogue = [
            ("<rst_epilogue>", 0),
            ("<rst_epilogue>", 1),
            ("<rst_epilogue>", 2),
        ];
        let table = |head: &[(&'static str, u32)], body: &[(&'static str, u32)]| {
            let mut all = head.to_vec();
            all.extend_from_slice(body);
            rows(&all)
        };

        // plain
        let mut body = prologue.to_vec();
        body.push(("<generated>", 0));
        body.extend([("<doc>", 0), ("<doc>", 1), ("<doc>", 2), ("<doc>", 3)]);
        body.push(("<doc>", 4));
        body.extend(epilogue);
        assert_eq!(
            decorated(
                "Title\n=====\n\nBody |psub| |esub|.\n",
                Some(PROLOG),
                Some(EPILOG)
            ),
            rows(&body),
            "plain"
        );

        // docinfo: a field body's continuation line stops the count, so
        // the prologue cuts the field in two.
        let head = [("<doc>", 0), ("<doc>", 1), ("<generated>", 0)];
        let mut body = prologue.to_vec();
        body.push(("<generated>", 0));
        body.extend([("<doc>", 2), ("<doc>", 3), ("<doc>", 4), ("<doc>", 5)]);
        body.extend(epilogue);
        assert_eq!(
            decorated(
                ":author: Me\n:date: 2020\n  cont\nTitle\n=====\n",
                Some(PROLOG),
                Some(EPILOG)
            ),
            table(&head, &body),
            "docinfo"
        );

        // orphan
        let head = [("<doc>", 0), ("<generated>", 0)];
        let mut body = prologue.to_vec();
        body.push(("<generated>", 0));
        body.extend([("<doc>", 1), ("<doc>", 2), ("<doc>", 3), ("<doc>", 4)]);
        body.extend(epilogue);
        assert_eq!(
            decorated(":orphan:\n\nTitle\n=====\n", Some(PROLOG), Some(EPILOG)),
            table(&head, &body),
            "orphan"
        );

        // empty-doc: the epilogue's blank line follows the prologue's last
        // `<generated>` line.
        let mut body = prologue.to_vec();
        body.extend([("<generated>", 0), ("<generated>", 1)]);
        body.extend(epilogue);
        assert_eq!(
            decorated("", Some(PROLOG), Some(EPILOG)),
            rows(&body),
            "empty-doc"
        );

        // prolog-no-trailing-newline, no epilogue
        assert_eq!(
            decorated("Body.\n", Some("line one\nline two"), Some("")),
            rows(&[
                ("<rst_prologue>", 0),
                ("<rst_prologue>", 1),
                ("<generated>", 0),
                ("<doc>", 0),
            ]),
            "prolog-no-trailing-newline"
        );

        // An empty input with no prologue: `_append_epilogue` numbers its
        // blank line after a `<generated>` offset 0 (`rst.py:110-113`).
        let mut body = vec![("<generated>", 1)];
        body.extend(epilogue);
        assert_eq!(
            decorated("", None, Some(EPILOG)),
            rows(&body),
            "empty-doc, epilogue only"
        );
    }

    /// The lines `message` is logged at, in a Sphinx-mode read of `doc`
    /// with `opts`.
    fn logged_at(message: &str, doc: &str, opts: ParseOptions) -> Vec<LogLocation> {
        let output = parse_rst_full(
            doc,
            &ParseOptions {
                sphinx: true,
                ..opts
            },
        );
        output
            .registry
            .log_warnings
            .iter()
            .filter(|warning| warning.message == message)
            .map(|warning| warning.location)
            .collect()
    }

    /// The lines `.. hlist content is not a list` is logged at, in a
    /// Sphinx-mode read of `doc` with `opts`.
    fn hlist_warning_lines(doc: &str, opts: ParseOptions) -> Vec<LogLocation> {
        logged_at(".. hlist content is not a list", doc, opts)
    }

    fn prolog_opts(prolog: Option<&str>, epilog: Option<&str>) -> ParseOptions {
        ParseOptions {
            read: ReadConfig {
                rst_prolog: prolog.map(str::to_string),
                rst_epilog: epilog.map(str::to_string),
                ..ReadConfig::default()
            },
            ..ParseOptions::default()
        }
    }

    /// Sphinx's `HList` logs `.. hlist content is not a list` at
    /// `location=(docname, self.lineno)` (`SP/sphinx/directives/other.py:291-294`),
    /// the state machine's `abs_line_number()`: the directive's line in the
    /// decorated input, every inclusion's lines spliced in, which no
    /// reporter maps back to a line of its file. A 3-line prologue moves it
    /// down 4 lines, and one after `:orphan:` 5 ([RC §4.1] P18); the
    /// prologue's and the epilogue's own lines and an inclusion's lines
    /// count the same way (Sphinx 9.1.0 CLI probes, scratch
    /// `t17b/specs/p18*.json`, `hlist_in_*.json`, `hlist_after_include.json`).
    #[test]
    fn raw_lineno_sites_shift_by_the_prolog() {
        const DOC: &str = "T\n=\n\n.. hlist::\n\n   not a list\n\nSee :external:py:func:`f`.\n";
        const PROLOG3: &str = ".. |a| replace:: A\n.. |b| replace:: B\n.. |c| replace:: C\n";
        assert_eq!(
            hlist_warning_lines(DOC, prolog_opts(None, None)),
            [LogLocation::DocLine(4)],
            "no prolog"
        );
        assert_eq!(
            hlist_warning_lines(DOC, prolog_opts(Some(PROLOG3), None)),
            [LogLocation::DocLine(8)],
            "a 3-line prolog"
        );
        assert_eq!(
            hlist_warning_lines(
                &format!(":orphan:\n\n{DOC}"),
                prolog_opts(Some(PROLOG3), None)
            ),
            [LogLocation::DocLine(11)],
            "a 3-line prolog after :orphan:"
        );
        assert_eq!(
            hlist_warning_lines(
                "T\n=\n\nText.\n",
                prolog_opts(Some(".. hlist::\n\n   not a list\n"), None)
            ),
            [LogLocation::DocLine(1)],
            "in the prologue"
        );
        assert_eq!(
            hlist_warning_lines(
                "T\n=\n\nText.\n",
                prolog_opts(None, Some("\n.. hlist::\n\n   not a list\n"))
            ),
            [LogLocation::DocLine(7)],
            "in the epilogue"
        );

        // After an inclusion: its five lines, docutils' two end-of-inclusion
        // lines and the two padding lines come before the directive.
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("part.inc"), "One.\n\nTwo.\n\nThree.\n").unwrap();
        assert_eq!(
            hlist_warning_lines(
                "T\n=\n\n.. include:: part.inc\n\n.. hlist::\n\n   not a list\n",
                ParseOptions {
                    source_path: dir.path().join("index.rst").display().to_string(),
                    srcdir: Some(dir.path().to_path_buf()),
                    ..ParseOptions::default()
                }
            ),
            [LogLocation::DocLine(15)],
            "after an inclusion"
        );
    }

    /// Sphinx's `Acks` logs `.. acks content is not a list` as `HList` logs
    /// its warning, at `location=(docname, self.lineno)`
    /// (`SP/sphinx/directives/other.py:268-271`): the directive's line in
    /// the decorated input, 4 lines down after a 3-line prologue and 5 when
    /// the prologue follows `:orphan:` ([RC §4.1] P18).
    //
    // oracle: Sphinx 9.1.0 through tools/gen_sphinx_fixture.py's harness,
    //   each row with its `rst_prolog` (scratchpad/t18c/probes/sreadc.py
    //   over cases2.json): `index.rst:4`, `:8` and `:11`.
    #[test]
    fn acks_warning_shifts_with_the_prolog() {
        const MESSAGE: &str = ".. acks content is not a list";
        const DOC: &str = "T\n=\n\n.. acks::\n\n   not a list\n";
        const PROLOG3: &str = ".. |a| replace:: A\n.. |b| replace:: B\n.. |c| replace:: C\n";
        assert_eq!(
            logged_at(MESSAGE, DOC, prolog_opts(None, None)),
            [LogLocation::DocLine(4)],
            "no prolog"
        );
        assert_eq!(
            logged_at(MESSAGE, DOC, prolog_opts(Some(PROLOG3), None)),
            [LogLocation::DocLine(8)],
            "a 3-line prolog"
        );
        assert_eq!(
            logged_at(
                MESSAGE,
                &format!(":orphan:\n\n{DOC}"),
                prolog_opts(Some(PROLOG3), None)
            ),
            [LogLocation::DocLine(11)],
            "a 3-line prolog after :orphan:"
        );
    }

    /// The raw line of a directive on a line the top-level input does not
    /// hold is its own line, whether or not a glossary definition holds it:
    /// the one rule a node's location is counted by too
    /// (`block::counted_line`). Sphinx 9.1.0 counts such a line in the
    /// definition's own list, inclusions spliced in, and prints `:11` for
    /// both inputs here, an `hlist` in a table cell in a file an `include`
    /// in a glossary definition reads, and one in a file an `include` in a
    /// table cell in a definition reads (scratch
    /// `t17b-fix1/specs/glossary_*.json`): the accepted divergence
    /// "Glossary definitions, one class", which counts these lines by
    /// their own line (`part.inc` 2 and 1).
    #[test]
    fn a_line_the_input_does_not_hold_keeps_its_own_line() {
        let cell = |content: &str| {
            format!(
                "+-----------------+\n| {content:<15} |\n|                 |\n\
                 |    not a list   |\n+-----------------+\n"
            )
        };
        let hlist_in_a_cell = cell(".. hlist::");
        for (name, index, part, line) in [
            (
                "a cell in an included file",
                "T\n=\n\n.. glossary::\n\n   term\n      .. include:: part.inc\n".to_string(),
                hlist_in_a_cell.as_str(),
                2,
            ),
            (
                "an included file in a cell",
                "T\n=\n\n.. glossary::\n\n   term\n      \
                 +--------------------------+\n      | .. include:: part.inc    |\n      \
                 +--------------------------+\n"
                    .to_string(),
                ".. hlist::\n\n   not a list\n",
                1,
            ),
        ] {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("part.inc"), part).unwrap();
            assert_eq!(
                hlist_warning_lines(
                    &index,
                    ParseOptions {
                        source_path: dir.path().join("index.rst").display().to_string(),
                        srcdir: Some(dir.path().to_path_buf()),
                        ..ParseOptions::default()
                    }
                ),
                [LogLocation::DocLine(line)],
                "{name}"
            );
        }
    }
}
