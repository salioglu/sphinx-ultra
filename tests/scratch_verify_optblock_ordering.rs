//! Scratch verification: option-block error ordering (multi-word name vs invalid block).
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
fn optblock_multiword_then_invalid_line() {
    let out = run(".. image:: x.png\n   :a b: 2\n   notafield\n");
    println!("=== optblock-ordering ===\n{out}");
}
