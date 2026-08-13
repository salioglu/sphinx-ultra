//! Scratch verification: figure with a pending child (.. class:: without content).
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
fn figure_pending_children() {
    for (name, rst) in [
        (
            "pending_then_caption",
            ".. figure:: p.png\n\n   .. class:: cls\n\n   Caption.\n",
        ),
        (
            "pending_target_caption",
            ".. figure:: p.png\n\n   .. class:: cls\n\n   .. _tgt:\n\n   Caption.\n",
        ),
        ("pending_only", ".. figure:: p.png\n\n   .. class:: cls\n"),
    ] {
        println!("=== {name} ===\n{}", run(rst));
    }
}
