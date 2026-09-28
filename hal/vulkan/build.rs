//! The installed name is the library's soname. `AHardwareBuffer_getNativeHandle`
//! is LL-NDK (not in the NDK's stub), so the driver links against the
//! image's own libnativewindow.so, which `cargo aim` links into place
//! (`aim_paths::android_link_dir`).

fn main() {
    println!("cargo::rustc-cdylib-link-arg=-Wl,-soname,vulkan.aim.so");
    println!(
        "cargo::rustc-link-search=native={}",
        aim_paths::android_link_dir().display()
    );
}
