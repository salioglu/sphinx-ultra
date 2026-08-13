//! Scratch verification: image/figure :target: with non-simplename body ending in `_`.
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
fn image_target_uri_trailing_underscore() {
    let out = run(".. image:: x.png\n   :target: http://x.com/a_\n");
    println!("=== image-url-underscore ===\n{out}");
    let out2 = run(".. image:: x.png\n   :target: a._\n");
    println!("=== image-dot-underscore ===\n{out2}");
    let out3 = run(".. figure:: x.png\n   :target: http://x.com/a_\n");
    println!("=== figure-url-underscore ===\n{out3}");
    let out4 = run(".. image:: x.png\n   :target: abc_\n");
    println!("=== image-simplename ===\n{out4}");
}

#[test]
fn explicit_target_uri_trailing_underscore() {
    let out = run(".. _t: http://x.com/a_\n");
    println!("=== explicit-target-url-underscore ===\n{out}");
    let out2 = run("__ http://x.com/a_\n");
    println!("=== anon-target-url-underscore ===\n{out2}");
}
