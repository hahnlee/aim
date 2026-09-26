use super::*;

pub(crate) fn probe_runtime_graphics(root: &Path) -> Result<()> {
    // The real HWUI path needs the package-side AnimationHost bridge used by
    // RenderNode.create(..., AnimationHost). The button flavor is the
    // smallest Android UI DEX that carries that bridge and framework resources
    // while still exercising the normal Activity/DecorView pipeline.
    probe_runtime_dex_flavor(root, false, true, true, false, false, false)
}

pub(crate) fn probe_runtime_graphics_window(root: &Path) -> Result<()> {
    probe_runtime_dex_flavor(root, true, true, true, false, false, false)
}

pub(crate) fn prepare_probe_android_system_root(root: &Path) -> Result<PathBuf> {
    let base = env::temp_dir();
    let mut directory = None;
    for attempt in 0..128_u32 {
        let candidate = base.join(format!(
            "darwin-art-android-system-root.{}.{}",
            std::process::id(),
            attempt
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => {
                directory = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let directory = directory.ok_or("could not allocate Android system root")?;
    let etc = directory.join("etc");
    let fonts = directory.join("fonts");
    fs::create_dir_all(&etc)?;
    fs::create_dir_all(&fonts)?;
    fs::copy(root.join("probes/button/fonts.xml"), etc.join("fonts.xml"))?;
    fs::copy(
        root.join("_aosp/external/skia/resources/fonts/Roboto-Regular.ttf"),
        fonts.join("Roboto-Regular.ttf"),
    )?;
    fs::set_permissions(etc.join("fonts.xml"), fs::Permissions::from_mode(0o400))?;
    fs::set_permissions(
        fonts.join("Roboto-Regular.ttf"),
        fs::Permissions::from_mode(0o400),
    )?;
    fs::set_permissions(&etc, fs::Permissions::from_mode(0o500))?;
    fs::set_permissions(&fonts, fs::Permissions::from_mode(0o500))?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))?;
    Ok(directory)
}

pub(crate) fn probe_runtime_button(root: &Path, show_window: bool) -> Result<()> {
    probe_runtime_dex_flavor(root, show_window, true, true, false, false, false)
}

pub(crate) fn probe_runtime_apk_app(root: &Path, show_window: bool) -> Result<()> {
    probe_runtime_apk_variant(root, show_window, false)
}

pub(crate) fn probe_runtime_apk_jni_app(root: &Path, show_window: bool) -> Result<()> {
    probe_runtime_apk_variant(root, show_window, true)
}

fn probe_runtime_apk_variant(root: &Path, show_window: bool, apk_jni: bool) -> Result<()> {
    build_shell_gate(root, "fixtures/simple-apk/build.sh")?;
    probe_runtime_dex_flavor_impl(root, show_window, true, false, false, false, true, apk_jni)
}
