use super::fixture_framework_inputs::append_runtime_support_classes;
use super::*;

pub(crate) fn build_button_dex_probe(root: &Path) -> Result<()> {
    // Rebuild the baseline first: the Button flavor intentionally reuses the
    // launcher/context/resource test classes, but replaces only Activity/View
    // and adds the real SystemFonts bootstrap. Keeping a separate DEX prevents
    // widget dependencies from weakening the small baseline regression gate.
    build_dex_probe(root)?;

    let android_platform_jar = find_android_platform_jar()?;
    let android_mock_jar = android_platform_jar
        .parent()
        .ok_or("Android platform jar has no parent")?
        .join("optional/android.test.mock.jar");
    if !android_mock_jar.is_file() {
        return Err(format!(
            "Android mock library is missing: {}",
            android_mock_jar.display()
        )
        .into());
    }

    let baseline_classes = root.join("_build/dex-probe/classes");
    let build_dir = root.join("_build/button-dex");
    let class_dir = build_dir.join("classes");
    let dex_dir = build_dir.join("dex");
    // javac and d8 do not remove outputs for source types that were deleted or
    // renamed. Reusing these directories silently kept obsolete compatibility
    // classes in the product support DEX, so every build starts from an empty
    // generated-output boundary.
    if class_dir.exists() {
        fs::remove_dir_all(&class_dir)?;
    }
    if dex_dir.exists() {
        fs::remove_dir_all(&dex_dir)?;
    }
    fs::create_dir_all(&class_dir)?;
    fs::create_dir_all(&dex_dir)?;

    // The production runtime support owners, compiled by their own build;
    // the fixture packages all of them instead of keeping a source list.
    let support_classes = runtime_support_classes(root)?;
    let javac_classpath = env::join_paths([
        &android_platform_jar,
        &android_mock_jar,
        &baseline_classes,
        &support_classes,
    ])?;
    let mut javac = Command::new("javac");
    javac
        .args(["--release", "8", "-encoding", "UTF-8", "-d"])
        .arg(&class_dir)
        .arg("-classpath")
        .arg(&javac_classpath)
        .arg(root.join("probes/button/FontBootstrap.java"))
        .arg(root.join("probes/button/ProbeAnimationHost.java"))
        .arg(root.join("probes/button/ProbeActivity.java"))
        .arg(root.join("probes/button/ProbeView.java"))
        .arg(root.join("probes/apk_support/java/javax/microedition/khronos/egl/EGL.java"))
        .arg(root.join("probes/apk_support/java/javax/microedition/khronos/egl/EGL10.java"))
        .arg(root.join("probes/apk_support/java/javax/microedition/khronos/egl/EGLConfig.java"))
        .arg(root.join("probes/apk_support/java/javax/microedition/khronos/egl/EGLContext.java"))
        .arg(root.join("probes/apk_support/java/javax/microedition/khronos/egl/EGLDisplay.java"))
        .arg(root.join("probes/apk_support/java/javax/microedition/khronos/egl/EGLSurface.java"));
    run_command(&mut javac)?;

    let baseline = |relative: &str| baseline_classes.join(relative);
    let button = |relative: &str| class_dir.join(relative);
    let mut d8 = Command::new(find_d8()?);
    d8.args(["--min-api", "26"])
        .arg("--lib")
        .arg(&android_platform_jar)
        .arg("--classpath")
        .arg(&baseline_classes)
        .arg("--classpath")
        .arg(&support_classes)
        .arg("--classpath")
        .arg(&android_mock_jar)
        .arg("--output")
        .arg(&dex_dir)
        // PackageDexUsage's store, as the runtime support DEX packages it.
        .arg(root.join("_build/package-dex-usage-runtime/package-dex-usage.jar"))
        .arg(baseline("android/test/mock/MockPackageManager.class"))
        .arg(baseline("android/test/mock/MockContext.class"))
        .arg(baseline("android/content/pm/ProbeShortcutManager.class"))
        .arg(baseline("android/os/ProbeUserManager.class"))
        .arg(baseline("dev/aim/probe/Hello.class"))
        .arg(baseline("dev/aim/probe/UpstreamTestHarness.class"))
        .arg(baseline(
            "dev/aim/probe/UpstreamTestHarness$OutputShutdownHook.class",
        ))
        .arg(baseline(
            "dev/aim/probe/UpstreamTestHarness$TestMainThread.class",
        ))
        .arg(baseline(
            "dev/aim/probe/UpstreamTestHarness$NativeOutputStream.class",
        ))
        .arg(baseline("dev/aim/probe/JitInvokeCustom.class"))
        .arg(baseline("dev/aim/probe/JitConstructorParent.class"))
        .arg(baseline("dev/aim/probe/JitFinalReference.class"))
        .arg(baseline("dev/aim/probe/JitVirtualBase.class"))
        .arg(baseline("dev/aim/probe/JitCallable.class"))
        .arg(baseline("dev/aim/probe/JitVirtualChild.class"))
        .arg(baseline("dev/aim/probe/JitColdInitialization.class"))
        .arg(baseline("dev/aim/probe/JitColdStatic.class"))
        .arg(baseline("dev/aim/probe/JitFailedStaticRead.class"))
        .arg(baseline("dev/aim/probe/JitFailedStaticWrite.class"))
        .arg(baseline("dev/aim/probe/JitColdMoving.class"))
        .arg(baseline("dev/aim/probe/JitColdLong.class"))
        .arg(baseline("dev/aim/probe/JitColdDouble.class"))
        .arg(baseline("dev/aim/probe/JitColdReference.class"))
        .arg(baseline("dev/aim/probe/JitRecursiveInitialization.class"))
        .arg(baseline("dev/aim/probe/JitConcurrentInitialization.class"))
        .arg(baseline(
            "dev/aim/probe/JitConcurrentFailedInitialization.class",
        ))
        .arg(baseline("dev/aim/probe/JitFailedInitialization.class"))
        .arg(baseline("dev/aim/probe/ProbeCanvas.class"))
        .arg(baseline("android/media/ProbeAudioManager.class"))
        .arg(baseline("dev/aim/probe/ProbeCalendarProvider.class"))
        .arg(baseline("dev/aim/probe/ProbeContentResolver.class"))
        .arg(baseline("dev/aim/probe/ProbeHostDocumentProvider.class"))
        .arg(baseline(
            "dev/aim/probe/ProbeHostDocumentProvider$Document.class",
        ))
        .arg(baseline("dev/aim/probe/ProbeMediaStoreProvider.class"))
        .arg(baseline("dev/aim/probe/ProbeContentRoot.class"))
        .arg(baseline("dev/aim/probe/ProbeContext.class"))
        .arg(baseline("dev/aim/probe/ProbeContext$BaseContext.class"))
        .arg(baseline(
            "dev/aim/probe/ProbeContext$LocalServiceRecord.class",
        ))
        .arg(baseline(
            "dev/aim/probe/ProbeContext$BoundServiceRecord.class",
        ))
        .arg(baseline(
            "dev/aim/probe/ProbeContext$CompatibilityHandler.class",
        ))
        .arg(baseline(
            "dev/aim/probe/ProbeContext$DefaultServiceHandler.class",
        ))
        .arg(baseline(
            "dev/aim/probe/ProbeContext$ThermalServiceHandler.class",
        ))
        .arg(baseline("dev/aim/probe/ProbeContext$MainExecutor.class"))
        .arg(baseline("dev/aim/probe/ProbeSharedPreferences.class"))
        .arg(baseline(
            "dev/aim/probe/ProbeSharedPreferences$EditorImpl.class",
        ))
        .arg(baseline("dev/aim/probe/ProbePackageManager.class"))
        .arg(baseline("dev/aim/probe/ProbeResources.class"))
        .arg(baseline("dev/aim/probe/ProbeXmlResourceParser.class"))
        .arg(button("dev/aim/probe/FontBootstrap.class"))
        .arg(button("dev/aim/probe/ProbeAnimationHost.class"))
        .arg(button("dev/aim/probe/ProbeAnimationHost$1.class"))
        .arg(button("dev/aim/probe/ProbeActivity.class"))
        .arg(button("dev/aim/probe/ProbeView.class"))
        .arg(button("javax/microedition/khronos/egl/EGL.class"))
        .arg(button("javax/microedition/khronos/egl/EGL10.class"))
        .arg(button("javax/microedition/khronos/egl/EGLConfig.class"))
        .arg(button("javax/microedition/khronos/egl/EGLContext.class"))
        .arg(button("javax/microedition/khronos/egl/EGLDisplay.class"))
        .arg(button("javax/microedition/khronos/egl/EGLSurface.class"))
        .arg(button("javax/microedition/khronos/egl/DarwinEGL10.class"));
    append_runtime_support_classes(&support_classes, &mut d8)?;
    run_command(&mut d8)?;

    let classes_dex = dex_dir.join("classes.dex");
    let dex_probe = root.join("_build/dex-probe/dex-probe");
    let output = command_output(Command::new(&dex_probe).arg(&classes_dex))?;
    verify_dex_contract(
        &output,
        427,
        5547,
        &[
            "Ldev/aim/probe/ProbeActivity;",
            "Ldev/aim/probe/ProbeContext$BaseContext;",
            "Ldev/aim/runtime/os/RemoteBinder;",
            "Ldev/aim/runtime/os/SystemServices;",
            "Ldev/aim/runtime/system/ServiceDirectory;",
            "Ldev/aim/runtime/system/SystemServiceFactory;",
            "Ldev/aim/runtime/camera/CameraServiceEndpoint;",
            "Lcom/android/server/pm/dex/DexUsageStore;",
            "Ldev/aim/runtime/am/ActivityManagerEndpoint;",
            "Ldev/aim/runtime/am/ActiveServices;",
            "Ldev/aim/runtime/am/SystemServiceBindings;",
            "Ldev/aim/runtime/am/ServiceRecord;",
            "Ldev/aim/runtime/am/IntentBindRecord;",
            "Ldev/aim/runtime/am/ConnectionRecord;",
            "Ldev/aim/runtime/am/ApplicationProcessRegistry;",
            "Ldev/aim/runtime/am/ApplicationProcessRegistry$InitialWork;",
            "Ldev/aim/runtime/am/ApplicationProcessRegistry$AttachedApplication;",
            "Ldev/aim/runtime/am/ActivityManagerClient;",
            "Ldev/aim/runtime/display/BuiltInDisplayConfiguration;",
            "Ldev/aim/runtime/display/DefaultDisplayRegistry;",
            "Ldev/aim/runtime/display/DisplayManagerEndpoint;",
            "Ldev/aim/runtime/wm/ActivityClientControllerEndpoint;",
            "Ldev/aim/runtime/wm/ActivityClientControllerEndpoint$ActivityRecord;",
            "Ldev/aim/runtime/wm/ActivityClientControllerEndpoint$State;",
            "Ldev/aim/runtime/wm/ActivityTaskManagerEndpoint;",
            "Ldev/aim/runtime/wm/WindowManagerEndpoint;",
            "Ldev/aim/runtime/wm/WindowSessionEndpoint;",
            "Ldev/aim/runtime/wm/WindowSurfaceRegistry;",
            "Ldev/aim/runtime/wm/WindowSurfaceRegistry$RelayoutPublication;",
            "Ldev/aim/runtime/wm/WindowInputPublisher;",
            "Ldev/aim/runtime/wm/DesktopWindowMetadataRegistry;",
            "Ldev/aim/runtime/wm/DesktopWindowMetadataEndpoint;",
            "Ldev/aim/runtime/wm/DesktopWindowMetadataClient;",
            "Ldev/aim/runtime/user/UserManagerEndpoint;",
            "Ldev/aim/runtime/content/SettingsProviderEndpoint;",
            "Ldev/aim/runtime/content/ContentServiceEndpoint;",
            "Ldev/aim/runtime/content/ContentServiceEndpoint$ObserverRecord;",
            "Ldev/aim/runtime/content/ClipboardServiceEndpoint;",
            "Ldev/aim/runtime/content/ClipboardServiceEndpoint$ClipRecord;",
            "Ldev/aim/runtime/content/ClipboardServiceEndpoint$ListenerRecord;",
            "Ldev/aim/runtime/notification/NotificationManagerEndpoint;",
            "Ldev/aim/runtime/inputmethod/InputMethodManagerEndpoint;",
            "Ldev/aim/runtime/input/InputManagerEndpoint;",
            "Ldev/aim/runtime/input/InputDeviceRegistry;",
            "Ldev/aim/runtime/input/SystemKeyboardMaps;",
            "Ldev/aim/runtime/alarm/AlarmManagerEndpoint;",
            "Ldev/aim/runtime/shortcut/ShortcutManagerEndpoint;",
            "Ldev/aim/runtime/storage/StorageManagerEndpoint;",
            "Ldev/aim/runtime/admin/DevicePolicyManagerEndpoint;",
            "Ldev/aim/runtime/power/PowerStateProvider;",
            "Ldev/aim/runtime/power/DarwinPowerStateProvider;",
            "Ldev/aim/runtime/power/PowerManagerEndpoint;",
            "Ldev/aim/runtime/power/ThermalServiceEndpoint;",
            "Ldev/aim/runtime/connectivity/ConnectivityState;",
            "Ldev/aim/runtime/connectivity/ConnectivityCallbackRegistry;",
            "Ldev/aim/runtime/connectivity/ConnectivitySnapshot;",
            "Ldev/aim/runtime/connectivity/ConnectivityProjection;",
            "Ldev/aim/runtime/connectivity/ConnectivityPermissionEnforcer;",
            "Ldev/aim/runtime/connectivity/ConnectivityManagerEndpoint;",
            "Ldev/aim/runtime/connectivity/NetworkPathProvider;",
            "Ldev/aim/runtime/connectivity/ConnectivityServiceState;",
            "Ldev/aim/runtime/connectivity/NetworkValidationMonitor;",
            "Ldev/aim/runtime/connectivity/NetworkProbeTransport;",
            "Ldev/aim/runtime/uimode/UiModeManagerEndpoint;",
            "Ldev/aim/runtime/locale/LocaleManagerEndpoint;",
            "Ldev/aim/runtime/trust/TrustManagerEndpoint;",
            "Ldev/aim/runtime/usage/UsageStatsManagerEndpoint;",
            "Ldev/aim/runtime/restrictions/RestrictionsManagerEndpoint;",
            "Ldev/aim/runtime/job/JobSchedulerEndpoint;",
            "Ldev/aim/runtime/job/JobSchedulerService;",
            "Ldev/aim/runtime/job/JobServiceContext;",
            "Ldev/aim/probe/JitInvokeCustom;",
            "Ldev/aim/system/DarwinSystemServer;",
            "Ljavax/microedition/khronos/egl/DarwinEGL10;",
        ],
    )?;

    super::verify_service_definitions_external(&output)?;
    println!("build-button-dex: {}", output.trim());
    Ok(())
}
