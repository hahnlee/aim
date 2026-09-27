//! Soong compiles a vendor module's rust_library with these cfgs.
fn main() {
    println!("cargo::rustc-cfg=android_vendor");
    println!("cargo::rustc-cfg=android_vndk");
}
