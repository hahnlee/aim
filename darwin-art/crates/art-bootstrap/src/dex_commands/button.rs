use super::*;

fn collect_class_tree(
    class_root: &Path,
    directory: &Path,
    classes: &mut BTreeMap<PathBuf, PathBuf>,
) -> Result<()> {
    if !directory.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_class_tree(class_root, &path, classes)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("class") {
            classes.insert(path.strip_prefix(class_root)?.to_owned(), path);
        }
    }
    Ok(())
}

fn build_runtime_support_dex(
    root: &Path,
    android_platform_jar: &Path,
    baseline_classes: &Path,
    button_classes: &Path,
) -> Result<()> {
    let build_dir = root.join("_build/runtime-support-dex");
    let dex_dir = build_dir.join("dex");
    if dex_dir.exists() {
        fs::remove_dir_all(&dex_dir)?;
    }
    fs::create_dir_all(&dex_dir)?;

    // Production application and system processes may see only Android-facing
    // runtime contracts. The probe Activity/Context and the retired local
    // DarwinServiceBridge remain in button-dex for isolated tests, never in
    // the support class path used to launch an installed APK.
    let mut classes = BTreeMap::new();
    for directory in [
        baseline_classes.join("dev/darwinart/runtime"),
        button_classes.join("dev/darwinart/runtime"),
        button_classes.join("dev/darwinart/system"),
        button_classes.join("javax/microedition/khronos/egl"),
    ] {
        let class_root = if directory.starts_with(baseline_classes) {
            baseline_classes
        } else {
            button_classes
        };
        collect_class_tree(class_root, &directory, &mut classes)?;
    }

    let mut d8 = Command::new(find_d8()?);
    d8.arg(root.join("_build/package-dex-usage-runtime/package-dex-usage.jar"))
        .args(["--min-api", "26"])
        .arg("--lib")
        .arg(android_platform_jar)
        .arg("--output")
        .arg(&dex_dir);
    for class in classes.values() {
        d8.arg(class);
    }
    run_command(&mut d8)?;

    let classes_dex = dex_dir.join("classes.dex");
    let dex_probe = root.join("_build/dex-probe/dex-probe");
    let output = command_output(Command::new(&dex_probe).arg(&classes_dex))?;
    for forbidden in [
        "Ldev/darwinart/probe/",
        "Ldev/darwinart/simple/DarwinServiceBridge",
    ] {
        if output.contains(forbidden) {
            return Err(format!("production support DEX contains {forbidden}: {output}").into());
        }
    }
    for required in [
        "Ldev/darwinart/runtime/am/ActivityManagerEndpoint;",
        "Ldev/darwinart/runtime/wm/ActivityTaskManagerEndpoint;",
        "Ldev/darwinart/runtime/wm/WindowManagerEndpoint;",
        "Ldev/darwinart/runtime/power/PowerStateProvider;",
        "Ldev/darwinart/runtime/power/DarwinPowerStateProvider;",
        "Ldev/darwinart/runtime/power/PowerManagerEndpoint;",
        "Ldev/darwinart/runtime/power/ThermalServiceEndpoint;",
        "Ldev/darwinart/runtime/connectivity/ConnectivityServiceState;",
        "Ldev/darwinart/runtime/connectivity/NetworkValidationMonitor;",
        "Ldev/darwinart/runtime/connectivity/NetworkProbeTransport;",
        "Ldev/darwinart/runtime/appops/AppOpsServiceEndpoint;",
        "Ldev/darwinart/runtime/uimode/UiModeManagerEndpoint;",
        "Ldev/darwinart/runtime/locale/LocaleManagerEndpoint;",
        "Ldev/darwinart/runtime/trust/TrustManagerEndpoint;",
        "Ldev/darwinart/runtime/usage/UsageStatsManagerEndpoint;",
        "Ldev/darwinart/runtime/restrictions/RestrictionsManagerEndpoint;",
        "Ldev/darwinart/runtime/os/SystemServices;",
        "Ldev/darwinart/system/DarwinSystemServer;",
        "Ljavax/microedition/khronos/egl/DarwinEGL10;",
    ] {
        if !output.contains(required) {
            return Err(format!("production support DEX is missing {required}: {output}").into());
        }
    }
    println!("build-runtime-support-dex: {}", output.trim());
    Ok(())
}

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

    let javac_classpath =
        env::join_paths([&android_platform_jar, &android_mock_jar, &baseline_classes])?;
    run_command(
        Command::new("javac")
            .args(["--release", "8", "-encoding", "UTF-8", "-d"])
            .arg(&class_dir)
            .arg("-classpath")
            .arg(&javac_classpath)
            .arg(root.join("probes/button/FontBootstrap.java"))
            .arg(root.join("probes/button/ProbeAnimationHost.java"))
            .arg(root.join("probes/button/ProbeActivity.java"))
            .arg(root.join("probes/button/ProbeView.java"))
            .arg(root.join("tools/android-apk-app-runtime/fixture/DarwinServiceBridge.java"))
            .arg(root.join("runtime/framework/system/DarwinSystemServer.java"))
            .arg(root.join("runtime/framework/system/SystemServiceFactory.java"))
            .arg(root.join("runtime/framework/appops/AppOpsServiceEndpoint.java"))
            .arg(root.join("runtime/framework/usage/UsageStatsManagerEndpoint.java"))
            .arg(root.join("runtime/framework/restrictions/RestrictionsManagerEndpoint.java"))
            .arg(root.join("runtime/framework/job/JobSchedulerEndpoint.java"))
            .arg(root.join("runtime/framework/job/JobSchedulerService.java"))
            .arg(root.join("runtime/framework/job/JobRecord.java"))
            .arg(root.join("runtime/framework/job/JobServiceContext.java"))
            .arg(root.join("runtime/framework/audio/AudioServiceEndpoint.java"))
            .arg(root.join("runtime/framework/camera/CameraServiceEndpoint.java"))
            .arg(root.join("runtime/framework/alarm/AlarmManagerEndpoint.java"))
            .arg(root.join("runtime/framework/shortcut/ShortcutManagerEndpoint.java"))
            .arg(root.join("runtime/framework/storage/StorageManagerEndpoint.java"))
            .arg(root.join("runtime/framework/admin/DevicePolicyManagerEndpoint.java"))
            .arg(root.join("runtime/framework/power/PowerStateProvider.java"))
            .arg(root.join("runtime/framework/power/DarwinPowerStateProvider.java"))
            .arg(root.join("runtime/framework/power/PowerManagerEndpoint.java"))
            .arg(root.join("runtime/framework/power/ThermalServiceEndpoint.java"))
            .arg(root.join("runtime/framework/trust/TrustManagerEndpoint.java"))
            .arg(root.join("runtime/framework/connectivity/ConnectivityState.java"))
            .arg(root.join("runtime/framework/connectivity/ConnectivityCallbackRegistry.java"))
            .arg(root.join("runtime/framework/connectivity/ConnectivitySnapshot.java"))
            .arg(root.join("runtime/framework/connectivity/ConnectivityProjection.java"))
            .arg(root.join("runtime/framework/connectivity/ConnectivityPermissionEnforcer.java"))
            .arg(root.join(
                "runtime/framework/connectivity/InstalledConnectivityPermissionEnforcer.java",
            ))
            .arg(root.join("runtime/framework/connectivity/ConnectivityManagerEndpoint.java"))
            .arg(root.join("runtime/framework/connectivity/NetworkPathProvider.java"))
            .arg(root.join("runtime/framework/connectivity/NetworkProbeTransport.java"))
            .arg(root.join("runtime/framework/connectivity/HttpNetworkProbeTransport.java"))
            .arg(root.join("runtime/framework/connectivity/NetworkValidationMonitor.java"))
            .arg(root.join("runtime/framework/connectivity/ConnectivityServiceState.java"))
            .arg(root.join("runtime/framework/compile-stubs/android/net/NetworkCapabilities.java"))
            .arg(root.join("runtime/framework/uimode/UiModeManagerEndpoint.java"))
            .arg(root.join("runtime/framework/locale/LocaleManagerEndpoint.java"))
            .arg(root.join("probes/apk_support/java/javax/microedition/khronos/egl/EGL.java"))
            .arg(root.join("probes/apk_support/java/javax/microedition/khronos/egl/EGL10.java"))
            .arg(root.join("probes/apk_support/java/javax/microedition/khronos/egl/EGLConfig.java"))
            .arg(
                root.join("probes/apk_support/java/javax/microedition/khronos/egl/EGLContext.java"),
            )
            .arg(
                root.join("probes/apk_support/java/javax/microedition/khronos/egl/EGLDisplay.java"),
            )
            .arg(
                root.join("probes/apk_support/java/javax/microedition/khronos/egl/EGLSurface.java"),
            ),
    )?;

    let baseline = |relative: &str| baseline_classes.join(relative);
    let button = |relative: &str| class_dir.join(relative);
    run_command(
        Command::new(find_d8()?)
            .arg(root.join("_build/package-dex-usage-runtime/package-dex-usage.jar"))
            .args(["--min-api", "26"])
            .arg("--lib")
            .arg(&android_platform_jar)
            .arg("--classpath")
            .arg(&baseline_classes)
            .arg("--classpath")
            .arg(&android_mock_jar)
            .arg("--output")
            .arg(&dex_dir)
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
            .arg(baseline("dev/darwinart/probe/JitRecursiveInitialization.class"))
            .arg(baseline("dev/darwinart/probe/JitConcurrentInitialization.class"))
            .arg(baseline("dev/darwinart/probe/JitConcurrentFailedInitialization.class"))
            .arg(baseline("dev/darwinart/probe/JitFailedInitialization.class"))
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
                "dev/darwinart/runtime/os/RemoteBinder.class",
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
            .arg(baseline("dev/darwinart/runtime/pm/InstalledApplicationInfo.class"))
            .arg(baseline("dev/darwinart/runtime/pm/InstalledManifestMetadata.class"))
            .arg(baseline("dev/darwinart/runtime/pm/InstalledResourceValue.class"))
            .arg(baseline("dev/darwinart/runtime/pm/InstalledActivityInfo.class"))
            .arg(baseline("dev/darwinart/runtime/pm/InstalledPackageInfo.class"))
            .arg(baseline("dev/darwinart/runtime/pm/InstalledPackageRecord.class"))
            .arg(baseline("dev/darwinart/runtime/pm/PackageRecords.class"))
            .arg(baseline("dev/darwinart/runtime/pm/PackageRecords$Source.class"))
            .arg(baseline("dev/darwinart/runtime/pm/DexLoadReports.class"))
            .arg(baseline("dev/darwinart/runtime/pm/PackageManagerEndpoint.class"))
            .arg(baseline("dev/darwinart/runtime/os/SystemServices.class"))
            .arg(baseline("dev/darwinart/runtime/system/ServiceDirectory.class"))
            .arg(button("dev/darwinart/runtime/system/SystemServiceFactory.class"))
            .arg(button("dev/darwinart/runtime/camera/CameraServiceEndpoint.class"))
            .arg(baseline(
                "dev/darwinart/runtime/am/ApplicationProcessRegistry.class",
            ))
            .arg(baseline(
                "dev/darwinart/runtime/am/ApplicationProcessRegistry$1.class",
            ))
            .arg(baseline(
                "dev/darwinart/runtime/am/ApplicationProcessRegistry$InitialWork.class",
            ))
            .arg(baseline(
                "dev/darwinart/runtime/am/ApplicationProcessRegistry$ProcessRecord.class",
            ))
            .arg(baseline(
                "dev/darwinart/runtime/am/ApplicationProcessRegistry$AttachedApplication.class",
            ))
            .arg(baseline("dev/darwinart/runtime/am/ActivityManagerEndpoint.class"))
            .arg(baseline("dev/darwinart/runtime/am/ActiveServices.class"))
            .arg(baseline("dev/darwinart/runtime/am/SystemServiceBindings.class"))
            .arg(baseline(
                "dev/darwinart/runtime/am/ActiveServices$ProcessLauncher.class",
            ))
            .arg(baseline("dev/darwinart/runtime/am/ServiceRecord.class"))
            .arg(baseline("dev/darwinart/runtime/am/IntentBindRecord.class"))
            .arg(baseline("dev/darwinart/runtime/am/ConnectionRecord.class"))
            .arg(baseline("dev/darwinart/runtime/am/ActivityManagerClient.class"))
            .arg(baseline(
                "dev/darwinart/runtime/display/BuiltInDisplayConfiguration.class",
            ))
            .arg(baseline("dev/darwinart/runtime/display/DefaultDisplayRegistry.class"))
            .arg(baseline("dev/darwinart/runtime/display/DisplayManagerEndpoint.class"))
            .arg(baseline(
                "dev/darwinart/runtime/wm/ActivityClientControllerEndpoint.class",
            ))
            .arg(baseline(
                "dev/darwinart/runtime/wm/ActivityTaskManagerEndpoint.class",
            ))
            .arg(baseline(
                "dev/darwinart/runtime/wm/ActivityClientControllerEndpoint$ActivityRecord.class",
            ))
            .arg(baseline(
                "dev/darwinart/runtime/wm/ActivityClientControllerEndpoint$State.class",
            ))
            .arg(baseline("dev/darwinart/runtime/wm/WindowManagerEndpoint.class"))
            .arg(baseline("dev/darwinart/runtime/wm/WindowSessionEndpoint.class"))
            .arg(baseline("dev/darwinart/runtime/wm/WindowSurfaceRegistry.class"))
            .arg(baseline("dev/darwinart/runtime/wm/WindowInputPublisher.class"))
            .arg(baseline("dev/darwinart/runtime/wm/DesktopWindowMetadataRegistry.class"))
            .arg(baseline("dev/darwinart/runtime/wm/DesktopWindowMetadataRegistry$WindowState.class"))
            .arg(baseline("dev/darwinart/runtime/wm/DesktopWindowMetadataRegistry$ProcessState.class"))
            .arg(baseline("dev/darwinart/runtime/wm/DesktopWindowMetadataEndpoint.class"))
            .arg(baseline("dev/darwinart/runtime/wm/DesktopWindowMetadataClient.class"))
            .arg(baseline("dev/darwinart/runtime/wm/DesktopWindowMetadataClient$1.class"))
            .arg(baseline("dev/darwinart/runtime/user/UserManagerEndpoint.class"))
            .arg(baseline(
                "dev/darwinart/runtime/content/SettingsProviderEndpoint.class",
            ))
            .arg(baseline(
                "dev/darwinart/runtime/content/ContentServiceEndpoint.class",
            ))
            .arg(baseline(
                "dev/darwinart/runtime/content/ContentServiceEndpoint$ObserverRecord.class",
            ))
            .arg(baseline(
                "dev/darwinart/runtime/content/ClipboardServiceEndpoint.class",
            ))
            .arg(baseline(
                "dev/darwinart/runtime/content/ClipboardServiceEndpoint$ClipRecord.class",
            ))
            .arg(baseline(
                "dev/darwinart/runtime/content/ClipboardServiceEndpoint$ListenerRecord.class",
            ))
            .arg(baseline(
                "dev/darwinart/runtime/notification/NotificationManagerEndpoint.class",
            ))
            .arg(baseline(
                "dev/darwinart/runtime/inputmethod/InputMethodManagerEndpoint.class",
            ))
            .arg(baseline("dev/darwinart/runtime/input/InputManagerEndpoint.class"))
            .arg(baseline("dev/darwinart/runtime/input/InputDeviceRegistry.class"))
            .arg(button("dev/darwinart/runtime/audio/AudioServiceEndpoint.class"))
            .arg(button("dev/darwinart/runtime/alarm/AlarmManagerEndpoint.class"))
            .arg(button("dev/darwinart/runtime/shortcut/ShortcutManagerEndpoint.class"))
            .arg(button("dev/darwinart/runtime/storage/StorageManagerEndpoint.class"))
            .arg(button("dev/darwinart/runtime/admin/DevicePolicyManagerEndpoint.class"))
            .arg(button("dev/darwinart/runtime/power/PowerStateProvider.class"))
            .arg(button("dev/darwinart/runtime/power/DarwinPowerStateProvider.class"))
            .arg(button("dev/darwinart/runtime/power/PowerManagerEndpoint.class"))
            .arg(button("dev/darwinart/runtime/power/ThermalServiceEndpoint.class"))
            .arg(button("dev/darwinart/runtime/appops/AppOpsServiceEndpoint.class"))
            .arg(button(
                "dev/darwinart/runtime/restrictions/RestrictionsManagerEndpoint.class",
            ))
            .arg(button("dev/darwinart/runtime/trust/TrustManagerEndpoint.class"))
            .arg(button("dev/darwinart/runtime/connectivity/ConnectivityState.class"))
            .arg(button(
                "dev/darwinart/runtime/connectivity/ConnectivityState$Listener.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/ConnectivityCallbackRegistry.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/ConnectivityCallbackRegistry$1.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/ConnectivityCallbackRegistry$2.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/ConnectivityCallbackRegistry$Registration.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/ConnectivitySnapshot.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/ConnectivityProjection.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/ConnectivityPermissionEnforcer.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/InstalledConnectivityPermissionEnforcer.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/ConnectivityManagerEndpoint.class",
            ))
            .arg(button("dev/darwinart/runtime/connectivity/NetworkPathProvider.class"))
            .arg(button(
                "dev/darwinart/runtime/connectivity/NetworkProbeTransport.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/NetworkProbeTransport$Result.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/HttpNetworkProbeTransport.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/NetworkValidationMonitor.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/NetworkValidationMonitor$Callback.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/NetworkValidationMonitor$1.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/NetworkValidationMonitor$2.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/NetworkValidationMonitor$2$1.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/ConnectivityServiceState.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/ConnectivityServiceState$1.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/ConnectivityServiceState$2.class",
            ))
            .arg(button(
                "dev/darwinart/runtime/connectivity/ConnectivityServiceState$3.class",
            ))
            .arg(button("dev/darwinart/runtime/uimode/UiModeManagerEndpoint.class"))
            .arg(button("dev/darwinart/runtime/locale/LocaleManagerEndpoint.class"))
            .arg(button("dev/darwinart/runtime/usage/UsageStatsManagerEndpoint.class"))
            .arg(button("dev/darwinart/runtime/job/JobSchedulerEndpoint.class"))
            .arg(button("dev/darwinart/runtime/job/JobSchedulerService.class"))
            .arg(button("dev/darwinart/runtime/job/JobSchedulerService$Key.class"))
            .arg(button("dev/darwinart/runtime/job/JobRecord.class"))
            .arg(button("dev/darwinart/runtime/job/JobRecord$State.class"))
            .arg(button("dev/darwinart/runtime/job/JobServiceContext.class"))
            .arg(button("dev/darwinart/runtime/job/JobServiceContext$Listener.class"))
            .arg(button("dev/darwinart/runtime/job/JobServiceContext$Callback.class"))
            .arg(button(
                "dev/darwinart/runtime/job/JobServiceContext$ServiceConnection.class",
            ))
            .arg(baseline("dev/darwinart/runtime/pm/InstalledServiceInfo.class"))
            .arg(baseline("dev/darwinart/probe/ProbeResources.class"))
            .arg(baseline("dev/darwinart/probe/ProbeXmlResourceParser.class"))
            .arg(button("dev/darwinart/probe/FontBootstrap.class"))
            .arg(button("dev/darwinart/probe/ProbeAnimationHost.class"))
            .arg(button("dev/darwinart/probe/ProbeAnimationHost$1.class"))
            .arg(button("dev/darwinart/probe/ProbeActivity.class"))
            .arg(button("dev/darwinart/probe/ProbeView.class"))
            .arg(button("dev/darwinart/simple/DarwinServiceBridge.class"))
            .arg(button("dev/darwinart/system/DarwinSystemServer.class"))
            .arg(button(
                "dev/darwinart/system/DarwinSystemServer$PackageRegistryBinder.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$ManagerHandler.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$JobSchedulerHandler.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$MediaSessionInterfaceHandler.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$UserManagerHandler.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$WindowManagerHandler.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$WindowManagerHandler$WindowLayoutState.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$DisplayHandler.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$ActivityTaskHandler.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$ActivityClientHandler.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$ActivityRecord.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$ActivityManagerHandler.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$AudioServiceBinder.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$DevicePolicyServiceBinder.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$HostSurfaceState.class",
            ))
            .arg(button(
                "dev/darwinart/simple/DarwinServiceBridge$IntentSenderHandler.class",
            ))
            .arg(button("javax/microedition/khronos/egl/EGL.class"))
            .arg(button("javax/microedition/khronos/egl/EGL10.class"))
            .arg(button("javax/microedition/khronos/egl/EGLConfig.class"))
            .arg(button("javax/microedition/khronos/egl/EGLContext.class"))
            .arg(button("javax/microedition/khronos/egl/EGLDisplay.class"))
            .arg(button("javax/microedition/khronos/egl/EGLSurface.class"))
            .arg(button("javax/microedition/khronos/egl/DarwinEGL10.class")),
    )?;

    let classes_dex = dex_dir.join("classes.dex");
    let dex_probe = root.join("_build/dex-probe/dex-probe");
    let output = command_output(Command::new(&dex_probe).arg(&classes_dex))?;
    let _historical_manifest = "AOSP DEX: verified=yes version=35 classes=77 methods=1075 \
                    class[0]=Landroid/content/pm/ProbeShortcutManager; \
                    class[1]=Landroid/media/ProbeAudioManager; \
                    class[2]=Landroid/os/ProbeUserManager; \
                    class[3]=Landroid/test/mock/MockPackageManager; \
                    class[4]=Ldev/darwinart/probe/FontBootstrap; \
                    class[5]=Ldev/darwinart/probe/Hello; \
                    class[6]=Ldev/darwinart/probe/ProbeActivity; \
                    class[7]=Ldev/darwinart/probe/ProbeAnimationHost$$ExternalSyntheticLambda0; \
                    class[8]=Ldev/darwinart/probe/ProbeAnimationHost$1; \
                    class[9]=Ldev/darwinart/probe/ProbeAnimationHost; \
                    class[10]=Ldev/darwinart/probe/ProbeCalendarProvider$$ExternalSyntheticBackport0; \
                    class[11]=Ldev/darwinart/probe/ProbeCalendarProvider$$ExternalSyntheticBackport1; \
                    class[12]=Ldev/darwinart/probe/ProbeCalendarProvider; \
                    class[13]=Ldev/darwinart/probe/ProbeCanvas; \
                    class[14]=Ldev/darwinart/probe/ProbeContentResolver$$ExternalSyntheticLambda0; \
                    class[15]=Ldev/darwinart/probe/ProbeContentResolver; \
                    class[16]=Ldev/darwinart/probe/ProbeContentRoot; \
                    class[17]=Ldev/darwinart/probe/ProbeContext$$ExternalSyntheticLambda0; \
                    class[18]=Ldev/darwinart/probe/ProbeContext$$ExternalSyntheticLambda1; \
                    class[19]=Ldev/darwinart/probe/ProbeContext$CompatibilityHandler; \
                    class[20]=Ldev/darwinart/probe/ProbeContext$DefaultServiceHandler; \
                    class[21]=Ldev/darwinart/probe/ProbeContext$LocalServiceRecord; \
                    class[22]=Ldev/darwinart/probe/ProbeContext$MainExecutor; \
                    class[23]=Ldev/darwinart/probe/ProbeContext$ThermalServiceHandler; \
                    class[24]=Ldev/darwinart/probe/ProbeContext; \
                    class[25]=Ldev/darwinart/probe/ProbeHostDocumentProvider$Document; \
                    class[26]=Ldev/darwinart/probe/ProbeHostDocumentProvider; \
                    class[27]=Ldev/darwinart/probe/ProbePackageManager; \
                    class[28]=Ldev/darwinart/probe/ProbeResources; \
                    class[29]=Ldev/darwinart/probe/ProbeSharedPreferences$EditorImpl; \
                    class[30]=Ldev/darwinart/probe/ProbeSharedPreferences; \
                    class[31]=Ldev/darwinart/probe/ProbeView; \
                    class[32]=Ldev/darwinart/probe/ProbeXmlResourceParser; \
                    class[33]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda0; \
                    class[34]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda10; \
                    class[35]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda11; \
                    class[36]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda12; \
                    class[37]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda13; \
                    class[38]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda14; \
                    class[39]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda15; \
                    class[40]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda1; \
                    class[41]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda2; \
                    class[42]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda3; \
                    class[43]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda4; \
                    class[44]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda5; \
                    class[45]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda6; \
                    class[46]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda7; \
                    class[47]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda8; \
                    class[48]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda9; \
                    class[49]=Ldev/darwinart/simple/DarwinServiceBridge$ActivityClientHandler$$ExternalSyntheticLambda0; \
                    class[50]=Ldev/darwinart/simple/DarwinServiceBridge$ActivityClientHandler; \
                    class[51]=Ldev/darwinart/simple/DarwinServiceBridge$ActivityManagerHandler; \
                    class[52]=Ldev/darwinart/simple/DarwinServiceBridge$ActivityRecord; \
                    class[53]=Ldev/darwinart/simple/DarwinServiceBridge$ActivityTaskHandler$$ExternalSyntheticLambda0; \
                    class[54]=Ldev/darwinart/simple/DarwinServiceBridge$ActivityTaskHandler; \
                    class[55]=Ldev/darwinart/simple/DarwinServiceBridge$AudioServiceBinder; \
                    class[56]=Ldev/darwinart/simple/DarwinServiceBridge$DevicePolicyServiceBinder; \
                    class[57]=Ldev/darwinart/simple/DarwinServiceBridge$DisplayHandler; \
                    class[58]=Ldev/darwinart/simple/DarwinServiceBridge$HostSurfaceState; \
                    class[59]=Ldev/darwinart/simple/DarwinServiceBridge$IntentSenderHandler$$ExternalSyntheticLambda0; \
                    class[60]=Ldev/darwinart/simple/DarwinServiceBridge$IntentSenderHandler; \
                    class[61]=Ldev/darwinart/simple/DarwinServiceBridge$ManagerHandler; \
                    class[62]=Ldev/darwinart/simple/DarwinServiceBridge$MediaSessionInterfaceHandler; \
                    class[63]=Ldev/darwinart/simple/DarwinServiceBridge$UserManagerHandler; \
                    class[64]=Ldev/darwinart/simple/DarwinServiceBridge$WindowManagerHandler$$ExternalSyntheticLambda0; \
                    class[65]=Ldev/darwinart/simple/DarwinServiceBridge$WindowManagerHandler; \
                    class[66]=Ldev/darwinart/simple/DarwinServiceBridge; \
                    class[67]=Ljavax/microedition/khronos/egl/EGL; \
                    class[68]=Ljavax/microedition/khronos/egl/EGL10; \
                    class[69]=Ljavax/microedition/khronos/egl/DarwinEGL10; \
                    class[70]=Ljavax/microedition/khronos/egl/EGLConfig; \
                    class[71]=Ljavax/microedition/khronos/egl/EGLContext; \
                    class[72]=Ljavax/microedition/khronos/egl/EGLDisplay; \
                    class[73]=Ljavax/microedition/khronos/egl/EGLSurface;";
    verify_dex_contract(
        &output,
        221,
        3829,
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
            "Ldev/darwinart/runtime/alarm/AlarmManagerEndpoint;",
            "Ldev/darwinart/runtime/shortcut/ShortcutManagerEndpoint;",
            "Ldev/darwinart/runtime/storage/StorageManagerEndpoint;",
            "Ldev/darwinart/runtime/admin/DevicePolicyManagerEndpoint;",
            "Ldev/darwinart/runtime/power/PowerStateProvider;",
            "Ldev/darwinart/runtime/power/DarwinPowerStateProvider;",
            "Ldev/darwinart/runtime/power/PowerManagerEndpoint;",
            "Ldev/darwinart/runtime/power/ThermalServiceEndpoint;",
            "Ldev/darwinart/runtime/appops/AppOpsServiceEndpoint;",
            "Ldev/darwinart/runtime/connectivity/ConnectivityState;",
            "Ldev/darwinart/runtime/connectivity/ConnectivityCallbackRegistry;",
            "Ldev/darwinart/runtime/connectivity/ConnectivitySnapshot;",
            "Ldev/darwinart/runtime/connectivity/ConnectivityProjection;",
            "Ldev/darwinart/runtime/connectivity/ConnectivityPermissionEnforcer;",
            "Ldev/darwinart/runtime/connectivity/InstalledConnectivityPermissionEnforcer;",
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
            "Ldev/darwinart/runtime/pm/InstalledServiceInfo;",
            "Ldev/darwinart/probe/JitInvokeCustom;",
            "Ldev/darwinart/simple/DarwinServiceBridge;",
            "Ldev/darwinart/simple/DarwinServiceBridge$JobSchedulerHandler;",
            "Ldev/darwinart/system/DarwinSystemServer;",
            "Ldev/darwinart/runtime/pm/InstalledApplicationInfo;",
            "Ldev/darwinart/runtime/pm/InstalledManifestMetadata;",
            "Ldev/darwinart/runtime/pm/InstalledResourceValue;",
            "Ldev/darwinart/runtime/pm/InstalledActivityInfo;",
            "Ldev/darwinart/runtime/pm/InstalledPackageInfo;",
            "Ldev/darwinart/runtime/pm/InstalledPackageRecord;",
            "Ldev/darwinart/runtime/pm/PackageRecords;",
            "Ldev/darwinart/runtime/pm/PackageManagerEndpoint;",
            "Ldev/darwinart/runtime/pm/DexLoadReports;",
            "Ljavax/microedition/khronos/egl/DarwinEGL10;",
        ],
    )?;

    super::verify_service_definitions_external(&output)?;
    build_runtime_support_dex(root, &android_platform_jar, &baseline_classes, &class_dir)?;
    println!("build-button-dex: {}", output.trim());
    Ok(())
}
