//! Property tests for the M2 RST parser (ROADMAP §10.6: the parser is total
//! — it never panics on arbitrary input; problems become system_message
//! nodes). First real use of the reserved proptest dev-dependency.

use proptest::prelude::*;
use sphinx_ultra::rst::{parse_rst, ParseOptions};

fn opts() -> ParseOptions {
    ParseOptions {
        source_path: "<p>".into(),
        sphinx: true,
        docname: "index".into(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn parse_never_panics_on_arbitrary_input(s in "\\PC*") {
        let _ = parse_rst(&s, &opts());
    }

    #[test]
    fn parse_never_panics_on_rst_shaped_input(
        s in proptest::collection::vec(
            prop_oneof![
                Just("Title\n=====\n".to_string()),
                Just("=====\nOver\n=====\n".to_string()),
                Just("- item\n".to_string()),
                Just("-\n".to_string()),
                Just("1. item\n".to_string()),
                Just("(i) item\n".to_string()),
                Just("#. item\n".to_string()),
                Just("   indented\n".to_string()),
                Just("  half\n".to_string()),
                Just("::\n".to_string()),
                Just("para::\n".to_string()),
                Just(".. _t:\n".to_string()),
                Just(".. _t: uri\n".to_string()),
                Just(".. comment\n".to_string()),
                Just("..\n".to_string()),
                Just("__ uri\n".to_string()),
                Just("| line\n".to_string()),
                Just("|\n".to_string()),
                Just(">>> code\n".to_string()),
                Just("term\n    def\n".to_string()),
                Just("term : c\n    def\n".to_string()),
                Just("*emph* and ``lit`` text\n".to_string()),
                Just("*unclosed here\n".to_string()),
                Just("`phrase ref`_ and word_ and anon__\n".to_string()),
                Just(":role:`text` and `bare`\n".to_string()),
                Just(":bogus:`x` end\n".to_string()),
                Just("[1]_ [#]_ [*]_ [cite]_\n".to_string()),
                Just("|sub| and |sub2|__\n".to_string()),
                Just("https://example.com/ and foo@bar.example\n".to_string()),
                Just(".. [1] footnote\n".to_string()),
                Just(".. [#lbl] auto\n".to_string()),
                Just(":field: value\n".to_string()),
                Just("-a  option desc\n".to_string()),
                Just("+----+----+\n".to_string()),
                Just("| A  | B  |\n".to_string()),
                Just("+====+====+\n".to_string()),
                Just("=====  =====\n".to_string()),
                Just("A      B\n".to_string()),
                Just("_`inline target` here\n".to_string()),
                Just("`text <https://x/>`_ ref\n".to_string()),
                Just("----\n".to_string()),
                Just("---\n".to_string()),
                Just("-- attribution\n".to_string()),
                Just("\n".to_string()),
                Just("\t\ttabs\n".to_string()),
                Just("> quoted\n".to_string()),
                Just("text\n".to_string()),
            ], 0..40).prop_map(|v| v.concat())
    ) {
        let _ = parse_rst(&s, &opts());
    }

    #[test]
    fn parse_handles_multibyte_boundaries(s in "[αβ✓🎉a\\-=\\n \\|•‣⁃ß]{0,200}") {
        let _ = parse_rst(&s, &opts());
    }

    #[test]
    fn pformat_never_panics_after_parse(s in "\\PC{0,300}") {
        let tree = parse_rst(&s, &opts());
        let _ = tree.root.pformat();
    }

    #[test]
    fn parse_never_panics_on_multiline_arbitrary_input(
        v in proptest::collection::vec("\\PC{0,40}", 0..30)
    ) {
        let s = v.join("\n");
        let _ = parse_rst(&s, &opts());
    }
}

#[test]
fn deep_nesting_does_not_overflow_stack() {
    // 6000 levels of nested bullet lists: far past the MAX_NEST_DEPTH guard
    // (200), which drops deeper content with an ERROR message instead of
    // overflowing the stack (docutils crashes with RecursionError here).
    let mut s = String::new();
    for depth in 0..6000 {
        s.push_str(&"  ".repeat(depth));
        s.push_str("- x\n");
    }
    let tree = parse_rst(&s, &opts());
    let out = tree.root.pformat();
    assert_eq!(
        out.matches("Maximum nesting depth exceeded; deeper content skipped.")
            .count(),
        1,
        "depth guard must fire exactly once"
    );
}

#[test]
fn pathological_wide_inputs() {
    // Very long single lines and very many siblings.
    let long_line = "x".repeat(100_000);
    let _ = parse_rst(&long_line, &opts());
    let adornment = "=".repeat(100_000);
    let _ = parse_rst(&format!("{long_line}\n{adornment}\n"), &opts());
    let many_paras = "para\n\n".repeat(20_000);
    let _ = parse_rst(&many_paras, &opts());
    let many_targets = ".. _t:\n".repeat(5_000);
    let _ = parse_rst(&many_targets, &opts());
}
