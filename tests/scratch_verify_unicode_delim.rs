//! Scratch verification: unicode_code decimal branch vs Python str.isdigit().
use sphinx_ultra::rst::{parse_rst, ParseOptions};

fn run(rst: &str) -> String {
    parse_rst(
        rst,
        &ParseOptions {
            source_path: "<test>".into(),
            sphinx: false,
            docname: "index".into(),
        },
    )
    .root
    .pformat()
}

#[test]
fn csv_table_delim_superscript_two() {
    let out = run(".. csv-table::\n   :delim: \u{00b2}\n\n   a,b\n");
    println!("=== superscript two ===\n{out}");
}

#[test]
fn csv_table_delim_arabic_indic_two() {
    let out = run(".. csv-table::\n   :delim: \u{0662}\n\n   a\u{0662}b\n");
    println!("=== arabic-indic two ===\n{out}");
}
