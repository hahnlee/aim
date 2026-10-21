//! What `PackageManagerShellCommand`'s installation commands
//! (`package::shell_install`) ask of the app host at `android-16.0.0_r1`:
//! the caller's user (`translateUserId`), whether a user exists, boot
//! completion, the package model's view of a package, the device's ABIs
//! (`checkAbiArgument`) and its flags. The installer is the app host's own,
//! so no owner of its sessions can die apart from it. An APEX's
//! uninstall: #1996; a package's installed size (`setParamsSize`, for an
//! install with no `-S`) and the dexopt compiler filter's check: #1939.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Weak};

use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

use super::AppHost;
use crate::package::installer::preapproval::IntentSender;
use crate::package::shell::{Host, OwnerDeath, uninstall_flags};

/// `UserHandle.USER_NULL`, `USER_ALL`.
const USER_NULL: i32 = -10000;
const USER_ALL: i32 = -1;
/// `Flags.sdkDependencyInstaller()`.
const SDK_DEPENDENCY_INSTALLER: &str = "android.content.pm.sdk_dependency_installer";

pub(super) struct ShellHost(pub(super) Weak<AppHost>);

impl ShellHost {
    fn host(&self) -> Result<Arc<AppHost>, Exception> {
        self.0
            .upgrade()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host stopped"))
    }

    fn unimplemented(&self, what: &str) -> Exception {
        match self.0.upgrade() {
            Some(host) => host.unimplemented_named("package", what),
            None => Exception::new(EX_ILLEGAL_STATE, "app host stopped"),
        }
    }
}

impl Host for ShellHost {
    fn translate_user(
        &self,
        _pid: i32,
        uid: u32,
        user: i32,
        all_default: i32,
        operation: &str,
    ) -> Result<i32, Exception> {
        let translated = self.host()?.handle_incoming_user(
            uid as i32,
            user,
            all_default != USER_NULL,
            true,
            Some(operation),
        )?;
        Ok(if translated == USER_ALL {
            all_default
        } else {
            translated
        })
    }

    fn user_exists(&self, user: i32) -> Result<bool, Exception> {
        Ok(super::user_service::serial_number(&*self.host()?, user)?.is_some())
    }

    /// `SystemProperties.getBoolean("sys.boot_completed", false)`.
    fn boot_completed(&self) -> Result<bool, Exception> {
        Ok(matches!(
            self.host()?.property("sys.boot_completed").as_deref(),
            Some("1" | "y" | "yes" | "on" | "true")
        ))
    }

    fn link_package_owner_death(
        &self,
        _on_death: Box<dyn FnOnce() + Send>,
    ) -> Result<OwnerDeath, Exception> {
        Ok(OwnerDeath::new(()))
    }

    fn uninstall_package_flags(
        &self,
        name: &str,
        user: i32,
    ) -> Result<Option<(bool, bool)>, Exception> {
        Ok(uninstall_flags(&self.host()?.packages()?.0, name, user))
    }

    fn uninstall_apex(
        &self,
        _uid: u32,
        _pid: i32,
        _name: &str,
        _version: i64,
        _user: i32,
        _receiver: IntentSender,
        _flags: i32,
    ) -> Result<(), Exception> {
        Err(self.unimplemented("uninstalling an APEX (#1996)"))
    }

    fn install_size(
        &self,
        _uid: u32,
        _pid: i32,
        _file: aim_binder_host::server::RetainedFd,
        _path: &str,
        _abi: Option<&str>,
    ) -> Result<i64, Exception> {
        Err(self.unimplemented("an install's size without -S (setParamsSize, #1939)"))
    }

    /// `checkAbiArgument`: one of `Build.SUPPORTED_ABIS`, or `-`.
    fn validate_install_abi(&self, _uid: u32, _pid: i32, abi: &str) -> Result<(), Exception> {
        if abi.is_empty() {
            return Err(Exception::illegal_argument("Missing ABI argument"));
        }
        let supported = self.host()?.property("ro.product.cpu.abilist");
        if abi == "-"
            || supported
                .as_deref()
                .unwrap_or_default()
                .split(',')
                .any(|a| a == abi)
        {
            return Ok(());
        }
        Err(Exception::illegal_argument(format!(
            "ABI {abi} not supported on this device"
        )))
    }

    fn validate_install_compiler_filter(
        &self,
        _uid: u32,
        _pid: i32,
        _filter: &str,
    ) -> Result<(), Exception> {
        Err(self.unimplemented("an install's dexopt compiler filter (#1939)"))
    }

    fn sdk_dependency_installer_enabled(&self, _uid: u32, _pid: i32) -> Result<bool, Exception> {
        let (state, _) = self.host()?.packages()?;
        Ok(state
            .system
            .flags
            .iter()
            .any(|(name, on)| name == SDK_DEPENDENCY_INSTALLER && *on))
    }
}
