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
        .arg(baseline("dev/darwinart/probe/Hello.class"))
        .arg(baseline("dev/darwinart/probe/UpstreamTestHarness.class"))
        .arg(baseline(
            "dev/darwinart/probe/UpstreamTestHarness$OutputShutdownHook.class",
        ))
        .arg(baseline(
            "dev/darwinart/probe/UpstreamTestHarness$TestMainThread.class",
        ))
        .arg(baseline(
            "dev/darwinart/probe/UpstreamTestHarness$NativeOutputStream.class",
        ))
        .arg(baseline("dev/darwinart/probe/JitInvokeCustom.class"))
        .arg(baseline("dev/darwinart/probe/JitConstructorParent.class"))
        .arg(baseline("dev/darwinart/probe/JitFinalReference.class"))
        .arg(baseline("dev/darwinart/probe/JitVirtualBase.class"))
        .arg(baseline("dev/darwinart/probe/JitCallable.class"))
        .arg(baseline("dev/darwinart/probe/JitVirtualChild.class"))
        .arg(baseline("dev/darwinart/probe/JitColdInitialization.class"))
        .arg(baseline("dev/darwinart/probe/JitColdStatic.class"))
        .arg(baseline("dev/darwinart/probe/JitFailedStaticRead.class"))
        .arg(baseline("dev/darwinart/probe/JitFailedStaticWrite.class"))
        .arg(baseline("dev/darwinart/probe/JitColdMoving.class"))
        .arg(baseline("dev/darwinart/probe/JitColdLong.class"))
        .arg(baseline("dev/darwinart/probe/JitColdDouble.class"))
        .arg(baseline("dev/darwinart/probe/JitColdReference.class"))
        .arg(baseline(
            "dev/darwinart/probe/JitRecursiveInitialization.class",
        ))
        .arg(baseline(
            "dev/darwinart/probe/JitConcurrentInitialization.class",
        ))
        .arg(baseline(
            "dev/darwinart/probe/JitConcurrentFailedInitialization.class",
        ))
        .arg(baseline(
            "dev/darwinart/probe/JitFailedInitialization.class",
        ))
        .arg(baseline("dev/darwinart/probe/ProbeCanvas.class"))
        .arg(baseline("android/media/ProbeAudioManager.class"))
        .arg(baseline("dev/darwinart/probe/ProbeCalendarProvider.class"))
        .arg(baseline("dev/darwinart/probe/ProbeContentResolver.class"))
        .arg(baseline(
            "dev/darwinart/probe/ProbeHostDocumentProvider.class",
        ))
        .arg(baseline(
            "dev/darwinart/probe/ProbeHostDocumentProvider$Document.class",
        ))
        .arg(baseline(
            "dev/darwinart/probe/ProbeMediaStoreProvider.class",
        ))
        .arg(baseline("dev/darwinart/probe/ProbeContentRoot.class"))
        .arg(baseline("dev/darwinart/probe/ProbeContext.class"))
        .arg(baseline(
            "dev/darwinart/probe/ProbeContext$BaseContext.class",
        ))
        .arg(baseline(
            "dev/darwinart/probe/ProbeContext$LocalServiceRecord.class",
        ))
        .arg(baseline(
            "dev/darwinart/probe/ProbeContext$BoundServiceRecord.class",
        ))
        .arg(baseline(
            "dev/darwinart/probe/ProbeContext$CompatibilityHandler.class",
        ))
        .arg(baseline(
            "dev/darwinart/probe/ProbeContext$DefaultServiceHandler.class",
        ))
        .arg(baseline(
            "dev/darwinart/probe/ProbeContext$ThermalServiceHandler.class",
        ))
        .arg(baseline(
            "dev/darwinart/probe/ProbeContext$MainExecutor.class",
        ))
        .arg(baseline("dev/darwinart/probe/ProbeSharedPreferences.class"))
        .arg(baseline(
            "dev/darwinart/probe/ProbeSharedPreferences$EditorImpl.class",
        ))
        .arg(baseline("dev/darwinart/probe/ProbePackageManager.class"))
        .arg(baseline("dev/darwinart/probe/ProbeResources.class"))
        .arg(baseline("dev/darwinart/probe/ProbeXmlResourceParser.class"))
        .arg(button("dev/darwinart/probe/FontBootstrap.class"))
        .arg(button("dev/darwinart/probe/ProbeAnimationHost.class"))
        .arg(button("dev/darwinart/probe/ProbeAnimationHost$1.class"))
        .arg(button("dev/darwinart/probe/ProbeActivity.class"))
        .arg(button("dev/darwinart/probe/ProbeView.class"))
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
            "Ldev/darwinart/probe/ProbeActivity;",
            "Ldev/darwinart/probe/ProbeContext$BaseContext;",
            "Ldev/darwinart/runtime/os/RemoteBinder;",
            "Ldev/darwinart/runtime/os/SystemServices;",
            "Ldev/darwinart/runtime/system/ServiceDirectory;",
            "Ldev/darwinart/runtime/system/SystemServiceFactory;",
            "Ldev/darwinart/runtime/camera/CameraServiceEndpoint;",
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
            "Ldev/darwinart/runtime/content/ContentServiceEndpoint;",
            "Ldev/darwinart/runtime/content/ContentServiceEndpoint$ObserverRecord;",
            "Ldev/darwinart/runtime/content/ClipboardServiceEndpoint;",
            "Ldev/darwinart/runtime/content/ClipboardServiceEndpoint$ClipRecord;",
            "Ldev/darwinart/runtime/content/ClipboardServiceEndpoint$ListenerRecord;",
            "Ldev/darwinart/runtime/notification/NotificationManagerEndpoint;",
            "Ldev/darwinart/runtime/inputmethod/InputMethodManagerEndpoint;",
            "Ldev/darwinart/runtime/input/InputManagerEndpoint;",
            "Ldev/darwinart/runtime/input/InputDeviceRegistry;",
            "Ldev/darwinart/runtime/input/SystemKeyboardMaps;",
            "Ldev/darwinart/runtime/alarm/AlarmManagerEndpoint;",
            "Ldev/darwinart/runtime/shortcut/ShortcutManagerEndpoint;",
            "Ldev/darwinart/runtime/storage/StorageManagerEndpoint;",
            "Ldev/darwinart/runtime/admin/DevicePolicyManagerEndpoint;",
            "Ldev/darwinart/runtime/power/PowerStateProvider;",
            "Ldev/darwinart/runtime/power/DarwinPowerStateProvider;",
            "Ldev/darwinart/runtime/power/PowerManagerEndpoint;",
            "Ldev/darwinart/runtime/power/ThermalServiceEndpoint;",
            "Ldev/darwinart/runtime/connectivity/ConnectivityState;",
            "Ldev/darwinart/runtime/connectivity/ConnectivityCallbackRegistry;",
            "Ldev/darwinart/runtime/connectivity/ConnectivitySnapshot;",
            "Ldev/darwinart/runtime/connectivity/ConnectivityProjection;",
            "Ldev/darwinart/runtime/connectivity/ConnectivityPermissionEnforcer;",
            "Ldev/darwinart/runtime/connectivity/ConnectivityManagerEndpoint;",
            "Ldev/darwinart/runtime/connectivity/NetworkPathProvider;",
            "Ldev/darwinart/runtime/connectivity/ConnectivityServiceState;",
            "Ldev/darwinart/runtime/connectivity/NetworkValidationMonitor;",
            "Ldev/darwinart/runtime/connectivity/NetworkProbeTransport;",
            "Ldev/darwinart/runtime/uimode/UiModeManagerEndpoint;",
            "Ldev/darwinart/runtime/locale/LocaleManagerEndpoint;",
            "Ldev/darwinart/runtime/trust/TrustManagerEndpoint;",
            "Ldev/darwinart/runtime/usage/UsageStatsManagerEndpoint;",
            "Ldev/darwinart/runtime/restrictions/RestrictionsManagerEndpoint;",
            "Ldev/darwinart/runtime/job/JobSchedulerEndpoint;",
            "Ldev/darwinart/runtime/job/JobSchedulerService;",
            "Ldev/darwinart/runtime/job/JobServiceContext;",
            "Ldev/darwinart/probe/JitInvokeCustom;",
            "Ldev/darwinart/system/DarwinSystemServer;",
            "Ljavax/microedition/khronos/egl/DarwinEGL10;",
        ],
    )?;

    super::verify_service_definitions_external(&output)?;
    println!("build-button-dex: {}", output.trim());
    Ok(())
}
