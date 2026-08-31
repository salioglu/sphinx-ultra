//! Adversarial-verify scratch: sphinx math :name: without :label:.

use sphinx_ultra::rst::{parse_rst, ParseOptions};

#[test]
fn sphinx_math_name_variants() {
    let cases = [
        ("math_name_only", ".. math::\n   :name: eq2\n\n   x\n"),
        (
            "math_name_and_label",
            ".. math::\n   :label: lbl\n   :name: nm\n\n   x\n",
        ),
        ("math_name_empty", ".. math::\n   :name:\n\n   x\n"),
        ("math_name_spaces", ".. math::\n   :name: My Eq\n\n   x\n"),
    ];
    for (name, rst) in cases {
        let tree = parse_rst(
            rst,
            &ParseOptions {
                source_path: "<snippet>".into(),
                sphinx: true,
                docname: "index".into(),
                exclude_patterns: Vec::new(),
                found_docs: None,
            },
        );
        println!("===== {name} =====");
        println!("{:?}", rst);
        print!("{}", tree.root.pformat());
        println!("=====");
    }
}
