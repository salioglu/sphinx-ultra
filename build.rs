fn main() {
    let target = std::env::var("TARGET").unwrap();

    // For musl targets, ensure we link the math library
    if target.contains("musl") {
        println!("cargo:rustc-link-lib=m");
    }
}
