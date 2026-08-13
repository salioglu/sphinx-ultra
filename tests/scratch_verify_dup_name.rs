//! Scratch verification: directive :name: duplicate-target warning placement.
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
fn dup_name_placement() {
    let cases: &[(&str, &str)] = &[
        ("image", ".. _n:\n\n.. image:: x.png\n   :name: n\n"),
        ("figure", ".. _n:\n\n.. figure:: x.png\n   :name: n\n"),
        ("math", ".. _n:\n\n.. math::\n   :name: n\n\n   e=mc^2\n"),
        ("rubric", ".. _n:\n\n.. rubric:: hi\n   :name: n\n"),
        ("code", ".. _n:\n\n.. code::\n   :name: n\n\n   x = 1\n"),
        (
            "parsed-literal",
            ".. _n:\n\n.. parsed-literal::\n   :name: n\n\n   text\n",
        ),
        (
            "line-block",
            ".. _n:\n\n.. line-block::\n   :name: n\n\n   a line\n",
        ),
        (
            "table",
            ".. _n:\n\n.. table::\n   :name: n\n\n   ==  ==\n   a   b\n   ==  ==\n",
        ),
        ("note", ".. _n:\n\n.. note::\n   :name: n\n\n   body\n"),
        ("topic", ".. _n:\n\n.. topic:: T\n   :name: n\n\n   body\n"),
        (
            "container",
            ".. _n:\n\n.. container::\n   :name: n\n\n   body\n",
        ),
    ];
    for (name, src) in cases {
        println!("==================== {name}");
        print!("{}", run(src));
    }
}
