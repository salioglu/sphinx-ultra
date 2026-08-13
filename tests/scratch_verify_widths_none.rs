//! Scratch verification: empty :widths: option value error detail.
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
fn widths_none_csv_table() {
    let out = run(".. csv-table::\n   :widths:\n\n   a,b\n");
    println!("=== csv-table empty widths ===\n{out}");
    let out = run(".. list-table::\n   :widths:\n\n   * - a\n     - b\n");
    println!("=== list-table empty widths ===\n{out}");
    let out = run(".. table::\n   :widths:\n\n   ==  ==\n   a   b\n   ==  ==\n");
    println!("=== table empty widths ===\n{out}");
}
