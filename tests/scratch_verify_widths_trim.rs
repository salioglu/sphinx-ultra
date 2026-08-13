//! Scratch verification: positive_int_list part trimming in :widths: error repr.
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
fn widths_trim_error_repr() {
    let out = run(".. csv-table::\n   :widths: 1, x\n\n   a,b\n");
    println!("=== csv_widths_comma_space_bad ===\n{out}");
    let out = run(".. table::\n   :widths: 1, x\n\n   ===  ===\n   a    b\n   ===  ===\n");
    println!("=== table_widths_comma_space_bad ===\n{out}");
    let out = run(".. csv-table::\n   :widths: 1,  2 \n\n   a,b\n");
    println!("=== csv_widths_ws_ok ===\n{out}");
}
