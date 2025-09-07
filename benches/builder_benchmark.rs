use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use sphinx_ultra::builder::SphinxBuilder;
use sphinx_ultra::config::BuildConfig;
use sphinx_ultra::parser::Parser;
use std::path::PathBuf;
use tempfile::TempDir;

fn bench_parser(c: &mut Criterion) {
    let config = BuildConfig::default();
    let parser = Parser::new(&config).unwrap();

    let rst_content = r#"
Title
=====

This is a sample RST document with various elements.

Subtitle
--------

Here's some **bold** and *italic* text.

.. code-block:: python

    def hello_world():
        print("Hello, World!")
        return True

.. note::
   This is a note admonition.

Cross-references
~~~~~~~~~~~~~~~~

See :doc:`other-document` and :ref:`some-reference`.

Lists
~~~~~

- Item 1
- Item 2
- Item 3

1. Numbered item 1
2. Numbered item 2
3. Numbered item 3

Tables
~~~~~~

+--------+--------+
| Header | Header |
+========+========+
| Cell   | Cell   |
+--------+--------+
| Cell   | Cell   |
+--------+--------+
"#;

    c.bench_function("parse_rst", |b| {
        b.iter(|| {
            parser
                .parse(
                    black_box(&PathBuf::from("test.rst")),
                    black_box(rst_content),
                )
                .unwrap()
        })
    });
}

fn bench_builder_small(c: &mut Criterion) {
    let temp_dir = TempDir::new().unwrap();
    let source_dir = temp_dir.path().join("source");
    let output_dir = temp_dir.path().join("output");

    std::fs::create_dir_all(&source_dir).unwrap();

    // Create test files
    for i in 0..10 {
        let content = format!(
            r#"
File {}
=======

This is test file number {}.

Content
-------

Some content for file {}.

.. code-block:: python

    # Example code
    def function_{}():
        return {}

"#,
            i, i, i, i, i
        );

        std::fs::write(source_dir.join(format!("file_{}.rst", i)), content).unwrap();
    }

    let config = BuildConfig::default();

    c.bench_function("build_small", |b| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        b.iter(|| {
            rt.block_on(async {
                let builder =
                    SphinxBuilder::new(config.clone(), source_dir.clone(), output_dir.clone())
                        .unwrap();

                black_box(builder.build().await.unwrap())
            })
        })
    });
}

fn bench_builder_parallel_jobs(c: &mut Criterion) {
    let temp_dir = TempDir::new().unwrap();
    let source_dir = temp_dir.path().join("source");
    let output_dir = temp_dir.path().join("output");

    std::fs::create_dir_all(&source_dir).unwrap();

    // Create more test files for parallel testing
    for i in 0..100 {
        let content = format!(
            r#"
File {}
=======

This is test file number {}.

Content Section 1
-----------------

Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor
incididunt ut labore et dolore magna aliqua.

Content Section 2
-----------------

Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut
aliquip ex ea commodo consequat.

.. code-block:: python

    # Example code for file {}
    def function_{}():
        result = {}
        for j in range(10):
            result += j * {}
        return result

.. note::
   This is note {} in the documentation.

Cross References
~~~~~~~~~~~~~~~~

See :doc:`file_{}` for related information.

"#,
            i,
            i,
            i,
            i,
            i,
            i,
            i,
            (i + 1) % 100
        );

        std::fs::write(source_dir.join(format!("file_{}.rst", i)), content).unwrap();
    }

    let mut group = c.benchmark_group("parallel_jobs");

    for jobs in [1, 2, 4, 8].iter() {
        group.bench_with_input(BenchmarkId::new("jobs", jobs), jobs, |b, &jobs| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            b.iter(|| {
                rt.block_on(async {
                    let config = BuildConfig::default();
                    let mut builder =
                        SphinxBuilder::new(config, source_dir.clone(), output_dir.clone()).unwrap();

                    builder.set_parallel_jobs(jobs);
                    black_box(builder.build().await.unwrap())
                })
            })
        });
    }

    group.finish();
}

fn bench_cache_performance(c: &mut Criterion) {
    // TODO: Implement cache benchmarks
    c.bench_function("cache_hit", |b| {
        b.iter(|| {
            // Placeholder for cache benchmark
            black_box(42)
        })
    });
}

criterion_group!(
    benches,
    bench_parser,
    bench_builder_small,
    bench_builder_parallel_jobs,
    bench_cache_performance
);

criterion_main!(benches);
