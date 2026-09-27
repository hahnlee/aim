//! `apexd` of the derived image: `apexservice` (IApexService) for the
//! pre-flattened APEXes (ADR 0012, "init": APEXes are pre-flattened into
//! the image, so nothing is mounted, verified or decompressed at boot).
//!
//! guest-init plays the rest of apexd's boot role (the APEX info list,
//! `apexd.status=ready`) and starts this program as the `apexd` service,
//! which reports `apexd.status=activated` and `apex.all.ready` as the
//! original does once its packages are active; system_server's
//! ApexManager then reads the packages from it.
//! The packages are `/apex/apex-info-list.xml`, which guest-init wrote
//! from the flattened APEXes. Staged sessions and installs are not
//! supported (the image is the only source of APEXes); the rollback
//! snapshot calls have no data to act on and succeed.

mod info_list;

use android_apex::aidl::android::apex::{
    ApexInfo::ApexInfo,
    ApexInfoList::ApexInfoList,
    ApexSessionInfo::ApexSessionInfo,
    ApexSessionParams::ApexSessionParams,
    CompressedApexInfoList::CompressedApexInfoList,
    IApexService::{BnApexService, IApexService},
};
use binder::{BinderFeatures, ExceptionCode, Interface, Status};

const SERVICE: &str = "apexservice";
const STATUS: &std::ffi::CStr = c"apexd.status";

unsafe extern "C" {
    fn __system_property_set(name: *const std::ffi::c_char, value: *const std::ffi::c_char) -> i32;
}
const INFO_LIST: &str = "/apex/apex-info-list.xml";

/// The info list's text; each call builds fresh `ApexInfo`s from it (the
/// generated parcelables are not `Clone`).
struct ApexService {
    list: String,
}

fn unsupported<T>(what: &str) -> binder::Result<T> {
    log::warn!("{what}: APEXes are pre-flattened into the image");
    Err(Status::new_exception_str(
        ExceptionCode::UNSUPPORTED_OPERATION,
        Some(format!("{what}: APEXes are pre-flattened into the image")),
    ))
}

impl Interface for ApexService {}

#[allow(non_snake_case)]
impl IApexService for ApexService {
    fn submitStagedSession(
        &self,
        _: &ApexSessionParams,
        _: &mut ApexInfoList,
    ) -> binder::Result<()> {
        unsupported("submitStagedSession")
    }
    fn markStagedSessionReady(&self, _: i32) -> binder::Result<()> {
        unsupported("markStagedSessionReady")
    }
    fn markStagedSessionSuccessful(&self, _: i32) -> binder::Result<()> {
        unsupported("markStagedSessionSuccessful")
    }
    fn getSessions(&self) -> binder::Result<Vec<ApexSessionInfo>> {
        Ok(Vec::new())
    }
    fn getStagedSessionInfo(&self, session_id: i32) -> binder::Result<ApexSessionInfo> {
        Ok(ApexSessionInfo {
            sessionId: session_id,
            isUnknown: true,
            ..Default::default()
        })
    }
    fn getStagedApexInfos(&self, _: &ApexSessionParams) -> binder::Result<Vec<ApexInfo>> {
        Ok(Vec::new())
    }
    fn getActivePackages(&self) -> binder::Result<Vec<ApexInfo>> {
        Ok(info_list::parse(&self.list)
            .into_iter()
            .filter(|p| p.isActive)
            .collect())
    }
    fn getAllPackages(&self) -> binder::Result<Vec<ApexInfo>> {
        Ok(info_list::parse(&self.list))
    }
    fn abortStagedSession(&self, _: i32) -> binder::Result<()> {
        unsupported("abortStagedSession")
    }
    fn revertActiveSessions(&self) -> binder::Result<()> {
        Ok(())
    }
    fn snapshotCeData(&self, _: i32, _: i32, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn restoreCeData(&self, _: i32, _: i32, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn destroyDeSnapshots(&self, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn destroyCeSnapshots(&self, _: i32, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn destroyCeSnapshotsNotSpecified(&self, _: i32, _: &[i32]) -> binder::Result<()> {
        Ok(())
    }
    fn unstagePackages(&self, _: &[String]) -> binder::Result<()> {
        unsupported("unstagePackages")
    }
    fn stagePackages(&self, _: &[String]) -> binder::Result<()> {
        unsupported("stagePackages")
    }
    fn resumeRevertIfNeeded(&self) -> binder::Result<()> {
        Ok(())
    }
    fn recollectPreinstalledData(&self) -> binder::Result<()> {
        Ok(())
    }
    fn markBootCompleted(&self) -> binder::Result<()> {
        Ok(())
    }
    fn calculateSizeForCompressedApex(&self, _: &CompressedApexInfoList) -> binder::Result<i64> {
        // Compressed APEXes are already decompressed in the image.
        Ok(0)
    }
    fn reserveSpaceForCompressedApex(&self, _: &CompressedApexInfoList) -> binder::Result<()> {
        Ok(())
    }
    fn installAndActivatePackage(&self, _: &str, _: bool) -> binder::Result<ApexInfo> {
        unsupported("installAndActivatePackage")
    }
}

fn main() {
    daemon_log::init("apexd");
    let list = match std::fs::read_to_string(INFO_LIST) {
        Ok(text) => text,
        Err(e) => {
            log::error!("{INFO_LIST}: {e}");
            std::process::exit(1);
        }
    };
    log::info!(
        "serving {} APEXes from {INFO_LIST}",
        info_list::parse(&list).len()
    );
    let service = BnApexService::new_binder(ApexService { list }, BinderFeatures::default());
    if let Err(e) = binder::add_service(SERVICE, service.as_binder()) {
        log::error!("cannot register {SERVICE}: {e:?}");
        std::process::exit(1);
    }
    // OnAllPackagesActivated; libvintf reads the vendor APEXes' manifest
    // fragments once apex.all.ready is set.
    for (name, value) in [(STATUS, c"activated"), (c"apex.all.ready", c"true")] {
        // SAFETY: NUL-terminated strings.
        if unsafe { __system_property_set(name.as_ptr(), value.as_ptr()) } != 0 {
            log::warn!("cannot set {}", name.to_string_lossy());
        }
    }
    binder::ProcessState::start_thread_pool();
    binder::ProcessState::join_thread_pool();
}
