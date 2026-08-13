//! Scratch verification: math directive with :name: and multiple blank-line-separated blocks.
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
fn math_multi_block_name() {
    let out = run(".. math::\n   :name: eq\n   :class: mc\n\n   a\n\n   b\n");
    println!("=== math-multi-block-name ===\n{out}");
    let out2 = run(".. math::\n   :name: Eq One\n\n   a\n\n   b\n");
    println!("=== math-multi-block-name-normalization ===\n{out2}");
}
