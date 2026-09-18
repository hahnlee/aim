//! Isolated genuine ART/BinderProxy list acceptance. Never add this image or
//! its entry point to either production runtime's source or export closure.
use super::*;
use crate::native_link_recipe::{transform_recipe, LinkIdentity};

pub(crate) fn build_binder_recipient_test(root: &Path) -> Result<()> {
    let product = root.join("_build/runtime-link-probe");
    let recipe = product.join("runtime-link.argv");
    super::common::require_file(
        &recipe,
        "run audit-runtime-link to publish its validated recipe",
    )?;
    let out = root.join("_build/native-fixtures/binder-recipient");
    fs::create_dir_all(&out)?;
    let classes = out.join("classes");
    let dex = out.join("dex");
    fs::create_dir_all(&classes)?;
    fs::create_dir_all(&dex)?;
    let platform = find_android_platform_jar()?;
    run_command(
        Command::new(crate::support::support_build_tool("JAVAC", "javac"))
            .args(["--release", "8", "-encoding", "UTF-8", "-cp"])
            .arg(&platform)
            .arg("-d")
            .arg(&classes)
            .arg(root.join("tools/tests/binder-recipient/Recipient.java")),
    )?;
    run_command(
        Command::new(find_d8()?)
            .args(["--min-api", "26", "--lib"])
            .arg(&platform)
            .arg("--output")
            .arg(&dex)
            .arg(classes.join("dev/darwinart/tests/Recipient.class")),
    )?;

    let paths = BuildPaths::from_root(root);
    let runtime = root.join("_aosp/art/runtime");
    let mut includes = super::graphics_core_probes::core_probe_includes(root, &paths, &runtime);
    for path in [
        "",
        "_build/binder-jni/patched-source/core/jni",
        "_build/surfaceflinger-core/work/frameworks-native/libs/binder/include",
        "_aosp/system/core/libutils/include",
        "_aosp/system/core/libsystem/include",
        "_aosp/libnativehelper-full/include_jni",
        "tools/bionic-process-state-facade/include",
    ] {
        includes.push(root.join(path));
    }
    let include_refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let (ndk, ndk_arch) = find_ndk_headers()?;
    let object = out.join("entry.mm.o");
    run_command(
        runtime_cpp_command(&include_refs)
            .args(["-include", "mirror/object_reference.h", "-idirafter"])
            .arg(ndk)
            .arg("-idirafter")
            .arg(ndk_arch)
            .arg("-c")
            .arg(root.join("tools/tests/binder-recipient/entry.mm"))
            .arg("-o")
            .arg(&object),
    )?;
    let library = out.join("libdarwin_art_binder_recipient_test.dylib");
    let expected = LinkIdentity {
        output: product.join("libdarwin_art_runtime.dylib"),
        map: product.join("runtime-link.map"),
        install_name: "@rpath/libdarwin_art_runtime.dylib".into(),
    };
    let replacement = LinkIdentity {
        output: library.clone(),
        map: out.join("test-link.map"),
        install_name: "@rpath/libdarwin_art_binder_recipient_test.dylib".into(),
    };
    let mut link = transform_recipe(
        &recipe,
        &expected,
        &replacement,
        &object,
        std::ffi::OsStr::new("_darwin_art_binder_recipient_test_entry"),
    )?;
    // Preserve the product's existing rpaths; this extra absolute test path
    // compensates only for the fixture image's different output directory.
    link.arg(format!(
        "-Wl,-rpath,{}",
        root.join("_build/tracing-perfetto/perfetto-out").display()
    ));
    run_command(&mut link)?;
    run_command(
        Command::new("bash")
            .arg(root.join("tools/build-android16-openjdk-named-jni-owner.sh"))
            .arg(out.join("libopenjdk-named-jni-owner.dylib")),
    )?;
    run_command(
        Command::new("clang++")
            .args(["-std=c++20", "-Wall", "-Wextra", "-Werror"])
            .arg(root.join("tools/tests/binder-recipient/driver.cc"))
            .arg("-o")
            .arg(out.join("driver")),
    )?;
    println!(
        "binder-recipient-test: built isolated image={} dex={}",
        library.display(),
        dex.join("classes.dex").display()
    );
    Ok(())
}
