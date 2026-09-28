use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=AIM_DL_PHDR_PROVIDER_LIBDIR");
    if env::var_os("CARGO_FEATURE_PROVIDER_LINK").is_none() {
        return;
    }
    let directory =
        env::var("AIM_DL_PHDR_PROVIDER_LIBDIR").expect("AIM_DL_PHDR_PROVIDER_LIBDIR is required");
    println!("cargo:rustc-link-search=native={directory}");
    println!("cargo:rustc-link-lib=static=aim-dl-phdr");
    println!("cargo:rustc-link-lib=c++");
}
