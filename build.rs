fn main() {
    pkg_config::Config::new()
        .probe("libsystemd")
        .expect("Install libsystemd development headers");
    pkg_config::Config::new()
        .probe("libdrm")
        .expect("Install libdrm development headers");
    println!("cargo:rerun-if-changed=native/rime.c");
    println!("cargo:rerun-if-changed=native/terminal.c");
    let ghostty = pkg_config::Config::new()
        .probe("libghostty-vt")
        .expect("Install libghostty-vt");
    let lib = pkg_config::Config::new()
        .atleast_version("1.11")
        .probe("rime")
        .expect("Install librime and its pkg-config metadata");
    let mut build = cc::Build::new();
    for path in lib.include_paths.into_iter().chain(ghostty.include_paths) {
        build.include(path);
    }
    build
        .file("native/rime.c")
        .file("native/terminal.c")
        .warnings(true)
        .compile("console_rime");
}
