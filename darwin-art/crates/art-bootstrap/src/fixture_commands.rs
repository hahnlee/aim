use super::*;

fn prepare_apk_jni_native(root: &Path) -> Result<PathBuf> {
    let apk = root.join("_build/android-apk-app-runtime/simple-jni.apk");
    let output = Command::new("unzip")
        .args(["-p"])
        .arg(&apk)
        .arg("lib/arm64-v8a/libdarwin-art-simple-jni.so")
        .output()?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err(format!(
            "JNI APK native entry could not be extracted: {}",
            apk.display()
        )
        .into());
    }
    let directory = root.join("_build/android-apk-app-runtime/native-root");
    fs::create_dir_all(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    let library = directory.join("libdarwin-art-simple-jni.so");
    if library.exists() {
        fs::set_permissions(&library, fs::Permissions::from_mode(0o600))?;
    }
    fs::write(&library, output.stdout)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))?;
    fs::set_permissions(&library, fs::Permissions::from_mode(0o400))?;
    Ok(library)
}

pub(crate) fn probe_runtime_dex_flavor(
    root: &Path,
    show_window: bool,
    real_graphics: bool,
    button: bool,
    elf_jni: bool,
    network: bool,
    apk_app: bool,
) -> Result<()> {
    probe_runtime_dex_flavor_impl(
        root,
        show_window,
        real_graphics,
        button,
        elf_jni,
        network,
        apk_app,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn probe_runtime_dex_flavor_impl(
    root: &Path,
    show_window: bool,
    real_graphics: bool,
    button: bool,
    elf_jni: bool,
    network: bool,
    apk_app: bool,
    apk_jni: bool,
) -> Result<()> {
    let core_icu4j = if real_graphics {
        prepare_icu_runtime_bootclasspath(root)?
    } else {
        prepare_icu_bootclasspath(root)?;
        root.join("_build/bootclasspath/core-icu4j.jar")
    };
    let executable = root.join("target/debug/darwin-art-fixture-runner");
    let fixture_library = root.join(if real_graphics {
        "_build/native-fixtures/graphics/libdarwin_art_fixture.dylib"
    } else {
        "_build/native-fixtures/headless/libdarwin_art_fixture.dylib"
    });
    let runtime_library = root.join(if real_graphics {
        "_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib"
    } else {
        "_build/runtime-link-probe/libdarwin_art_runtime.dylib"
    });
    let core_oj = root.join("_prebuilt/android-16/bootclasspath/core-oj.jar");
    let core_libart = root.join("_prebuilt/android-16/bootclasspath/core-libart.jar");
    let framework = root.join("_prebuilt/android-16/bootclasspath/framework.jar");
    let classes_dex = root.join(if apk_app {
        if apk_jni {
            "_build/android-apk-app-runtime/simple-jni.apk"
        } else {
            "_build/android-apk-app-runtime/simple-no-native.apk"
        }
    } else if network {
        "_build/network-runtime-probe/dex/classes.dex"
    } else if elf_jni {
        "_build/elf-jni-dex/dex/classes.dex"
    } else if button {
        "_build/button-dex/dex/classes.dex"
    } else {
        "_build/dex-probe/dex/classes.dex"
    });
    for input in [
        &executable,
        &fixture_library,
        &runtime_library,
        &core_oj,
        &core_libart,
        &framework,
        &core_icu4j,
        &classes_dex,
    ] {
        if !input.is_file() {
            return Err(format!(
                "runtime DEX probe input is missing: {}; run `art-bootstrap all` first",
                input.display()
            )
            .into());
        }
    }

    let mut command = Command::new(&executable);
    // The client owns access to its immutable test inputs. Publish exact
    // capabilities through the same filesystem contract as ordinary APK
    // launchers, rather than teaching the product about fixture filenames.
    let mut host_files = env::var("DARWIN_ART_RUNTIME_HOST_FILES").unwrap_or_default();
    for input in [
        &core_oj,
        &core_libart,
        &framework,
        &core_icu4j,
        &classes_dex,
    ] {
        if !host_files.is_empty() {
            host_files.push(':');
        }
        host_files.push_str(&input.to_string_lossy());
    }
    command.env("DARWIN_ART_RUNTIME_HOST_FILES", &host_files);
    command.arg("--fixture-image").arg(&fixture_library);
    if show_window {
        // Keep normal window probes short; the capture script controls its
        // own interaction hold time through DARWIN_ART_TEST_POINTER_HOLD_MS.
        command.args(["--window-seconds", "3"]);
    }
    command
        .arg(&runtime_library)
        .arg(&core_oj)
        .arg(&core_libart)
        .arg(&framework)
        .arg(&core_icu4j)
        .arg(&classes_dex);
    let mut system_root = None;
    if real_graphics {
        let icu_runtime = root.join("_build/icu-runtime-adapters/runtime");
        let i18n_root = icu_runtime.join("i18n");
        let data_root = icu_runtime.join("data");
        let tzdata_root = icu_runtime.join("tzdata");
        let icu_data = i18n_root.join("etc/icu/icudt76l.dat");
        if !icu_data.is_file() {
            return Err(format!(
                "Android ICU76 runtime data is missing: {}; run `build-icu-runtime-adapters` first",
                icu_data.display()
            )
            .into());
        }
        command
            .env("ANDROID_I18N_ROOT", &i18n_root)
            .env("ANDROID_DATA", &data_root)
            .env("ANDROID_TZDATA_ROOT", &tzdata_root);
        if button || apk_app {
            let fonts_xml = root.join("probes/button/fonts.xml");
            let roboto = root.join("_aosp/external/skia/resources/fonts/Roboto-Regular.ttf");
            for input in [&fonts_xml, &roboto] {
                if !input.is_file() {
                    return Err(format!("Button font input is missing: {}", input.display()).into());
                }
            }
            command
                .env("DARWIN_ART_TEST_FONTS_XML", &fonts_xml)
                .env("DARWIN_ART_TEST_FONT", &roboto)
                .env(
                    "DARWIN_ART_RUNTIME_HOST_FILES",
                    format!(
                        "{}:{}:{}",
                        host_files,
                        fonts_xml.display(),
                        roboto.display()
                    ),
                );
            if button {
                let framework_res = root.join("_prebuilt/android-16/resources/framework-res.apk");
                if !framework_res.is_file() {
                    return Err(format!(
                        "Android framework resources are missing: {}",
                        framework_res.display()
                    )
                    .into());
                }
                command.env("DARWIN_ART_FRAMEWORK_RES_APK", framework_res);
            }
            let guest_root = prepare_probe_android_system_root(root)?;
            command.env("DARWIN_ART_ANDROID_SYSTEM_ROOT", &guest_root);
            if button {
                // The framework Button probe uses a raw host-backed DEX rather
                // than an APK identity, but DexPathList still performs the
                // normal authorized-host `File.isFile()` check. Publish the
                // exact immutable DEX as a capability-scoped support path;
                // this does not opt the process into APK mode.
                command.env("DARWIN_ART_APK_APP_SUPPORT_DEX", &classes_dex);
            }
            system_root = Some(guest_root);
        }
    }
    if apk_app {
        // Unchanged APK fixtures use the same production Android subsystem
        // support as installed apps, never a Probe Activity/Context classpath.
        build_runtime_support_dex(root)?;
    }
    if apk_app {
        let framework_res = root.join("_prebuilt/android-16/resources/framework-res.apk");
        if !framework_res.is_file() {
            return Err(format!(
                "Android framework resources are missing: {}",
                framework_res.display()
            )
            .into());
        }
        command
            .env("DARWIN_ART_APK_APP_PACKAGE", "dev.darwinart.simple")
            .env(
                "DARWIN_ART_APK_APP_ACTIVITY",
                "dev.darwinart.simple.MainActivity",
            )
            .env(
                "DARWIN_ART_APK_APP_DESCRIPTOR",
                "Ldev/darwinart/simple/MainActivity;",
            )
            .env(
                "DARWIN_ART_APK_APP_SUPPORT_DEX",
                root.join("_build/runtime-support-dex/dex/classes.dex"),
            )
            .env("DARWIN_ART_APK_APP_RESOURCE_APK", &classes_dex)
            .env("DARWIN_ART_FRAMEWORK_RES_APK", framework_res);
        // The headless gate proves APK/Activity/resource/native loading;
        // pointer hit-testing is a window-only contract.  Do not install the
        // strict input marker unless a real window is being exercised.
        if show_window {
            command.env("DARWIN_ART_APK_APP_EXPECT_WIDGETS", "1");
        }
        if apk_jni {
            let native_path = prepare_apk_jni_native(root)?;
            command
                .env("DARWIN_ART_APK_APP_NATIVE_PATH", native_path)
                .env("DARWIN_ART_APK_EXPECT_NATIVE_ANSWER", "1");
        }
        if show_window {
            command.env("DARWIN_ART_WINDOW_SCALE", "2");
        }
        if show_window {
            // The fixture's framework Button is laid out beneath the title
            // and controls; use its center for the real pointer acceptance
            // rather than a coordinate that only exercises the root view.
            command.env("DARWIN_ART_TEST_POINTER_CLICK", "180,340");
        }
    }
    let mut network_fixture = None;
    if network {
        let fixture = prepare_private_network_fixture(root)?;
        command.env("DARWIN_ART_ANDROID_NETWORK_FIXTURE", &fixture.library);
        network_fixture = Some(fixture);
    }
    let mut apk_native_fixture = None;
    if elf_jni {
        build_shell_gate(root, "build-android-elf-jni-fixture.sh")?;
        build_shell_gate(root, "build-android35-libcxx-runtime-fixtures.sh")?;
        build_shell_gate(root, "build-android-elf-tls-runtime-fixture.sh")?;
        let fixture = root.join("_build/android-elf-jni-fixture/libdarwin-art-jni-fixture.so");
        let generic_fixture =
            root.join("_build/android-elf-jni-fixture/libdarwin-art-generic-root.so");
        let libcxx_collections = root.join(
            "_build/android35-libcxx-runtime-fixtures/collections/libdarwin_art_libcxx_consumer.so",
        );
        let libcxx_exception = root.join(
            "_build/android35-libcxx-runtime-fixtures/exception/libdarwin_art_libcxx_exception.so",
        );
        let tls_fixture =
            root.join("_build/android-elf-tls-runtime-fixture/libdarwin_art_tls_runtime.so");
        if !fixture.is_file()
            || !generic_fixture.is_file()
            || !libcxx_collections.is_file()
            || !libcxx_exception.is_file()
            || !tls_fixture.is_file()
        {
            return Err(format!(
                "Android ELF fixture is missing: {}, {}, {}, {}, or {}",
                fixture.display(),
                generic_fixture.display(),
                libcxx_collections.display(),
                libcxx_exception.display(),
                tls_fixture.display()
            )
            .into());
        }
        let extracted = prepare_private_apk_native_fixture(root)?;
        let private_data = extracted.temporary_root.join("data");
        fs::create_dir(&private_data)?;
        command.env("DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT", &private_data);
        let extracted_root = extracted
            .extracted_root
            .join("libdarwin-art-generic-root.so");
        let extracted_jni = extracted
            .extracted_root
            .join("libdarwin-art-jni-fixture.so");
        command
            .env("DARWIN_ART_ANDROID_ELF_JNI_FIXTURE", extracted_jni)
            .env("DARWIN_ART_ANDROID_ELF_GENERIC_FIXTURE", &extracted_root)
            .env("DARWIN_ART_ANDROID_APK_ELF_FIXTURE", extracted_root)
            .env("DARWIN_ART_ANDROID_APK_SHA256", &extracted.apk_sha256)
            .env("DARWIN_ART_ANDROID_APK_ROOT_SHA256", &extracted.root_sha256)
            .env(
                "DARWIN_ART_ANDROID_LIBCXX_COLLECTIONS_FIXTURE",
                libcxx_collections,
            )
            .env(
                "DARWIN_ART_ANDROID_LIBCXX_EXCEPTION_FIXTURE",
                libcxx_exception,
            )
            .env("DARWIN_ART_ANDROID_TLS_FIXTURE", tls_fixture);
        apk_native_fixture = Some(extracted);
    }
    let output_result = command_output(&mut command);
    if let Some(guest_root) = system_root {
        let _ = fs::remove_dir_all(guest_root);
    }
    let output = output_result?;
    let expected = if apk_app {
        let render_scale = if show_window { 2 } else { 1 };
        let frame_width = 360 * render_scale;
        let frame_height = 640 * render_scale;
        let native_marker = if apk_jni { 1 } else { 0 };
        let widget_marker = if show_window {
            " widgets=framework-owned"
        } else {
            ""
        };
        format!(
            "Hello from Darwin ART main: 안녕\n\
         ART Darwin Runtime::Create: ok\n\
         ART Darwin app ClassLoader: PathClassLoader\n\
         ART Darwin DEX interpreter: Hello.answer()=42\n\
         ART Darwin JNI: hostPageSize()=16384 nativeRoundTrip()=42\n\
         ART runtime native: System.arraycopy()=42\n\
         ART Android APK: package=dev.darwinart.simple launcher=dev.darwinart.simple.MainActivity classes.dex=APK native={native_marker} gpu=direct drawable={frame_width}x{frame_height}{widget_marker}\n\
         ART Android window: Activity.attach()=PhoneWindow+DecorView\n\
         ART Android view: Activity.setContentView()->DecorView.draw(Canvas)={frame_width}x{frame_height}\n\
         ART Android lifecycle: Activity.onCreate()=43\n\
         ART Darwin launcher: main(String[])=ok"
        )
    } else if network {
        "Hello from Darwin ART main: 안녕\n\
         ART Android network: JavaVMExt+JNI_OnLoad loopback-HTTP=42 socket+DNS=closed Internet=no\n\
         ART Darwin Runtime::Create: ok\n\
         ART Darwin app ClassLoader: PathClassLoader\n\
         ART Darwin DEX interpreter: Hello.answer()=42\n\
         ART Darwin JNI: hostPageSize()=16384 nativeRoundTrip()=42\n\
         ART runtime native: System.arraycopy()=42\n\
         ART Android framework: ProbeActivity().probeValue()=42\n\
         ART Android window: Activity.attach()=PhoneWindow+DecorView\n\
         ART Android view: Activity.setContentView()->DecorView.draw(Canvas)=360x640\n\
         ART Android lifecycle: Activity.onCreate()=43\n\
         ART Darwin launcher: main(String[])=ok"
            .to_owned()
    } else if elf_jni {
        let extracted = apk_native_fixture
            .as_ref()
            .expect("ELF JNI probe retains its APK extraction");
        format!(
            "Hello from Darwin ART main: 안녕\n\
         ART Android libc++: real-r28c collections=189 exception-cleanup=73 unload=sequential\n\
         ART Android ELF TLS: local-TLSDESC threads=4 align=64 unload=quiescent\n\
         ART Android ELF JNI: graph=child-first+relocated providers=bind_builtins+__errno+strlen+fs-random-ctor+scanf+swprintf+ioctl+strftime+sendfile load+JNI_OnLoad+RegisterNatives=generic+fixture scalar-ref=all nativeUsesEnv=current stack-repack=ok\n\
         ART Android APK ELF: apk-sha256={} root-sha256={} graph=root+child+grandchild load=JavaVMExt+NativeBridge unload=shutdown-trampolines-zero\n\
         ART Darwin Runtime::Create: ok\n\
         ART Darwin app ClassLoader: PathClassLoader\n\
         ART Darwin DEX interpreter: Hello.answer()=42\n\
         ART Darwin JNI: hostPageSize()=16384 nativeRoundTrip()=42\n\
         ART runtime native: System.arraycopy()=42\n\
         ART Android framework: ProbeActivity().probeValue()=42\n\
         ART Android window: Activity.attach()=PhoneWindow+DecorView\n\
         ART Android view: Activity.setContentView()->DecorView.draw(Canvas)=360x640\n\
         ART Android lifecycle: Activity.onCreate()=43\n\
         ART Darwin launcher: main(String[])=ok",
            extracted.apk_sha256, extracted.root_sha256
        )
    } else {
        "Hello from Darwin ART main: 안녕\n\
                    ART Darwin Runtime::Create: ok\n\
                    ART Darwin app ClassLoader: PathClassLoader\n\
                    ART Darwin DEX interpreter: Hello.answer()=42\n\
                    ART Darwin JNI: hostPageSize()=16384 nativeRoundTrip()=42\n\
                    ART runtime native: System.arraycopy()=42\n\
                    ART Android framework: ProbeActivity().probeValue()=42\n\
                    ART Android window: Activity.attach()=PhoneWindow+DecorView\n\
                    ART Android view: Activity.setContentView()->DecorView.draw(Canvas)=360x640\n\
                    ART Android lifecycle: Activity.onCreate()=43\n\
                    ART Darwin launcher: main(String[])=ok"
            .to_owned()
    };
    // The headless ELF acceptance intentionally has no GraphicsSession and
    // therefore performs managed Activity/DecorView validation without a
    // presented frame.  Keep that distinction explicit in the golden output;
    // real graphics/APK modes still require their concrete dimensions.
    let expected = if !real_graphics && !show_window {
        expected.replace(
            "DecorView.draw(Canvas)=360x640",
            "DecorView.draw(Canvas)=0x0",
        )
    } else {
        expected
    };
    if output.trim() != expected {
        return Err(format!("unexpected runtime DEX probe output: {output:?}").into());
    }
    drop(network_fixture);
    if apk_app {
        if show_window {
            println!(
                "probe-runtime-apk{}-window: APK -> Activity.onCreate -> NSWindow",
                if apk_jni { "-jni" } else { "-app" }
            );
        } else {
            println!(
                "probe-runtime-apk{}: {} APK -> manifest Activity -> frame -> shutdown",
                if apk_jni { "-jni-app" } else { "-app" },
                if apk_jni { "JNI" } else { "no-native" }
            );
        }
    } else if network {
        println!("probe-runtime-network: ART JNI loopback + socket/DNS quiescence PASS");
    } else if elf_jni {
        println!("probe-runtime-elf-jni: ART DT_NEEDED graph + JNI + reverse finalizers PASS");
    } else if button {
        if show_window {
            println!("probe-runtime-button-window: android.widget.Button -> AOSP HWUI -> NSWindow");
        } else {
            println!(
                "probe-runtime-button: android.widget.Button -> AOSP HWUI -> frame -> shutdown"
            );
        }
    } else if real_graphics {
        if show_window {
            println!(
                "probe-runtime-graphics-window: Bitmap-backed Canvas -> DecorView.draw() -> NSWindow"
            );
        } else {
            println!(
                "probe-runtime-graphics: Bitmap-backed Canvas -> DecorView.draw() -> frame -> shutdown"
            );
        }
    } else if show_window {
        println!("probe-window: PhoneWindow -> DecorView.draw(Canvas) -> NSWindow");
    } else {
        println!(
            "probe-runtime-dex: Activity.attach() + PhoneWindow + DecorView.draw(Canvas) -> Darwin"
        );
    }
    Ok(())
}
