//! Scratch verification: line number of system_message inside csv-table cells.
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
fn csv_cell_message_linenos() {
    for (name, src) in [
        ("basic", ".. csv-table::\n\n   \"*bad\", ok\n"),
        ("second_row", ".. csv-table::\n\n   a, b\n   \"*bad\", ok\n"),
        ("multiline", ".. csv-table::\n\n   \"line1\n   *bad\", ok\n"),
        (
            "block_directive_cell",
            ".. csv-table::\n\n   \".. topic:: T\", b\n",
        ),
        (
            "preceding_para",
            "para\n\npara2\n\n.. csv-table::\n\n   \"*bad\", ok\n",
        ),
        (
            "with_options",
            ".. csv-table::\n   :header-rows: 0\n\n   \"*bad\", ok\n",
        ),
    ] {
        println!("==================== {name}\n{}", run(src));
    }
}
