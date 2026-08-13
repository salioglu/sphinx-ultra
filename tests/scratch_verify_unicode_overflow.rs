//! Scratch verification: unicode_code overflow error text vs docutils OverflowError detail.
use sphinx_ultra::rst::{parse_rst, ParseOptions};

fn run(rst: &str) -> String {
    parse_rst(
        rst,
        &ParseOptions {
            source_path: "<snippet>".into(),
            sphinx: false,
            docname: "index".into(),
        },
    )
    .root
    .pformat()
}

#[test]
fn unicode_code_overflow_texts() {
    for (name, delim) in [
        ("decimal_2_32", "4294967296"),
        ("decimal_huge", "99999999999999999999"),
        ("hex_huge", "0xffffffffffffffffffff"),
        ("decimal_2_31", "2147483648"),
        ("decimal_in_c_int_but_over_unicode", "2000000000"),
    ] {
        let src = format!(".. csv-table::\n   :delim: {delim}\n\n   a,b\n");
        println!("==================== {name}\n{}", run(&src));
    }
}
