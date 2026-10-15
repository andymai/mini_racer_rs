#![allow(
    clippy::print_stdout,
    reason = "cargo reads build script directives from stdout"
)]

fn main() {
    println!("cargo::rerun-if-env-changed=DEP_MINI_RACER_MADVISE_WRAP");
    if std::env::var("DEP_MINI_RACER_MADVISE_WRAP").is_ok_and(|v| v == "madvise") {
        println!("cargo::rustc-link-arg-cdylib=-Wl,--wrap=madvise");
    }
    // Ruby dlopens extensions RTLD_GLOBAL; -Bsymbolic keeps our V8 and rusty_v8
    // symbols bound to our own copies if another extension exports the same names.
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "linux") {
        println!("cargo::rustc-link-arg-cdylib=-Wl,-Bsymbolic");
    }
}
