//! Soong's `libbinder_ndk_bindgen` for a vendor module: the flags of
//! `libbinder_ndk_bindgen_flags.txt` and Android.bp, with the vendor
//! defines, against the NDK sysroot and the pinned binder headers. With the
//! `system` feature it is the platform (system partition) variant, which
//! the replaced native daemons use (`daemons/`).

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor_hals = manifest.join("../../_build/vendor-hals");
    let binder = vendor_hals.join("src/frameworks/native/libs/binder");
    let ndk = PathBuf::from(env::var("ANDROID_NDK_HOME").expect("ANDROID_NDK_HOME"));
    let variant: &[&str] = if env::var_os("CARGO_FEATURE_SYSTEM").is_some() {
        &[]
    } else {
        &["-D__ANDROID_VENDOR__", "-D__ANDROID_VNDK__"]
    };
    let sysroot = ndk.join("toolchains/llvm/prebuilt/darwin-x86_64/sysroot");
    let bindings = bindgen::Builder::default()
        .header(
            binder
                .join("rust/sys/BinderBindings.hpp")
                .display()
                .to_string(),
        )
        .clang_args([
            "-x",
            "c++",
            "-std=c++17",
            "--target=aarch64-linux-android35",
            "-DANDROID_PLATFORM",
        ])
        .clang_args(variant)
        .clang_arg(format!("--sysroot={}", sysroot.display()))
        .clang_arg(format!("-I{}", binder.join("ndk/include_ndk").display()))
        .clang_arg(format!(
            "-I{}",
            binder.join("ndk/include_platform").display()
        ))
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .default_enum_style(bindgen::EnumVariation::Rust {
            non_exhaustive: true,
        })
        .constified_enum("android::c_interface::consts::.*")
        .allowlist_type("android::c_interface::.*")
        .allowlist_type("AStatus")
        .allowlist_type("AIBinder_Class")
        .allowlist_type("AIBinder")
        .allowlist_type("AIBinder_Weak")
        .allowlist_type("AIBinder_DeathRecipient")
        .allowlist_type("AParcel")
        .allowlist_type("binder_status_t")
        .blocklist_function("vprintf")
        .blocklist_function("strtold")
        .blocklist_function("_vtlog")
        .blocklist_function("vscanf")
        .blocklist_function("vfprintf_worker")
        .blocklist_function("vsprintf")
        .blocklist_function("vsnprintf")
        .blocklist_function("vsnprintf_filtered")
        .blocklist_function("vfscanf")
        .blocklist_function("vsscanf")
        .blocklist_function("vdprintf")
        .blocklist_function("vasprintf")
        .blocklist_function("strtold_l")
        .allowlist_function(".*")
        .blocklist_type("sockaddr")
        .raw_line("use libc::sockaddr;")
        .generate()
        .expect("bindgen of the libbinder_ndk headers");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings.write_to_file(out.join("bindings.rs")).unwrap();
    println!(
        "cargo::rustc-link-search=native={}",
        vendor_hals.join("link").display()
    );
    println!("cargo::rustc-link-lib=dylib=binder_ndk");
    println!("cargo::rerun-if-env-changed=ANDROID_NDK_HOME");
}
