//! The installed name is the library's soname.
fn main() {
    println!("cargo::rustc-cdylib-link-arg=-Wl,-soname,mapper.darwin.so");
}
