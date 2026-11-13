fn main() {
    // The write harness is compiled in with `--cfg apt_scout_write_harness`, so rustc has to know
    // the name is expected; otherwise every `#[cfg(...)]` in the tree trips the unexpected_cfgs
    // lint, and the alternative (`#![allow(unexpected_cfgs)]`) would also hide a misspelt name in
    // the very attributes the compile-out depends on.
    println!("cargo:rustc-check-cfg=cfg(apt_scout_write_harness)");
}
