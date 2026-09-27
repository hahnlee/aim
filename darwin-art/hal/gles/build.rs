//! The installed name is the library's soname. The ANativeWindow calls are
//! LL-NDK (not in the NDK's stub), so they link against the image's own
//! libnativewindow.so, which tools/build-vendor-hals.sh links into place.
use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-cdylib-link-arg=-Wl,-soname,libGLES_darwin.so");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    println!(
        "cargo::rustc-link-search=native={}",
        manifest.join("../../_build/vendor-hals/link").display()
    );
}
