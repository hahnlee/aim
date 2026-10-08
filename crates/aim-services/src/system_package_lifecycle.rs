//! Production factory for the package lifecycle owner before forceCurrent.
use crate::package::{bootstrap::Bridge, lifecycle, owner::recovery::Report,
    scan_snapshot::query_state::Context, settings::Settings, system_config::Properties};
use aim_binder_host::{local::Strong, parcel::{Exception, Parcel, EX_ILLEGAL_STATE}};
use aim_service_aidl::dev_aim_server_ipackagelifecycleleaf as leaf;
use std::sync::Arc;

/// This value captures authoritative restored input before scan/version writes.
/// Keeping it separate prevents an already-forceCurrent Settings object from
/// silently defining first-boot/upgrade status as the current boot.
pub struct Restored {
    settings: Settings,
    read_succeeded: bool,
}
impl Restored {
    pub fn capture(settings: &Settings, recovery: &Report) -> Self {
        Self { settings: settings.clone(), read_succeeded: !recovery.first_boot }
    }
    pub fn settings(&self) -> &Settings { &self.settings }
    pub fn read_succeeded(&self) -> bool { self.read_succeeded }
}

/// The same Arc is installed in production Context and the bootstrap freezer
/// publisher. No independent shadow owner is created by facade/query producers.
pub struct Production {
    pub owner: Arc<lifecycle::Owner>,
}
impl Production {
    pub fn construct(restored: Restored, original: &Arc<Bridge>, storage: Strong,
        live_native_properties: Properties) -> Result<Self, Exception> {
        let reply = storage.transact(u32::from_be_bytes(*b"_NTF"), &Parcel::new(), false)
            .map_err(|status| Exception::new(EX_ILLEGAL_STATE, format!("CE-storage descriptor transport: {status}")))?;
        let mut reader = reply.reader();
        if reader.read_string16().map_err(|status| Exception::new(EX_ILLEGAL_STATE, format!("CE-storage descriptor parcel: {status}")))?.as_deref() != Some(leaf::DESCRIPTOR)
            || reader.remaining() != 0 {
            return Err(Exception::new(EX_ILLEGAL_STATE, "CE-storage owner descriptor differs"));
        }
        // Original PackageBootstrapBridge constructs this Version from original
        // Build/SystemProperties partition fingerprint inputs. No image filename,
        // environment value or native default substitutes for that fingerprint.
        let current = original.current_package_version().map_err(|error| match error {
            crate::package::bootstrap::OwnerError::Owner(error) => error,
            other => Exception::new(EX_ILLEGAL_STATE, format!("original package version owner: {other:?}")),
        })?;
        let fingerprint = current.fingerprint.as_deref().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "original partition fingerprint absent"))?;
        let ce_storage: lifecycle::CeStorage = Box::new(move |user| {
            let mut request = Parcel::new();
            leaf::IsCeStorageUnlocked { user_id: user }.write(&mut request);
            let reply = storage.transact(leaf::IS_CE_STORAGE_UNLOCKED, &request, false)
                .map_err(|status| format!("original CE-storage transport: {status}"))?;
            let mut reader = reply.reader();
            let value = leaf::read_is_ce_storage_unlocked_reply(&mut reader)
                .map_err(|status| format!("original CE-storage parcel: {status}"))?
                .map_err(|error| format!("original CE-storage exception: {error:?}"))?;
            if reader.remaining() != 0 { return Err("original CE-storage reply has trailing data".into()); }
            Ok(value)
        });
        let owner = lifecycle::Owner::from_settings(&restored.settings, restored.read_succeeded,
            fingerprint, live_native_properties, ce_storage)
            .map_err(|message| Exception::new(EX_ILLEGAL_STATE, message))?;
        Ok(Self { owner })
    }
    pub fn attach(&self, mut context: Context) -> Result<Context, Exception> {
        if context.system.lifecycle.is_some() {
            return Err(Exception::new(EX_ILLEGAL_STATE, "production package lifecycle owner already assigned"));
        }
        context.system.lifecycle = Some(self.owner.clone());
        // Root installs its existing freezer publication hook against this same
        // owner after capture publication; that hook supplies the initial map.
        Ok(context)
    }
}
