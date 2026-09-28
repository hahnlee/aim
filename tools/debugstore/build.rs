use std::path::PathBuf;
fn main() {
    let root = PathBuf::from(std::env::var_os("AIM_SOURCE_ROOT").expect("source root"));
    cxx_build::bridge("src/lib.rs")
        .include(root.join("_aosp/system/core/libutils/include"))
        .include(root.join("_aosp/system/core/libsystem/include"))
        .std("c++17")
        .compile("debugstore-cxx");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-env-changed=AIM_SOURCE_ROOT");
}
