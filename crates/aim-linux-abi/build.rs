fn main() {
    let plist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin/linux-run.plist");
    println!("cargo:rerun-if-changed={}", plist.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // Cargo links to a hash-suffixed filename; keep its ad-hoc identity stable.
        println!("cargo:rustc-link-arg-bin=linux-run=-Wl,-final_output,dev.aim.linux-run");
        println!(
            "cargo:rustc-link-arg-bin=linux-run=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            plist.display()
        );
    }
}
