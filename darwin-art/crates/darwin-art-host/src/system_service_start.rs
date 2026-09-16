//! Profile-owned process startup, not Android framework service policy.
//!
//! Derive identity and the typed launch request from the same captured child
//! configuration. No PID searches, socket-existence readiness or legacy adoption.

use crate::{runtime_identity, system_image, system_service_configuration};
use darwin_art_profile::runtime_service_protocol::{
    ReadinessMask, RuntimeKey, StartRuntimeRequest,
};
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

pub struct StartedSystemService {
    pub pid: u32,
    pub endpoints: darwin_art_profile::runtime_service_endpoints::RuntimeEndpoints,
}

/// Arguments are ARCHIVE STORE HOST --window-seconds 0 RUNTIME CORE_OJ
/// CORE_LIBART FRAMEWORK BOOT_TAIL SUPPORT_DEX. The existing host ABI is checked
/// explicitly until its callers migrate; ART uses the complete boot classpath.
pub fn start(
    arguments: &[OsString],
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<StartedSystemService, Box<dyn Error>> {
    if arguments.len() != 11 {
        return Err(invalid("expected archive, store and the system host command").into());
    }
    let command = &arguments[2..];
    validate_command(command)?;
    let environment = system_service_configuration::capture(environment)?;
    let socket = absolute(required(&environment, "DARWIN_ART_PROFILE_SOCKET")?)?;
    let image =
        system_image::prepare_with_identity(&absolute(&arguments[0])?, &absolute(&arguments[1])?)?;
    let inventory = Inputs::collect(command, &environment, &image.root)?;
    let files: Vec<_> = inventory
        .files
        .iter()
        .map(|(r, p)| (r.as_str(), p.as_path()))
        .collect();
    let directories: Vec<_> = inventory
        .directories
        .iter()
        .map(|(r, p)| (r.as_str(), p.as_path()))
        .collect();
    let identity = runtime_identity::fingerprint_runtime(
        &image,
        Path::new(&command[0]),
        &inventory.native,
        &files,
        &directories,
    )?;
    let request = StartRuntimeRequest {
        package: "android.system".into(),
        key: RuntimeKey(system_service_configuration::key(
            identity,
            command,
            &environment,
        )),
        required: ReadinessMask::BINDER.union(ReadinessMask::COMPOSITOR),
        arguments: command.to_vec(),
        environment,
    };
    // Encoding validates the complete bounded request before any launch IPC.
    request.encode()?;
    let response = darwin_art_profile::start_runtime_service(&socket, &request)?;
    let endpoints = darwin_art_profile::runtime_service_endpoints::RuntimeEndpoints::resolve(
        &request.environment,
        response.token,
    )?
    .ok_or_else(|| invalid("system runtime has no transport endpoints"))?;
    Ok(StartedSystemService {
        pid: response.pid,
        endpoints,
    })
}

fn validate_command(command: &[OsString]) -> io::Result<()> {
    if command.len() != 9 || command[1] != "--window-seconds" || command[2] != "0" {
        return Err(invalid(
            "system service requires the non-expiring host command ABI",
        ));
    }
    for index in [0, 3, 4, 5, 6, 8] {
        absolute(&command[index])?;
    }
    Ok(())
}

struct Inputs {
    native: Vec<PathBuf>,
    files: Vec<(String, PathBuf)>,
    directories: Vec<(String, PathBuf)>,
}

impl Inputs {
    fn collect(
        command: &[OsString],
        env: &[(OsString, OsString)],
        image: &Path,
    ) -> io::Result<Self> {
        if required(env, "DARWIN_ART_SYSTEM_SERVER_MODE")? != "1"
            || required(env, "DARWIN_ART_APK_APP_PACKAGE")? != "android"
        {
            return Err(invalid(
                "system service requires explicit system process configuration",
            ));
        }
        if absolute(required(env, "DARWIN_ART_ANDROID_FILESYSTEM_ROOT")?)? != image
            || absolute(required(env, "DARWIN_ART_ANDROID_SYSTEM_ROOT")?)? != image.join("system")
            || absolute(required(env, "DARWIN_ART_ANDROID_SYSTEM_NATIVE_DIR")?)?
                != image.join("system/lib64")
        {
            return Err(invalid(
                "system process roots do not match the prepared image",
            ));
        }
        if required(env, "DARWIN_ART_APK_APP_SUPPORT_DEX")? != command[8] {
            return Err(invalid(
                "support DEX differs between the command and environment",
            ));
        }
        let mut host_files = required(env, "DARWIN_ART_BOOT_CLASSPATH")?.to_os_string();
        host_files.push(":");
        host_files.push(&command[8]);
        let system_services = image.join("system/framework/services.jar");
        host_files.push(":");
        host_files.push(system_services.as_os_str());
        if required(env, "DARWIN_ART_RUNTIME_HOST_FILES")? != host_files {
            return Err(invalid(
                "host file grants differ from the inventoried system boot inputs",
            ));
        }
        let runtime = absolute(&command[3])?;
        let sidecar = runtime
            .parent()
            .ok_or_else(|| invalid("runtime requires a parent"))?
            .join("libopenjdk-named-jni-owner.dylib");
        let mut result = Self {
            native: vec![runtime, sidecar],
            files: Vec::new(),
            directories: Vec::new(),
        };
        if let Some(angle) = optional(env, "DARWIN_ART_ANGLE_DIRECTORY") {
            let angle = absolute(angle)?;
            result
                .native
                .extend([angle.join("libEGL.dylib"), angle.join("libGLESv2.dylib")]);
        }
        if let Some(vulkan) = optional(env, "DARWIN_ART_MOLTENVK_DYLIB") {
            result.native.push(absolute(vulkan)?);
        }
        // Include the full ordered classpath and every still-supported positional
        // ABI input. Order also participates in the semantic configuration hash.
        result.paths(
            "boot-classpath",
            required(env, "DARWIN_ART_BOOT_CLASSPATH")?,
        )?;
        result.paths("abi-boot-tail", &command[7])?;
        result
            .files
            .push(("system-services".into(), system_services));
        for index in [4, 5, 6, 8] {
            result
                .files
                .push((format!("abi-{index}"), absolute(&command[index])?));
        }
        // These files are explicitly opened outside Mach-O load commands.
        result.files.push((
            "android-unwind".into(),
            absolute(required(env, "DARWIN_ART_ANDROID_UNWIND_PROVIDER")?)?,
        ));
        result.files.push((
            "framework-resources".into(),
            absolute(required(env, "DARWIN_ART_APK_APP_RESOURCE_APK")?)?,
        ));
        let i18n = absolute(required(env, "ANDROID_I18N_ROOT")?)?;
        result
            .files
            .push(("icu-common".into(), i18n.join("etc/icu/icudt76l.dat")));
        let timezone = absolute(required(env, "ANDROID_TZDATA_ROOT")?)?.join("etc/tz/versioned/9");
        for relative in [
            "tz_version",
            "tzdata",
            "telephonylookup.xml",
            "tzlookup.xml",
            "icu/metaZones.res",
            "icu/timezoneTypes.res",
            "icu/windowsZones.res",
            "icu/zoneinfo64.res",
        ] {
            result
                .files
                .push((format!("timezone-{relative}"), timezone.join(relative)));
        }
        for name in [
            "DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT",
            "DARWIN_ART_APK_APP_DATA_DIR",
            "DARWIN_ART_ANDROID_SHARED_STORAGE_ROOT",
            "ANDROID_DATA",
        ] {
            result
                .directories
                .push((name.into(), absolute(required(env, name)?)?));
        }
        Ok(result)
    }

    fn paths(&mut self, role: &str, value: &OsStr) -> io::Result<()> {
        for (index, path) in std::env::split_paths(value).enumerate() {
            self.files
                .push((format!("{role}-{index}"), absolute(path.as_os_str())?));
        }
        Ok(())
    }
}

fn optional<'a>(environment: &'a [(OsString, OsString)], name: &str) -> Option<&'a OsStr> {
    environment
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_os_str())
}

fn required<'a>(environment: &'a [(OsString, OsString)], name: &str) -> io::Result<&'a OsStr> {
    optional(environment, name)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid(&format!("system service requires {name}")))
}

fn absolute(value: &OsStr) -> io::Result<PathBuf> {
    let path = PathBuf::from(value);
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(invalid(
            "system service inputs must be absolute paths without parent traversal",
        ));
    }
    Ok(path)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> Vec<OsString> {
        [
            "/bundle/host",
            "--window-seconds",
            "0",
            "/bundle/runtime.dylib",
            "/boot/oj",
            "/boot/libart",
            "/boot/framework",
            "/boot/a:/boot/b",
            "/data/support.dex",
        ]
        .into_iter()
        .map(Into::into)
        .collect()
    }

    #[test]
    fn command_requires_checked_non_expiring_abi_and_absolute_inputs() {
        let mut command = command();
        validate_command(&command).unwrap();
        command[2] = "30".into();
        assert!(validate_command(&command).is_err());
        command[2] = "0".into();
        command[3] = "/bundle/../runtime".into();
        assert!(validate_command(&command).is_err());
        assert!(validate_command(&[]).is_err());
    }

    #[test]
    fn inventory_rejects_missing_system_identity_and_empty_classpath_elements() {
        assert!(Inputs::collect(&command(), &[], Path::new("/image")).is_err());
        let mut inputs = Inputs {
            native: Vec::new(),
            files: Vec::new(),
            directories: Vec::new(),
        };
        assert!(inputs.paths("boot", OsStr::new("/first::/last")).is_err());
        assert!(inputs.paths("boot", OsStr::new("relative")).is_err());
    }

    #[test]
    fn inventory_covers_explicit_providers_boot_data_and_storage() {
        let mut env: Vec<(OsString, OsString)> = [
            ("DARWIN_ART_SYSTEM_SERVER_MODE", "1"),
            ("DARWIN_ART_APK_APP_PACKAGE", "android"),
            ("DARWIN_ART_ANDROID_FILESYSTEM_ROOT", "/image"),
            ("DARWIN_ART_ANDROID_SYSTEM_ROOT", "/image/system"),
            (
                "DARWIN_ART_ANDROID_SYSTEM_NATIVE_DIR",
                "/image/system/lib64",
            ),
            ("DARWIN_ART_APK_APP_SUPPORT_DEX", "/data/support.dex"),
            (
                "DARWIN_ART_RUNTIME_HOST_FILES",
                "/boot/original:/boot/replacement:/data/support.dex:/image/system/framework/services.jar",
            ),
            (
                "DARWIN_ART_BOOT_CLASSPATH",
                "/boot/original:/boot/replacement",
            ),
            ("DARWIN_ART_ANGLE_DIRECTORY", "/providers/angle"),
            ("DARWIN_ART_MOLTENVK_DYLIB", "/providers/vulkan.dylib"),
            ("DARWIN_ART_ANDROID_UNWIND_PROVIDER", "/providers/unwind.so"),
            (
                "DARWIN_ART_APK_APP_RESOURCE_APK",
                "/image/system/framework/framework-res.apk",
            ),
            ("ANDROID_I18N_ROOT", "/i18n"),
            ("ANDROID_TZDATA_ROOT", "/tzdata"),
            ("ANDROID_DATA", "/icu-data"),
            ("DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT", "/private"),
            ("DARWIN_ART_APK_APP_DATA_DIR", "/data"),
            ("DARWIN_ART_ANDROID_SHARED_STORAGE_ROOT", "/storage"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        let inputs = Inputs::collect(&command(), &env, Path::new("/image")).unwrap();
        assert_eq!(
            inputs.native,
            [
                "/bundle/runtime.dylib",
                "/bundle/libopenjdk-named-jni-owner.dylib",
                "/providers/angle/libEGL.dylib",
                "/providers/angle/libGLESv2.dylib",
                "/providers/vulkan.dylib"
            ]
            .map(PathBuf::from)
        );
        assert_eq!(inputs.files.len(), 20);
        assert!(inputs.files.contains(&(
            "system-services".into(),
            "/image/system/framework/services.jar".into()
        )));
        assert_eq!(
            inputs
                .files
                .iter()
                .filter(|(role, _)| role.starts_with("timezone-"))
                .count(),
            8
        );
        assert!(
            inputs
                .files
                .contains(&("boot-classpath-1".into(), "/boot/replacement".into()))
        );
        assert!(
            inputs
                .files
                .contains(&("android-unwind".into(), "/providers/unwind.so".into()))
        );
        assert_eq!(inputs.directories.len(), 4);
        assert!(Inputs::collect(&command(), &env, Path::new("/other-image")).is_err());
        env.iter_mut()
            .find(|(k, _)| k == "DARWIN_ART_APK_APP_SUPPORT_DEX")
            .unwrap()
            .1 = "/different.dex".into();
        assert!(Inputs::collect(&command(), &env, Path::new("/image")).is_err());
    }
}
