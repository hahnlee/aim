//! Daemon-owned application launch input captured from profile administration.
//!
//! The Android system process may request a process identity, but it must not
//! select a Darwin executable or manufacture an inherited environment. The
//! profile daemon combines its live system-runtime command with the exact
//! installed application's previously accepted launch environment.

use crate::ProfileError;
use crate::bound_service_process::{BoundServiceProcessLaunch, BoundServiceProcessRequest};
use crate::runtime_service_protocol::StartRuntimeRequest;
use std::ffi::{OsStr, OsString};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ApplicationLaunchTemplate {
    arguments: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
}

impl ApplicationLaunchTemplate {
    pub(crate) fn capture(
        package: &str,
        arguments: &[OsString],
        environment: &[(OsString, OsString)],
    ) -> Result<Option<Self>, ProfileError> {
        let configured = value(environment, "AIM_APK_APP_PACKAGE");
        let Some(configured) = configured else {
            return Ok(None);
        };
        if configured != package {
            return Err(invalid("application launch package identity mismatch"));
        }
        validate_host_abi(arguments)?;
        if value(environment, "AIM_SYSTEM_SERVER_MODE").is_some() {
            return Err(invalid("application launch template is a system process"));
        }
        Ok(Some(Self {
            arguments: arguments.to_vec(),
            environment: environment.to_vec(),
        }))
    }

    pub(crate) fn bound_service_launch(
        &self,
        request: BoundServiceProcessRequest,
        system: &StartRuntimeRequest,
    ) -> Result<BoundServiceProcessLaunch, ProfileError> {
        validate_host_abi(&system.arguments)?;
        if system.package != "android.system"
            || value(&system.environment, "AIM_SYSTEM_SERVER_MODE") != Some(OsStr::new("1"))
        {
            return Err(invalid("bound service requires a system runtime template"));
        }
        if value(&self.environment, "AIM_APK_APP_PACKAGE") != Some(OsStr::new(&request.package)) {
            return Err(invalid(
                "bound service package has no matching launch template",
            ));
        }
        for key in [
            "AIM_PROFILE_SOCKET",
            "AIM_ANDROID_FILESYSTEM_ROOT",
            "AIM_ANDROID_SYSTEM_ROOT",
            "AIM_ANDROID_SYSTEM_NATIVE_DIR",
            "AIM_APK_APP_SUPPORT_DEX",
            "AIM_BOOT_CLASSPATH",
        ] {
            if value(&self.environment, key) != value(&system.environment, key) {
                return Err(invalid("application and system runtime templates disagree"));
            }
        }

        // The daemon-selected host, runtime and boot inputs come from the
        // currently live system instance. Only the application code-path slot
        // comes from the package launch that profile administration accepted.
        let mut arguments = system.arguments.clone();
        arguments[8] = self.arguments[8].clone();
        let mut environment = self
            .environment
            .iter()
            .filter(|(name, _)| !is_per_launch_or_legacy_service_key(name))
            .cloned()
            .collect::<Vec<_>>();
        environment.push((
            "AIM_APK_PROCESS_NAME".into(),
            request.process_name.clone().into(),
        ));
        environment.push((
            "AIM_APK_ISOLATED_PROCESS".into(),
            if request.isolated { "1" } else { "0" }.into(),
        ));
        environment.push((
            "AIM_PROCESS_START_SEQUENCE".into(),
            request.start_sequence.to_string().into(),
        ));
        environment.push(("AIM_ANDROID_UID".into(), request.uid.to_string().into()));
        let launch = BoundServiceProcessLaunch {
            request,
            arguments,
            environment,
        };
        launch.validate()?;
        Ok(launch)
    }
}

fn validate_host_abi(arguments: &[OsString]) -> Result<(), ProfileError> {
    if arguments.len() != 9
        || arguments[1] != "--window-seconds"
        || arguments[0].is_empty()
        || arguments[8].is_empty()
    {
        return Err(invalid(
            "application launch does not match the runtime host ABI",
        ));
    }
    Ok(())
}

fn value<'a>(environment: &'a [(OsString, OsString)], key: &str) -> Option<&'a OsStr> {
    environment
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_os_str())
}

fn is_per_launch_or_legacy_service_key(name: &OsString) -> bool {
    matches!(
        name.to_str(),
        Some(
            "AIM_RUNTIME_INSTANCE_TOKEN"
                | "AIM_PROFILE_LEASE_FD"
                | "AIM_APK_PROCESS_NAME"
                | "AIM_APK_ISOLATED_PROCESS"
                | "AIM_PROCESS_START_SEQUENCE"
                | "AIM_ANDROID_UID"
                | "AIM_DESKTOP_PRESENTATION"
                | "AIM_APK_SERVICE_COMPONENT"
                | "AIM_SERVICE_CONTROL_FD"
                | "AIM_TEST_POINTER_CLICK"
                | "AIM_TEST_POINTER_HOLD_MS"
        )
    )
}

fn invalid(message: &'static str) -> ProfileError {
    ProfileError::Daemon(format!("invalid application launch template: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_service_protocol::{ReadinessMask, RuntimeKey};

    fn args(last: &str) -> Vec<OsString> {
        [
            "/host",
            "--window-seconds",
            "0",
            "/runtime",
            "/core-oj",
            "/core-libart",
            "/framework",
            "/boot-tail",
            last,
        ]
        .into_iter()
        .map(Into::into)
        .collect()
    }

    fn common(package: &str) -> Vec<(OsString, OsString)> {
        [
            ("AIM_APK_APP_PACKAGE", package),
            ("AIM_PROFILE_SOCKET", "/profile/control.sock"),
            ("AIM_ANDROID_FILESYSTEM_ROOT", "/image"),
            ("AIM_ANDROID_SYSTEM_ROOT", "/image/system"),
            ("AIM_ANDROID_SYSTEM_NATIVE_DIR", "/image/system/lib64"),
            ("AIM_APK_APP_SUPPORT_DEX", "/support.dex"),
            ("AIM_BOOT_CLASSPATH", "/boot/a:/boot/b"),
        ]
        .into_iter()
        .map(|(name, value)| (name.into(), value.into()))
        .collect()
    }

    #[test]
    fn system_command_and_application_environment_have_distinct_owners() {
        let mut app_environment = common("org.example.app");
        app_environment.push(("AIM_TEST_POINTER_CLICK".into(), "1,2".into()));
        app_environment.push(("AIM_DESKTOP_PRESENTATION".into(), "1".into()));
        app_environment.push(("AIM_APK_SERVICE_COMPONENT".into(), "old".into()));
        app_environment.push(("AIM_ANDROID_UID".into(), "123".into()));
        let template = ApplicationLaunchTemplate::capture(
            "org.example.app",
            &args("/apps/example.apk"),
            &app_environment,
        )
        .unwrap()
        .unwrap();
        let mut system_environment = common("android");
        system_environment.push(("AIM_SYSTEM_SERVER_MODE".into(), "1".into()));
        let system = StartRuntimeRequest {
            package: "android.system".into(),
            key: RuntimeKey([1; 32]),
            required: ReadinessMask::BINDER,
            arguments: args("/support.dex"),
            environment: system_environment,
        };
        let launch = template
            .bound_service_launch(
                BoundServiceProcessRequest {
                    package: "org.example.app".into(),
                    process_name: "org.example.app:renderer".into(),
                    uid: 10_000,
                    isolated: false,
                    start_sequence: 7,
                },
                &system,
            )
            .unwrap();
        assert_eq!(launch.arguments[0], "/host");
        assert_eq!(launch.arguments[2], "0");
        assert_eq!(launch.arguments[8], "/apps/example.apk");
        assert!(value(&launch.environment, "AIM_TEST_POINTER_CLICK").is_none());
        assert!(value(&launch.environment, "AIM_DESKTOP_PRESENTATION").is_none());
        assert!(value(&launch.environment, "AIM_APK_SERVICE_COMPONENT").is_none());
        assert_eq!(
            value(&launch.environment, "AIM_APK_APP_PACKAGE"),
            Some(OsStr::new("org.example.app"))
        );
        assert_eq!(
            value(&launch.environment, "AIM_APK_PROCESS_NAME"),
            Some(OsStr::new("org.example.app:renderer"))
        );
        assert_eq!(
            value(&launch.environment, "AIM_APK_ISOLATED_PROCESS"),
            Some(OsStr::new("0"))
        );
        assert_eq!(
            value(&launch.environment, "AIM_PROCESS_START_SEQUENCE"),
            Some(OsStr::new("7"))
        );
        assert_eq!(
            value(&launch.environment, "AIM_ANDROID_UID"),
            Some(OsStr::new("10000"))
        );
    }

    #[test]
    fn mismatched_or_system_application_templates_fail_closed() {
        assert!(
            ApplicationLaunchTemplate::capture("org.example", &args("/app.apk"), &[])
                .unwrap()
                .is_none()
        );
        assert!(
            ApplicationLaunchTemplate::capture(
                "org.example",
                &args("/app.apk"),
                &common("org.other")
            )
            .is_err()
        );
        let mut system = common("org.example");
        system.push(("AIM_SYSTEM_SERVER_MODE".into(), "1".into()));
        assert!(
            ApplicationLaunchTemplate::capture("org.example", &args("/app.apk"), &system).is_err()
        );
    }
}
