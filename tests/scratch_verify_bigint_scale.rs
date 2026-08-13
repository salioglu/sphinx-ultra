//! Scratch verification: big integer option values (py_int i64 cap claim).
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
fn bigint_scale() {
    let out = run(".. image:: x.png\n   :scale: 99999999999999999999\n");
    println!("=== bigint-scale ===\n{out}");
}
