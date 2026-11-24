//! `ReferenceRole`'s split of the interpreted text into a title and a
//! target, and its disabled-reference marker
//! (`SP/sphinx/util/docutils.py:714-760`; [RC §3.16-3.17]).

use crate::doctree::unescape;

/// The null escape marker the inliner leaves before an escaped character
/// (docutils `escape2null`).
const NULL: char = '\u{0}';

/// What `ReferenceRole.__call__` makes of a role's text before the role
/// runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReferenceParts {
    /// `self.disabled = text.startswith('!')`, on the null-escaped text,
    /// so an escaped `\!` disables nothing.
    pub(crate) disabled: bool,
    /// `has_explicit_title`: the text is `title <target>`.
    pub(crate) explicit: bool,
    /// The link title, unescaped; the whole text when no title is given.
    pub(crate) title: String,
    /// The link target, unescaped; the whole text when no title is given.
    pub(crate) target: String,
}

impl ReferenceParts {
    /// `ReferenceRole.__call__` over `escaped`, the interpreted text with
    /// docutils' null escapes: `explicit_title_re`,
    /// `^(.+?)\s*(?<!\x00)<(.*?)>$` with DOTALL, matched on the escaped
    /// text ([`explicit_title`]), and both groups unescaped; with no
    /// match, the unescaped text is both title and target.
    pub(crate) fn parse(escaped: &str) -> ReferenceParts {
        let disabled = escaped.starts_with('!');
        match explicit_title(escaped) {
            Some((title, target)) => ReferenceParts {
                disabled,
                explicit: true,
                title,
                target,
            },
            None => {
                let text = unescape(escaped, false);
                ReferenceParts {
                    disabled,
                    explicit: false,
                    title: text.clone(),
                    target: text,
                }
            }
        }
    }
}

/// `ReferenceRole.explicit_title_re`, `^(.+?)\s*(?<!\x00)<(.*?)>$` with
/// DOTALL (`SP/sphinx/util/docutils.py:730`), over the null-escaped text,
/// as `(title, target)`, both unescaped: the first `<` that is not
/// escaped, has a title before it and opens a target the closing `>` of
/// the text ends. The lazy title gives its trailing whitespace (Python's
/// `\s`) to `\s*`, but keeps one character.
pub(crate) fn explicit_title(escaped: &str) -> Option<(String, String)> {
    let chars: Vec<char> = escaped.chars().collect();
    let n = chars.len();
    if n < 3 || chars[n - 1] != '>' {
        return None;
    }
    let open = (1..n - 1).find(|&j| chars[j] == '<' && chars[j - 1] != NULL)?;
    let mut title_end = open;
    while title_end > 1 && crate::utils::py_isspace(chars[title_end - 1]) {
        title_end -= 1;
    }
    let title: String = chars[..title_end].iter().collect();
    let target: String = chars[open + 1..n - 1].iter().collect();
    Some((unescape(&title, false), unescape(&target, false)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rst::inline::escape2null;

    fn parts(source_text: &str) -> ReferenceParts {
        ReferenceParts::parse(&escape2null(source_text))
    }

    fn explicit(title: &str, target: &str) -> ReferenceParts {
        ReferenceParts {
            disabled: false,
            explicit: true,
            title: title.to_string(),
            target: target.to_string(),
        }
    }

    fn implicit(text: &str) -> ReferenceParts {
        ReferenceParts {
            disabled: false,
            explicit: false,
            title: text.to_string(),
            target: text.to_string(),
        }
    }

    /// [RC §3.17] (probe P20, Sphinx 9.1.0) and the title/target probes
    /// of the reference roles: the title is the shortest prefix, the
    /// target everything between the first unescaped `<` after it and the
    /// final `>`; an escaped `\<` never opens a target, an escaped space
    /// before the `<` is gone with its marker, and a leading `!` (not an
    /// escaped one) disables the reference without leaving the text.
    #[test]
    fn reference_parts_parse() {
        for (source_text, expected) in [
            // P20: `:doc:` and the other reference roles.
            ("a <b> <c>", explicit("a", "b> <c")),
            ("x \\<y>", implicit("x <y>")),
            ("p\\ <q>", explicit("p", "q")),
            ("t <u(1)> <v(2)>", explicit("t", "u(1)> <v(2)")),
            ("n <m <o>>", explicit("n", "m <o>")),
            // The title loses its trailing whitespace; the target keeps
            // its padding (`:term:`x < foo   bar >``).
            ("x < foo   bar >", explicit("x", " foo   bar ")),
            ("Title  <target>", explicit("Title", "target")),
            ("a\n<b>", explicit("a", "b")),
            // An empty target, and no title at all.
            ("t <>", explicit("t", "")),
            ("<b>", implicit("<b>")),
            ("b>", implicit("b>")),
            ("plain", implicit("plain")),
            // An escaped space before the `<` vanishes with its marker,
            // and the title it ended keeps its own trailing space.
            ("a \\ <x>", explicit("a ", "x")),
            ("\\ <x>", explicit("", "x")),
            // Escapes are gone from both halves.
            ("a\\*b <c\\*d>", explicit("a*b", "c*d")),
        ] {
            assert_eq!(parts(source_text), expected, "{source_text:?}");
        }
        // `!` disables the reference; the text is parsed all the same.
        for (source_text, disabled) in [("!label", true), ("!t <x>", true), ("\\!x", false)] {
            assert_eq!(parts(source_text).disabled, disabled, "{source_text:?}");
        }
        assert_eq!(
            parts("!t <x>"),
            ReferenceParts {
                disabled: true,
                ..explicit("!t", "x")
            }
        );
        assert_eq!(parts("\\!x"), implicit("!x"));
    }
}
