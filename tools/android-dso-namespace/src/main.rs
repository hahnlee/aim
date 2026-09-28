use android_dso_namespace::{manifest, resolve};

fn main() {
    assert!(resolve("libdl.so", "android_dlopen_ext", Some("LIBC")).is_ok());
    assert!(resolve("liblog.so", "__android_log_print", None).is_ok());
    assert!(resolve("libSystem.B.dylib", "malloc", None).is_err());
    assert!(resolve("libc.so", "malloc", Some("LIBC")).is_err());
    print!("{}", manifest());
    eprintln!("android-dso-namespace: PASS closed=1 libdl=5 liblog=18");
}
