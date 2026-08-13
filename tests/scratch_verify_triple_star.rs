//! Scratch verification: `***` paragraph — leftover `*` after failed `**`.
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
fn triple_star_paragraph() {
    let out = run("***\n");
    println!("=== triple-star ===\n{out}");
    let out2 = run("adm test:\n\n.. admonition:: ***\n\n   body\n");
    println!("=== adm-title-triple-star ===\n{out2}");
}
