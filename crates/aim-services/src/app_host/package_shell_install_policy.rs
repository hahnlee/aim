//! `PackageShellInstallPolicy`, ported from the device leaf and AOSP
//! android-16.0.0_r1 (ApkLiteParseUtils, InstallLocationUtils and ART).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use super::{AppHost, reply};
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, Reader, UNKNOWN_TRANSACTION,
};
use aim_binder_host::server::RetainedFd;
use aim_service_aidl::{ReadParcelable, dev_aim_server_ipackageshellinstallpolicy as policy};
use android_image_extract::source::ReadAt;
use std::{os::unix::fs::FileExt, sync::Arc};

struct Descriptor(u32);
impl ReadParcelable for Descriptor {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        if r.read_i32()? != 0 {
            return Err(BAD_VALUE);
        }
        r.read_fd().map(Self)
    }
}

struct Input {
    file: RetainedFd,
    size: u64,
}
impl ReadAt for Input {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_exact_at(&self, bytes: &mut [u8], offset: u64) -> std::io::Result<()> {
        self.file.read_exact_at(bytes, offset)
    }
}

pub struct ShellInstallPolicy(pub Arc<AppHost>);
impl ShellInstallPolicy {
    fn enforce(&self, call: &Call<'_>, uid: i32, pid: i32) -> Result<(), Exception> {
        if call.sender_euid != 1000 || !matches!(uid, 0 | 2000) || pid < 0 {
            return Err(Exception::security("Invalid native shell policy caller"));
        }
        Ok(())
    }

    fn abis(&self, name: &str) -> Vec<String> {
        self.0
            .property(name)
            .filter(|value| !value.is_empty())
            .map(|value| value.split(',').map(str::to_owned).collect())
            .unwrap_or_default()
    }

    fn installed_size(&self, fd: u32, path: &str, abi: Option<&str>) -> Result<i64, Exception> {
        let parse_error = |message| {
            Exception::illegal_argument(format!(
                "Error: Failed to parse APK file: {path}: {message}"
            ))
        };
        let io_error =
            || Exception::illegal_argument(format!("Error: Failed to parse APK file: {path}"));
        let file = self
            .0
            .process
            .file(fd)
            .and_then(|file| aim_binder_host::server::file_fd(&file))
            .ok_or_else(|| parse_error(format!("Failed to parse {path}")))?;
        let size = file
            .metadata()
            .map_err(|_| parse_error(format!("Failed to parse {path}")))?
            .len();
        let source = Input { file, size };
        let apk = aim_apps::apk::Apk::from_source(Box::new(Input {
            file: source.file.try_clone().map_err(|_| io_error())?,
            size,
        }))
        .map_err(|_| parse_error(format!("Failed to parse {path}")))?;
        let lite = crate::package::parse::lite::apk_lite(
            &apk,
            path,
            self.0
                .property("ro.build.version.sdk")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            if self.0.property("ro.build.version.codename").as_deref() == Some("REL") {
                Vec::new()
            } else {
                self.abis("ro.build.version.all_codenames")
            },
            |name| self.0.property(name),
        )
        .map_err(|error| {
            use crate::package::parse::Error;
            let message = match error {
                Error::Parse(message)
                | Error::Status(_, message)
                | Error::OlderSdk(message)
                | Error::Unsupported(message) => message,
            };
            parse_error(message)
        })?;
        let native = if lite.extract_native_libs {
            let libraries = crate::package::scan::abi::ZipNativeLibraries::read(&source)
                .map_err(|_| io_error())?;
            libraries
                .sum_with_override(
                    &source,
                    &self.abis("ro.product.cpu.abilist"),
                    crate::package::scan::abi::SupportedAbis {
                        bit32: &self.abis("ro.product.cpu.abilist32"),
                        bit64: &self.abis("ro.product.cpu.abilist64"),
                    },
                    lite.multi_arch,
                    abi,
                )
                .map_err(|_| io_error())?
        } else {
            0
        };
        size.checked_add(native)
            .and_then(|size| i64::try_from(size).ok())
            .ok_or_else(io_error)
    }

    /// An empty filter asks ReasonMapping for `pm.dexopt.install`.
    /// https://android.googlesource.com/platform/art/+/android-16.0.0_r1/libartservice/service/java/com/android/server/art/ReasonMapping.java
    fn validate_compiler_filter(&self, filter: &str) -> Result<(), Exception> {
        if !filter.is_empty() {
            return validate_filter(filter);
        }
        let default = self
            .0
            .property("pm.dexopt.install")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                Exception::illegal_argument("No compiler filter for reason 'install'")
            })?;
        validate_filter(&default).map_err(|_| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("Got invalid compiler filter '{default}' for reason 'install'"),
            )
        })
    }
}

/// ART `DexoptParams.Builder.build` and `Utils.isValidArtServiceCompilerFilter`,
/// then `DexFile.isValidCompilerFilter` / `CompilerFilter::ParseCompilerFilter`.
/// https://android.googlesource.com/platform/art/+/android-16.0.0_r1/libartservice/service/java/com/android/server/art/model/DexoptParams.java
/// https://android.googlesource.com/platform/art/+/android-16.0.0_r1/libartservice/service/java/com/android/server/art/Utils.java
/// https://android.googlesource.com/platform/art/+/android-16.0.0_r1/libartbase/base/compiler_filter.cc
fn validate_filter(filter: &str) -> Result<(), Exception> {
    if matches!(
        filter,
        "skip"
            | "assume-verified"
            | "verify"
            | "space"
            | "space-profile"
            | "speed"
            | "speed-profile"
            | "everything"
            | "everything-profile"
            | "verify-none"
            | "interpret-only"
            | "verify-profile"
            | "verify-at-runtime"
            | "balanced"
            | "time"
            | "extract"
            | "quicken"
    ) {
        Ok(())
    } else {
        Err(Exception::illegal_argument(format!(
            "Invalid compiler filter '{filter}'"
        )))
    }
}

impl Service for ShellInstallPolicy {
    fn descriptor(&self) -> &str {
        policy::DESCRIPTOR
    }
    fn accepts_fds(&self) -> bool {
        true
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let result = match call.code {
            policy::CALCULATE_INSTALLED_SIZE => {
                let a = policy::CalculateInstalledSize::<Descriptor>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    self.enforce(call, a.calling_uid, a.calling_pid)?;
                    let file = a.file.ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
                    let path = a.path.ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
                    self.installed_size(file.0, &path, a.abi.as_deref())
                        .map(|size| policy::write_calculate_installed_size_reply(&mut p, size))
                })()
            }
            policy::VALIDATE_ABI => {
                let a = policy::ValidateAbi::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.enforce(call, a.calling_uid, a.calling_pid)?;
                    let abi = a
                        .abi
                        .filter(|abi| !abi.is_empty())
                        .ok_or_else(|| Exception::illegal_argument("Missing ABI argument"))?;
                    if abi != "-" && !self.abis("ro.product.cpu.abilist").contains(&abi) {
                        return Err(Exception::illegal_argument(format!(
                            "ABI {abi} not supported on this device"
                        )));
                    }
                    policy::write_validate_abi_reply(&mut p);
                    Ok(())
                })()
            }
            policy::VALIDATE_COMPILER_FILTER => {
                let a =
                    policy::ValidateCompilerFilter::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.enforce(call, a.calling_uid, a.calling_pid)?;
                    let filter = a
                        .filter
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
                    self.validate_compiler_filter(&filter)?;
                    policy::write_validate_compiler_filter_reply(&mut p);
                    Ok(())
                })()
            }
            policy::IS_DEPENDENCY_INSTALLER_ENABLED => {
                let a = policy::IsDependencyInstallerEnabled::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.enforce(call, a.calling_uid, a.calling_pid)
                    .and_then(|()| {
                        let enabled = self
                            .0
                            .framework()?
                            .0
                            .flags
                            .get("android.content.pm.sdk_dependency_installer")
                            .copied()
                            .unwrap_or(false);
                        policy::write_is_dependency_installer_enabled_reply(&mut p, enabled);
                        Ok(())
                    })
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
#[path = "package_shell_install_policy_tests.rs"]
mod tests;
