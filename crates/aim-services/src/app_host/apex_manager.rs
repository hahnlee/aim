//! ApexManager's inputs for the app host (`ApexManagerImpl` at
//! `android-16.0.0_r1`): the packages apexd's `apexservice` reports
//! (`getAllApexInfos`, `getAllPackages`) and the active ones
//! (`getActiveApexInfos`, `getActivePackages`), as `ApexBootFeed` hands
//! them to the full boot's scan.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::time::{Duration, Instant};

use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Parcel, Reader, Result as ParcelResult};

use super::display::{check_service, parcelable, present};
use crate::package::bootstrap::{ActiveApex, ApexInventory, ApexPackage};

const SERVICE: &str = "apexservice";
const DESCRIPTOR: &str = "android.apex.IApexService";
const GET_STAGED_SESSION_INFO: u32 = 5;
const GET_ACTIVE_PACKAGES: u32 = 7;
const GET_ALL_PACKAGES: u32 = 8;
/// How long `waitForApexService` waits for apexd to register.
const PATIENCE: Duration = Duration::from_secs(30);

/// `android.apex.ApexInfo`, as far as ApexManager reads it.
struct ApexInfo {
    module_name: Option<String>,
    module_path: Option<String>,
    preinstalled_module_path: Option<String>,
    version_code: i64,
    is_factory: bool,
    is_active: bool,
    active_apex_changed: bool,
}

fn read_info(r: &mut Reader<'_>) -> ParcelResult<ApexInfo> {
    present(r)?;
    parcelable(r, |r| {
        let module_name = r.read_string16()?;
        let module_path = r.read_string16()?;
        let preinstalled_module_path = r.read_string16()?;
        let version_code = r.read_i64()?;
        r.read_string16()?; // versionName
        let is_factory = r.read_bool()?;
        let is_active = r.read_bool()?;
        r.read_bool()?; // hasClassPathJars
        Ok(ApexInfo {
            module_name,
            module_path,
            preinstalled_module_path,
            version_code,
            is_factory,
            is_active,
            active_apex_changed: r.read_bool()?,
        })
    })
}

fn packages(service: &Strong, code: u32, name: &str) -> Result<Vec<ApexInfo>, String> {
    let mut data = Parcel::new();
    data.write_interface_token(DESCRIPTOR);
    let reply = service
        .transact(code, &data, false)
        .map_err(|s| format!("{name}: {s}"))?;
    let mut r = reply.reader();
    let read = |r: &mut Reader<'_>| -> ParcelResult<Vec<ApexInfo>> {
        (0..r.read_i32()?.max(0)).map(|_| read_info(r)).collect()
    };
    match r.read_exception() {
        Ok(Ok(())) => read(&mut r).map_err(|s| format!("{name} reply: {s}")),
        Ok(Err(e)) => Err(format!("{name}: {e:?}")),
        Err(s) => Err(format!("{name} reply: {s}")),
    }
}

/// `waitForApexService`: apexd's service, once it registered.
fn service(process: &LocalProcess) -> Result<Strong, String> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Ok(s) = check_service(process, SERVICE) {
            return Ok(s);
        }
        if Instant::now() > deadline {
            return Err(format!("{SERVICE} did not register"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `android.apex.ApexSessionInfo`'s states, as far as a session's state
/// reads them.
#[derive(Debug, Default, PartialEq)]
pub struct SessionInfo {
    pub unknown: bool,
    pub activated: bool,
    pub activation_failed: bool,
    pub success: bool,
    pub reverted: bool,
    pub revert_failed: bool,
}

fn read_session(r: &mut Reader<'_>) -> ParcelResult<SessionInfo> {
    present(r)?;
    parcelable(r, |r| {
        r.read_i32()?; // sessionId
        let unknown = r.read_bool()?;
        r.read_bool()?; // isVerified
        r.read_bool()?; // isStaged
        let activated = r.read_bool()?;
        r.read_bool()?; // isRevertInProgress
        Ok(SessionInfo {
            unknown,
            activated,
            activation_failed: r.read_bool()?,
            success: r.read_bool()?,
            reverted: r.read_bool()?,
            revert_failed: r.read_bool()?,
        })
    })
}

/// `ApexManager.getStagedSessionInfo(sessionId)`: apexd's session, none
/// when it does not know it (`isUnknown`).
pub fn staged_session_info(process: &LocalProcess, id: i32) -> Result<Option<SessionInfo>, String> {
    let service = service(process)?;
    let mut data = Parcel::new();
    data.write_interface_token(DESCRIPTOR);
    data.write_i32(id);
    let reply = service
        .transact(GET_STAGED_SESSION_INFO, &data, false)
        .map_err(|s| format!("getStagedSessionInfo: {s}"))?;
    let mut r = reply.reader();
    match r.read_exception() {
        Ok(Ok(())) => {
            let info =
                read_session(&mut r).map_err(|s| format!("getStagedSessionInfo reply: {s}"))?;
            Ok((!info.unknown).then_some(info))
        }
        Ok(Err(e)) => Err(format!("getStagedSessionInfo: {e:?}")),
        Err(s) => Err(format!("getStagedSessionInfo reply: {s}")),
    }
}

/// The packages and the active ones, as `ApexBootFeed.capture` records
/// them: an active one is mounted at `/apex/<moduleName>`
/// (`ActiveApexInfo(ApexInfo)`).
pub fn inventory(process: &LocalProcess) -> Result<ApexInventory, String> {
    let service = service(process)?;
    let path = |p: Option<String>, what: &str| {
        p.ok_or_else(|| format!("{SERVICE}: an APEX without a {what}"))
    };
    let all = packages(&service, GET_ALL_PACKAGES, "getAllPackages")?
        .into_iter()
        .map(|i| {
            Ok(ApexPackage {
                module_path: path(i.module_path, "module path")?,
                preinstalled_path: path(i.preinstalled_module_path, "preinstalled path")?,
                module_name: i.module_name,
                version_code: i.version_code,
                factory: i.is_factory,
                active: i.is_active,
                active_changed: i.active_apex_changed,
            })
        })
        .collect::<Result<_, String>>()?;
    let active = packages(&service, GET_ACTIVE_PACKAGES, "getActivePackages")?
        .into_iter()
        .map(|i| {
            Ok(ActiveApex {
                mount_path: format!("/apex/{}", i.module_name.as_deref().unwrap_or("null")),
                preinstalled_path: path(i.preinstalled_module_path, "preinstalled path")?,
                module_path: path(i.module_path, "module path")?,
                module_name: i.module_name,
                factory: i.is_factory,
                active_changed: i.active_apex_changed,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(ApexInventory {
        packages: Some(all),
        active,
    })
}
