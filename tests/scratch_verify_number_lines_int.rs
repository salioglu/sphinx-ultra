//! Scratch verification: :number-lines: Python int() edge cases.
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
fn number_lines_int_edges() {
    for (label, src) in [
        (
            "underscore",
            ".. code::\n   :number-lines: 1_0\n\n   x\n   y\n",
        ),
        (
            "arabic-indic",
            ".. code::\n   :number-lines: \u{661}\u{660}\n\n   x\n   y\n",
        ),
        (
            "bigint",
            ".. code::\n   :number-lines: 99999999999999999999\n\n   x\n   y\n",
        ),
    ] {
        println!("=== {label} ===\n{}", run(src));
    }
}
