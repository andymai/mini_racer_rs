#![allow(
    clippy::print_stdout,
    reason = "cargo reads build script directives from stdout"
)]

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let linux = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "linux");
    if linux && std::env::var_os("CARGO_FEATURE_NO_MADVISE_WRAP").is_none() {
        println!("cargo::rustc-link-arg=-Wl,--wrap=madvise");
        println!("cargo::metadata=wrap=madvise");
    }
}
