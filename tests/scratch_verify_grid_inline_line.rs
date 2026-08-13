//! Scratch verification: line number of inline-markup system_message inside grid cell.
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
fn grid_cell_inline_message_line() {
    let out = run("+------------------+\n| *bad             |\n+------------------+\n");
    println!("=== grid-cell-inline-message ===\n{out}");
}
