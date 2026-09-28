//! The installed name is the library's soname. The ANativeWindow calls are
//! LL-NDK (not in the NDK's stub), so they link against the image's own
//! libnativewindow.so, which `cargo aim` links into place
//! (`aim_paths::android_link_dir`).

fn main() {
    println!("cargo::rustc-cdylib-link-arg=-Wl,-soname,libGLES_aim.so");
    println!(
        "cargo::rustc-link-search=native={}",
        aim_paths::android_link_dir().display()
    );
}
