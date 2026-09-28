//! Soong compiles a vendor module's rust_library with these cfgs; a system
//! module (the `system` feature) has none.
fn main() {
    if std::env::var_os("CARGO_FEATURE_SYSTEM").is_none() {
        println!("cargo::rustc-cfg=android_vendor");
        println!("cargo::rustc-cfg=android_vndk");
    }
}
