//! Scratch verification: line number of directive-raised system_message inside grid cells.
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
fn grid_cell_directive_error_lines() {
    let out = run("+------------------+\n| .. note::        |\n+------------------+\n");
    println!("=== note_in_cell_line2 ===\n{out}");

    let out = run(
        "+------------------+\n| x                |\n+------------------+\n| .. topic::       |\n+------------------+\n",
    );
    println!("=== topic_in_cell_line4 ===\n{out}");

    let out = run(
        ".. table::\n\n   +------------------+\n   | .. note::        |\n   +------------------+\n",
    );
    println!("=== table_directive_wrapped ===\n{out}");
}
