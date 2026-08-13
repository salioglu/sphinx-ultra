//! Scratch verification: big-integer measures through get_measure (i64 overflow path).
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
fn bigint_width_measure() {
    let out = run(".. image:: x.png\n   :width: 99999999999999999999px\n");
    println!("=== bigint-width ===\n{out}");
}
