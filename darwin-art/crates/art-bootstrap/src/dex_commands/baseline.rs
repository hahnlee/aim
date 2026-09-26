use super::fixture_framework_inputs::{
    append_am_classes, append_wm_classes, append_wm_sources, clear_am_class_output,
    clear_wm_class_output,
};
use super::*;
use std::fmt::Write as _;

const LARGE_FIELD_PADDING_COUNT: usize = 4096;

/// Generate the large-object field fixture into the ignored build tree. ART
/// lays out the generated padding fields before the named targets, giving the
/// ARM64 field-get path a real offset well beyond its immediate-offset range.
fn generate_large_field_fixture(root: &Path, build_dir: &Path) -> Result<PathBuf> {
    let template_path = root.join("probes/Hello.java");
    let template = fs::read_to_string(&template_path)?;
    let marker = "    // DARWIN_ART_LARGE_FIELD_FIXTURE\n";
    if !template.contains(marker) {
        return Err(format!("Hello.java is missing {marker:?}").into());
    }

    let mut generated_fields = String::with_capacity(LARGE_FIELD_PADDING_COUNT * 82 * 2);
    for index in 0..LARGE_FIELD_PADDING_COUNT {
        writeln!(
            generated_fields,
            "    public int darwinLargeIntPadding{index:04};"
        )?;
    }
    for index in 0..LARGE_FIELD_PADDING_COUNT {
        writeln!(
            generated_fields,
            "    public Object darwinLargeReferencePadding{index:04};"
        )?;
    }
    generated_fields.push_str(
        "    public int jitLargeIntField;\n\
         public Object jitLargeReferenceField;\n\
         public volatile int jitLargeVolatileIntField;\n",
    );

    let generated_dir = build_dir.join("generated");
    fs::create_dir_all(&generated_dir)?;
    let generated_path = generated_dir.join("Hello.java");
    fs::write(&generated_path, template.replace(marker, &generated_fields))?;
    Ok(generated_path)
}

pub(crate) fn build_dex_probe(root: &Path) -> Result<()> {
    build_foundation(root)?;

    let artbase = root.join("_aosp/art/libartbase");
    let patched_artbase = root.join("_build/foundation/patched-source/libartbase");
    let libdexfile = root.join("_aosp/art/libdexfile");
    let libbase_include = root.join("_aosp/system/libbase/include");
    let libziparchive_include = root.join("_aosp/system/libziparchive/include");
    let palette_include = root.join("_aosp/art/libartpalette/include");
    if !libdexfile.join("Android.bp").exists() {
        return Err("libdexfile sources are missing; run `art-bootstrap sync` first".into());
    }

    let java_home = PathBuf::from("/opt/homebrew/opt/openjdk@17");
    let jni_include = java_home.join("include");
    let jni_darwin_include = jni_include.join("darwin");
    if !jni_include.join("jni.h").exists() {
        return Err("OpenJDK 17 JNI headers were not found under /opt/homebrew".into());
    }

    let build_dir = root.join("_build/dex-probe");
    let class_dir = build_dir.join("classes");
    let dex_dir = build_dir.join("dex");
    let unsafe_boot_dex_dir = build_dir.join("unsafe-boot-dex");
    let object_dir = build_dir.join("objects");
    fs::create_dir_all(&class_dir)?;
    fs::create_dir_all(&dex_dir)?;
    if unsafe_boot_dex_dir.exists() {
        fs::remove_dir_all(&unsafe_boot_dex_dir)?;
    }
    fs::create_dir_all(&unsafe_boot_dex_dir)?;
    fs::create_dir_all(&object_dir)?;
    let hello_source = generate_large_field_fixture(root, &build_dir)?;

    let android_platform_jar = find_android_platform_jar()?;
    let android_mock_jar = android_platform_jar
        .parent()
        .ok_or("Android platform jar has no parent")?
        .join("optional/android.test.mock.jar");
    if !android_mock_jar.exists() {
        return Err(format!(
            "Android mock library is missing: {}",
            android_mock_jar.display()
        )
        .into());
    }
    let compile_signatures_dir = build_dir.join("compile-signatures");
    if compile_signatures_dir.exists() {
        fs::remove_dir_all(&compile_signatures_dir)?;
    }
    fs::create_dir_all(&compile_signatures_dir)?;
    let mut signature_javac = Command::new("javac");
    signature_javac
        .args([
            "--release",
            "8",
            "-encoding",
            "UTF-8",
            "-sourcepath",
            "",
            "-classpath",
        ])
        .arg(&android_platform_jar)
        .arg("-d")
        .arg(&compile_signatures_dir)
        .arg(root.join("runtime/framework/compile-stubs/android/os/ServiceManager.java"));
    run_command(&mut signature_javac)?;
    let javac_bootclasspath = env::join_paths([
        compile_signatures_dir,
        find_android_core_system_modules()?,
        android_platform_jar.clone(),
    ])?;
    let mut javac = Command::new("javac");
    javac
        // Android's API surface is the boot class path. `--release 8`
        // incorrectly substitutes the Java SE 8 API and hides Android
        // methods such as Math.fma() while source/target 8 is still the
        // required classfile language level for this fixture.
        .args(["-source", "8", "-target", "8", "-encoding", "UTF-8", "-d"])
        .arg(&class_dir)
        .arg("-bootclasspath")
        .arg(&javac_bootclasspath)
        .arg("-classpath")
        .arg(&android_mock_jar)
        .arg(&hello_source)
        .arg(root.join("probes/ProbeActivity.java"))
        .arg(root.join("probes/UpstreamTestHarness.java"))
        .arg(root.join("probes/ProbeContext.java"))
        .arg(root.join("probes/ProbeAudioManager.java"))
        .arg(root.join("probes/ProbeSharedPreferences.java"))
        .arg(root.join("probes/ProbeShortcutManager.java"))
        .arg(root.join("probes/ProbeUserManager.java"))
        .arg(root.join("probes/ProbeContentResolver.java"))
        .arg(root.join("probes/ProbeHostDocumentProvider.java"))
        .arg(root.join("probes/ProbeMediaStoreProvider.java"))
        .arg(root.join("probes/ProbeCalendarProvider.java"))
        .arg(root.join("probes/ProbeResources.java"))
        .arg(root.join("probes/ProbePackageManager.java"))
        .arg(root.join("runtime/framework/storage/StorageManagerEndpoint.java"))
        .arg(root.join("runtime/framework/os/RemoteBinder.java"))
        .arg(root.join("runtime/framework/os/SystemServices.java"))
        .arg(root.join("runtime/framework/system/ServiceDirectory.java"))
        .arg(root.join("runtime/framework/am/ApplicationProcessRegistry.java"))
        .arg(root.join("runtime/framework/am/SystemServiceBindings.java"))
        .arg(root.join("runtime/framework/am/ServiceRecord.java"))
        .arg(root.join("runtime/framework/am/IntentBindRecord.java"))
        .arg(root.join("runtime/framework/am/ConnectionRecord.java"))
        .arg(root.join("runtime/framework/am/ServiceConnectionOwner.java"))
        .arg(root.join("runtime/framework/am/ServiceConnectionIndex.java"))
        .arg(root.join("runtime/framework/am/ServiceConnectionDeathRegistration.java"))
        .arg(root.join("runtime/framework/am/ServiceNotificationController.java"))
        .arg(root.join("runtime/framework/am/ServiceLifecycleOperation.java"))
        .arg(root.join("runtime/framework/am/ServiceLifecycleController.java"))
        .arg(root.join("runtime/framework/am/BoundServiceProcessLauncher.java"))
        .arg(root.join("runtime/framework/am/ProcessLaunchTransport.java"))
        .arg(root.join("runtime/framework/am/ServiceProcessLaunchController.java"))
        .arg(root.join("runtime/framework/am/ServiceConnectionResourceController.java"))
        .arg(root.join("runtime/framework/am/ActiveServices.java"))
        .arg(root.join("runtime/framework/am/PackageQueries.java"))
        .arg(root.join("runtime/framework/am/ApplicationPackages.java"))
        .arg(root.join("runtime/framework/am/ActivityManagerEndpoint.java"))
        .arg(root.join("runtime/framework/am/ActivityManagerClient.java"))
        .arg(root.join("runtime/framework/display/BuiltInDisplayConfiguration.java"))
        .arg(root.join("runtime/framework/display/DefaultDisplayRegistry.java"))
        .arg(root.join("runtime/framework/display/DisplayManagerEndpoint.java"))
        .arg(root.join("runtime/framework/user/UserManagerEndpoint.java"))
        .arg(root.join("runtime/framework/content/SettingsProviderEndpoint.java"))
        .arg(root.join("runtime/framework/content/ContentServiceEndpoint.java"))
        .arg(root.join("runtime/framework/content/ClipboardServiceEndpoint.java"))
        .arg(root.join("runtime/framework/notification/NotificationManagerEndpoint.java"))
        .arg(root.join("runtime/framework/inputmethod/InputMethodManagerEndpoint.java"))
        .arg(root.join("runtime/framework/input/InputManagerEndpoint.java"))
        .arg(root.join("runtime/framework/input/InputDeviceRegistry.java"))
        .arg(root.join("runtime/framework/input/SystemKeyboardMaps.java"))
        .arg(root.join("probes/ProbeXmlResourceParser.java"))
        .arg(root.join("probes/ProbeCanvas.java"))
        .arg(root.join("probes/ProbeView.java"))
        .arg(root.join("probes/ProbeContentRoot.java"))
        .arg(root.join("probes/compile-stubs/android/content/IContentProvider.java"))
        .arg(root.join("probes/compile-stubs/android/app/IApplicationThread.java"))
        .arg(root.join("probes/compile-stubs/android/app/IServiceConnection.java"))
        .arg(root.join("probes/compile-stubs/android/content/res/CompatibilityInfo.java"))
        .arg(root.join("probes/compile-stubs/android/content/ContentCaptureOptions.java"))
        .arg(root.join("probes/compile-stubs/android/view/autofill/AutofillManager.java"))
        .arg(root.join("probes/compile-stubs/android/view/InputChannel.java"));
    append_wm_sources(root, &mut javac)?;
    clear_wm_class_output(&class_dir)?;
    clear_am_class_output(&class_dir)?;
    run_command(&mut javac)?;

    let invoke_custom_generator_dir = build_dir.join("invoke-custom-generator");
    fs::create_dir_all(&invoke_custom_generator_dir)?;
    run_command(
        Command::new("javac")
            .args([
                "--add-exports",
                "java.base/jdk.internal.org.objectweb.asm=ALL-UNNAMED",
                "-d",
            ])
            .arg(&invoke_custom_generator_dir)
            .arg(root.join("probes/GenerateJitInvokeCustom.java"))
            .arg(root.join("probes/GenerateJitUnsafe.java")),
    )?;
    run_command(
        Command::new("java")
            .args([
                "--add-exports",
                "java.base/jdk.internal.org.objectweb.asm=ALL-UNNAMED",
                "-cp",
            ])
            .arg(&invoke_custom_generator_dir)
            .arg("GenerateJitInvokeCustom")
            .arg(&class_dir),
    )?;
    run_command(
        Command::new("java")
            .args([
                "--add-exports",
                "java.base/jdk.internal.org.objectweb.asm=ALL-UNNAMED",
                "-cp",
            ])
            .arg(&invoke_custom_generator_dir)
            .arg("GenerateJitUnsafe")
            .arg(&class_dir),
    )?;

    run_command(
        Command::new("unzip")
            .args(["-qq", "-o"])
            .arg(&android_mock_jar)
            .arg("android/test/mock/MockPackageManager.class")
            .arg("android/test/mock/MockContext.class")
            .arg("-d")
            .arg(&class_dir),
    )?;

    // The SDK's optional android.test.mock.jar is a compile-time stub: unlike
    // the device implementation, MockContext's constructor throws "Stub!".
    // Keep its exhaustive Context method surface, but make construction match
    // the real framework class so BaseContext can be the terminal (non-wrapper)
    // context beneath Application/ProbeContext.
    let mock_context_class = class_dir.join("android/test/mock/MockContext.class");
    let mut mock_context_bytes = fs::read(&mock_context_class)?;
    let throwing_constructor = [
        0x2a, 0xb7, 0x00, 0x01, 0xbb, 0x00, 0x07, 0x59, 0x12, 0x09, 0xb7, 0x00, 0x0b, 0xbf,
    ];
    let matches = mock_context_bytes
        .windows(throwing_constructor.len())
        .enumerate()
        .filter_map(|(offset, bytes)| (bytes == throwing_constructor).then_some(offset))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!(
            "unexpected API 36 MockContext constructor bytecode: matches={}",
            matches.len()
        )
        .into());
    }
    let constructor = &mut mock_context_bytes[matches[0]..matches[0] + throwing_constructor.len()];
    // Keep the bytecode linear so D8 does not require a new StackMapTable.
    constructor[4..].copy_from_slice(&[0x03, 0x57, 0x03, 0x57, 0x03, 0x57, 0x03, 0x57, 0x00, 0xb1]); // four iconst_0/pop pairs, nop, return
    fs::write(&mock_context_class, mock_context_bytes)?;

    let package_manager_class = class_dir.join("dev/darwinart/probe/ProbePackageManager.class");
    let mock_package_manager_class = class_dir.join("android/test/mock/MockPackageManager.class");

    let hello_class = class_dir.join("dev/darwinart/probe/Hello.class");
    let activity_class = class_dir.join("dev/darwinart/probe/ProbeActivity.class");
    let context_class = class_dir.join("dev/darwinart/probe/ProbeContext.class");
    let base_context_class = class_dir.join("dev/darwinart/probe/ProbeContext$BaseContext.class");
    let local_service_record_class =
        class_dir.join("dev/darwinart/probe/ProbeContext$LocalServiceRecord.class");
    let bound_service_record_class =
        class_dir.join("dev/darwinart/probe/ProbeContext$BoundServiceRecord.class");
    let remote_service_binder_class = class_dir.join("dev/darwinart/runtime/os/RemoteBinder.class");
    let audio_manager_class = class_dir.join("android/media/ProbeAudioManager.class");
    let compatibility_handler_class =
        class_dir.join("dev/darwinart/probe/ProbeContext$CompatibilityHandler.class");
    let default_service_handler_class =
        class_dir.join("dev/darwinart/probe/ProbeContext$DefaultServiceHandler.class");
    let thermal_service_handler_class =
        class_dir.join("dev/darwinart/probe/ProbeContext$ThermalServiceHandler.class");
    let main_executor_class = class_dir.join("dev/darwinart/probe/ProbeContext$MainExecutor.class");
    let preferences_class = class_dir.join("dev/darwinart/probe/ProbeSharedPreferences.class");
    let preferences_editor_class =
        class_dir.join("dev/darwinart/probe/ProbeSharedPreferences$EditorImpl.class");
    let shortcut_manager_class = class_dir.join("android/content/pm/ProbeShortcutManager.class");
    let user_manager_class = class_dir.join("android/os/ProbeUserManager.class");
    let resolver_class = class_dir.join("dev/darwinart/probe/ProbeContentResolver.class");
    let host_document_provider_class =
        class_dir.join("dev/darwinart/probe/ProbeHostDocumentProvider.class");
    let host_document_record_class =
        class_dir.join("dev/darwinart/probe/ProbeHostDocumentProvider$Document.class");
    let media_store_provider_class =
        class_dir.join("dev/darwinart/probe/ProbeMediaStoreProvider.class");
    let calendar_provider_class = class_dir.join("dev/darwinart/probe/ProbeCalendarProvider.class");
    let resources_class = class_dir.join("dev/darwinart/probe/ProbeResources.class");
    let xml_parser_class = class_dir.join("dev/darwinart/probe/ProbeXmlResourceParser.class");
    let canvas_class = class_dir.join("dev/darwinart/probe/ProbeCanvas.class");
    let view_class = class_dir.join("dev/darwinart/probe/ProbeView.class");
    let content_root_class = class_dir.join("dev/darwinart/probe/ProbeContentRoot.class");
    let upstream_test_harness_class =
        class_dir.join("dev/darwinart/probe/UpstreamTestHarness.class");
    let upstream_test_shutdown_hook_class =
        class_dir.join("dev/darwinart/probe/UpstreamTestHarness$OutputShutdownHook.class");
    let upstream_test_main_thread_class =
        class_dir.join("dev/darwinart/probe/UpstreamTestHarness$TestMainThread.class");
    let upstream_test_native_output_class =
        class_dir.join("dev/darwinart/probe/UpstreamTestHarness$NativeOutputStream.class");
    run_command(
        Command::new(find_d8()?)
            .args(["--min-api", "26", "--no-desugaring"])
            .arg("--lib")
            .arg(&android_platform_jar)
            .arg("--output")
            .arg(&unsafe_boot_dex_dir)
            .arg(class_dir.join("dev/darwinart/probe/JitUnsafe.class"))
            .arg(class_dir.join("java/lang/JitStringHidden.class"))
            .arg(class_dir.join("dev/darwinart/probe/JitMathDirect.class"))
            .arg(class_dir.join("libcore/io/JitMemoryDirect.class"))
            .arg(class_dir.join("java/lang/ref/JitReferenceDirect.class"))
            .arg(class_dir.join("java/lang/JitBoxingDirect.class"))
            .arg(class_dir.join("android/view/InputChannel.class")),
    )?;

    // Boot class path entries are ZIP/JAR containers on Android. Keep the
    // generated DEX payload unchanged, but publish a proper container so
    // libcore's ClassPathURLStreamHandler can resolve resources/classes when
    // a test exercises a custom class loader.
    let unsafe_boot_jar = build_dir.join("unsafe-boot-dex/unsafe-boot.jar");
    run_command(
        Command::new("zip")
            .args(["-q", "-j"])
            .arg(&unsafe_boot_jar)
            .arg(unsafe_boot_dex_dir.join("classes.dex")),
    )?;

    let mut d8 = Command::new(find_d8()?);
    d8.args(["--min-api", "26"])
        .arg("--lib")
        .arg(&android_platform_jar)
        .arg("--classpath")
        .arg(&class_dir)
        .arg("--classpath")
        .arg(&android_mock_jar)
        .arg("--output")
        .arg(&dex_dir)
        .arg(&hello_class)
        .arg(root.join("_build/package-dex-usage-runtime/package-dex-usage.jar"))
        .arg(class_dir.join("dev/darwinart/probe/JitInvokeCustom.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitConstructorParent.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitFinalReference.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitVirtualBase.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitCallable.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitVirtualChild.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitColdInitialization.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitColdStatic.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitFailedStaticRead.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitFailedStaticWrite.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitColdMoving.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitColdLong.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitColdDouble.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitColdReference.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitRecursiveInitialization.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitConcurrentInitialization.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitConcurrentFailedInitialization.class"))
        .arg(class_dir.join("dev/darwinart/probe/JitFailedInitialization.class"))
        .arg(&activity_class)
        .arg(&context_class)
        .arg(&base_context_class)
        .arg(&local_service_record_class)
        .arg(&bound_service_record_class)
        .arg(&remote_service_binder_class)
        .arg(class_dir.join("dev/darwinart/runtime/os/SystemServices.class"))
        .arg(class_dir.join("dev/darwinart/runtime/system/ServiceDirectory.class"))
        .arg(class_dir.join("dev/darwinart/runtime/display/BuiltInDisplayConfiguration.class"))
        .arg(class_dir.join("dev/darwinart/runtime/display/DefaultDisplayRegistry.class"))
        .arg(class_dir.join("dev/darwinart/runtime/display/DisplayManagerEndpoint.class"))
        .arg(class_dir.join("dev/darwinart/runtime/user/UserManagerEndpoint.class"))
        .arg(class_dir.join("dev/darwinart/runtime/content/SettingsProviderEndpoint.class"))
        .arg(class_dir.join("dev/darwinart/runtime/notification/NotificationManagerEndpoint.class"))
        .arg(class_dir.join("dev/darwinart/runtime/inputmethod/InputMethodManagerEndpoint.class"))
        .arg(class_dir.join("dev/darwinart/runtime/input/InputManagerEndpoint.class"))
        .arg(class_dir.join("dev/darwinart/runtime/input/InputDeviceRegistry.class"))
        .arg(class_dir.join("dev/darwinart/runtime/input/SystemKeyboardMaps.class"))
        .arg(&audio_manager_class)
        .arg(&compatibility_handler_class)
        .arg(&default_service_handler_class)
        .arg(&thermal_service_handler_class)
        .arg(&main_executor_class)
        .arg(&preferences_class)
        .arg(&preferences_editor_class)
        .arg(&shortcut_manager_class)
        .arg(&user_manager_class)
        .arg(&resolver_class)
        .arg(&host_document_provider_class)
        .arg(&host_document_record_class)
        .arg(&media_store_provider_class)
        .arg(&calendar_provider_class)
        .arg(&resources_class)
        .arg(&package_manager_class)
        .arg(class_dir.join("dev/darwinart/runtime/pm/InstalledPackageInfos.class"))
        .arg(class_dir.join("dev/darwinart/runtime/pm/InstalledPackageParser.class"))
        .arg(class_dir.join("dev/darwinart/runtime/pm/ServiceResolver.class"))
        .arg(class_dir.join("dev/darwinart/runtime/pm/InstalledPackageRecord.class"))
        .arg(class_dir.join("dev/darwinart/runtime/pm/PackageRecords.class"))
        .arg(class_dir.join("dev/darwinart/runtime/pm/PackageRecords$Source.class"))
        .arg(class_dir.join("dev/darwinart/runtime/pm/DexLoadReports.class"))
        .arg(class_dir.join("dev/darwinart/runtime/pm/PackageManagerEndpoint.class"))
        .arg(&mock_package_manager_class)
        .arg(&mock_context_class)
        .arg(&xml_parser_class)
        .arg(&canvas_class)
        .arg(&view_class)
        .arg(&content_root_class)
        .arg(&upstream_test_harness_class)
        .arg(&upstream_test_shutdown_hook_class)
        .arg(&upstream_test_main_thread_class)
        .arg(&upstream_test_native_output_class);
    append_wm_classes(&class_dir, &mut d8)?;
    append_am_classes(&class_dir, &mut d8)?;
    run_command(&mut d8)?;

    let libdexfile_external_include = libdexfile.join("external/include");
    let includes = [
        patched_artbase.as_path(),
        artbase.as_path(),
        libdexfile.as_path(),
        libdexfile_external_include.as_path(),
        libbase_include.as_path(),
        libziparchive_include.as_path(),
        palette_include.as_path(),
        Path::new("/opt/homebrew/include"),
        jni_include.as_path(),
        jni_darwin_include.as_path(),
    ];
    let dex_operator_source = build_dir.join("generated/dexfile_operator_out.cc");
    generate_operator_source(
        root,
        &libdexfile,
        &[
            "dex/dex_file.h",
            "dex/dex_file_layout.h",
            "dex/dex_instruction.h",
            "dex/dex_instruction_utils.h",
            "dex/invoke_type.h",
        ],
        &dex_operator_source,
    )?;
    let dex_sources = [
        dex_operator_source,
        // libunwindstack's AOSP dex adapter consumes the public ADexFile C
        // ABI. Keep the external implementation in the same provider archive
        // instead of relying on an accidental host symbol.
        libdexfile.join("external/dex_file_ext.cc"),
        libdexfile.join("dex/dex_file.cc"),
        libdexfile.join("dex/dex_file_loader.cc"),
        libdexfile.join("dex/standard_dex_file.cc"),
        libdexfile.join("dex/compact_dex_file.cc"),
        libdexfile.join("dex/compact_offset_table.cc"),
        libdexfile.join("dex/dex_file_verifier.cc"),
        libdexfile.join("dex/dex_file_exception_helpers.cc"),
        libdexfile.join("dex/dex_file_layout.cc"),
        libdexfile.join("dex/dex_file_tracking_registrar.cc"),
        libdexfile.join("dex/dex_instruction.cc"),
        libdexfile.join("dex/descriptors_names.cc"),
        libdexfile.join("dex/modifiers.cc"),
        libdexfile.join("dex/primitive.cc"),
        libdexfile.join("dex/signature.cc"),
        libdexfile.join("dex/type_lookup_table.cc"),
        libdexfile.join("dex/utf.cc"),
    ];
    let mut dex_objects = Vec::new();
    for source in dex_sources {
        dex_objects.push(compile_cpp(&source, &object_dir, &includes)?);
    }

    let dex_archive = build_dir.join("libdexfile-darwin.a");
    create_archive(&dex_archive, &dex_objects)?;

    let probe = build_dir.join("dex-probe");
    run_command(
        common_cpp_command(&includes)
            .arg(root.join("probes/dex_probe.cc"))
            .arg(&dex_archive)
            .arg(root.join("_build/foundation/libartbase-darwin.a"))
            .arg(root.join("_build/foundation/libandroid-base-darwin.a"))
            .arg(root.join("_build/foundation/libziparchive-darwin.a"))
            .args(["-Wl,-dead_strip", "-lz", "-o"])
            .arg(&probe),
    )?;

    let classes_dex = dex_dir.join("classes.dex");
    let output = command_output(Command::new(&probe).arg(&classes_dex))?;
    let _historical_manifest = "AOSP DEX: verified=yes version=35 classes=33 methods=756 \
                    class[0]=Landroid/content/pm/ProbeShortcutManager; \
                    class[1]=Landroid/media/ProbeAudioManager; \
                    class[2]=Landroid/os/ProbeUserManager; \
                    class[3]=Landroid/test/mock/MockPackageManager; \
                    class[4]=Ldev/darwinart/probe/Hello; \
                    class[5]=Ldev/darwinart/probe/ProbeActivity; \
                    class[6]=Ldev/darwinart/probe/ProbeCalendarProvider$$ExternalSyntheticBackport0; \
                    class[7]=Ldev/darwinart/probe/ProbeCalendarProvider$$ExternalSyntheticBackport1; \
                    class[8]=Ldev/darwinart/probe/ProbeCalendarProvider; \
                    class[9]=Ldev/darwinart/probe/ProbeCanvas; \
                    class[10]=Ldev/darwinart/probe/ProbeContentResolver$$ExternalSyntheticLambda0; \
                    class[11]=Ldev/darwinart/probe/ProbeContentResolver; \
                    class[12]=Ldev/darwinart/probe/ProbeContentRoot; \
                    class[13]=Ldev/darwinart/probe/ProbeContext$$ExternalSyntheticLambda0; \
                    class[14]=Ldev/darwinart/probe/ProbeContext$$ExternalSyntheticLambda1; \
                    class[15]=Ldev/darwinart/probe/ProbeContext$$ExternalSyntheticLambda2; \
                    class[16]=Ldev/darwinart/probe/ProbeContext$$ExternalSyntheticLambda3; \
                    class[17]=Ldev/darwinart/probe/ProbeContext$BoundServiceRecord; \
                    class[18]=Ldev/darwinart/probe/ProbeContext$CompatibilityHandler; \
                    class[19]=Ldev/darwinart/probe/ProbeContext$DefaultServiceHandler; \
                    class[20]=Ldev/darwinart/probe/ProbeContext$LocalServiceRecord; \
                    class[21]=Ldev/darwinart/probe/ProbeContext$MainExecutor; \
                    class[22]=Ldev/darwinart/probe/ProbeContext$RemoteServiceBinder; \
                    class[23]=Ldev/darwinart/probe/ProbeContext$ThermalServiceHandler; \
                    class[24]=Ldev/darwinart/probe/ProbeContext; \
                    class[25]=Ldev/darwinart/probe/ProbeHostDocumentProvider$Document; \
                    class[26]=Ldev/darwinart/probe/ProbeHostDocumentProvider; \
                    class[27]=Ldev/darwinart/probe/ProbePackageManager; \
                    class[28]=Ldev/darwinart/probe/ProbeResources; \
                    class[29]=Ldev/darwinart/probe/ProbeSharedPreferences$EditorImpl; \
                    class[30]=Ldev/darwinart/probe/ProbeSharedPreferences; \
                    class[31]=Ldev/darwinart/probe/ProbeView; \
                    class[32]=Ldev/darwinart/probe/ProbeXmlResourceParser;";
    verify_dex_contract(
        &output,
        193,
        3548,
        &[
            "Ldev/darwinart/probe/ProbeContext;",
            "Ldev/darwinart/probe/ProbeContext$BaseContext;",
            "Ldev/darwinart/probe/ProbeContext$BoundServiceRecord;",
            "Ldev/darwinart/runtime/os/RemoteBinder;",
            "Ldev/darwinart/runtime/os/SystemServices;",
            "Ldev/darwinart/runtime/system/ServiceDirectory;",
            "Lcom/android/server/pm/dex/DexUsageStore;",
            "Ldev/darwinart/runtime/am/ActivityManagerEndpoint;",
            "Ldev/darwinart/runtime/am/ActiveServices;",
            "Ldev/darwinart/runtime/am/SystemServiceBindings;",
            "Ldev/darwinart/runtime/am/ServiceRecord;",
            "Ldev/darwinart/runtime/am/IntentBindRecord;",
            "Ldev/darwinart/runtime/am/ConnectionRecord;",
            "Ldev/darwinart/runtime/am/ApplicationProcessRegistry;",
            "Ldev/darwinart/runtime/am/ApplicationProcessRegistry$InitialWork;",
            "Ldev/darwinart/runtime/am/ApplicationProcessRegistry$AttachedApplication;",
            "Ldev/darwinart/runtime/am/ActivityManagerClient;",
            "Ldev/darwinart/runtime/display/BuiltInDisplayConfiguration;",
            "Ldev/darwinart/runtime/display/DefaultDisplayRegistry;",
            "Ldev/darwinart/runtime/display/DisplayManagerEndpoint;",
            "Ldev/darwinart/runtime/wm/ActivityClientControllerEndpoint;",
            "Ldev/darwinart/runtime/wm/ActivityClientControllerEndpoint$ActivityRecord;",
            "Ldev/darwinart/runtime/wm/ActivityClientControllerEndpoint$State;",
            "Ldev/darwinart/runtime/wm/ActivityTaskManagerEndpoint;",
            "Ldev/darwinart/runtime/wm/WindowManagerEndpoint;",
            "Ldev/darwinart/runtime/wm/WindowSessionEndpoint;",
            "Ldev/darwinart/runtime/wm/WindowSurfaceRegistry;",
            "Ldev/darwinart/runtime/wm/WindowSurfaceRegistry$RelayoutPublication;",
            "Ldev/darwinart/runtime/wm/WindowInputPublisher;",
            "Ldev/darwinart/runtime/wm/DesktopWindowMetadataRegistry;",
            "Ldev/darwinart/runtime/wm/DesktopWindowMetadataEndpoint;",
            "Ldev/darwinart/runtime/wm/DesktopWindowMetadataClient;",
            "Ldev/darwinart/runtime/user/UserManagerEndpoint;",
            "Ldev/darwinart/runtime/content/SettingsProviderEndpoint;",
            "Ldev/darwinart/runtime/notification/NotificationManagerEndpoint;",
            "Ldev/darwinart/runtime/inputmethod/InputMethodManagerEndpoint;",
            "Ldev/darwinart/runtime/input/InputManagerEndpoint;",
            "Ldev/darwinart/runtime/input/InputDeviceRegistry;",
            "Ldev/darwinart/runtime/input/SystemKeyboardMaps;",
            "Ldev/darwinart/probe/JitInvokeCustom;",
            "Ldev/darwinart/probe/ProbePackageManager;",
            "Ldev/darwinart/probe/ProbeResources;",
        ],
    )?;

    super::verify_service_definitions_external(&output)?;
    let corrupt_dex = dex_dir.join("classes-corrupt.dex");
    let mut corrupt_bytes = fs::read(&classes_dex)?;
    let last = corrupt_bytes
        .last_mut()
        .ok_or("generated classes.dex was unexpectedly empty")?;
    *last ^= 0x01;
    fs::write(&corrupt_dex, corrupt_bytes)?;
    let rejected = Command::new(&probe).arg(&corrupt_dex).output()?;
    let rejected_stderr = String::from_utf8_lossy(&rejected.stderr);
    if rejected.status.success() || !rejected_stderr.contains("DEX verification failed") {
        return Err(format!(
            "corrupted DEX was not rejected as expected: status={} stderr={rejected_stderr:?}",
            rejected.status
        )
        .into());
    }

    println!("build-dex: {} corrupt=rejected", output.trim());
    Ok(())
}
