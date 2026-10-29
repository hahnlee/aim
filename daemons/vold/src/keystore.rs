//! Vold's Keystore::earlyBootEnded (android-16.0.0_r1, system/vold/Keystore.cpp).
//! Keystore owns early-boot key retirement and the boot-level key cache,
//! including on devices without metadata encryption or checkpoints.

use aim_framework_aidl::android_security_maintenance_ikeystoremaintenance as maintenance;
use binder::binder_impl::{BorrowedParcel, FLAG_CLEAR_BUF, IBinderInternal, TransactionCode};
use binder::{Interface, Status, StatusCode, declare_binder_interface};

trait IKeystoreMaintenance: Interface {}

fn unused(
    _: &dyn IKeystoreMaintenance,
    _: TransactionCode,
    _: &BorrowedParcel<'_>,
    _: &mut BorrowedParcel<'_>,
) -> Result<(), StatusCode> {
    Err(StatusCode::UNKNOWN_TRANSACTION)
}

declare_binder_interface! {
    IKeystoreMaintenance[maintenance::DESCRIPTOR] {
        native: BnKeystoreMaintenance(unused),
        proxy: BpKeystoreMaintenance,
    }
}

impl IKeystoreMaintenance for BpKeystoreMaintenance {}
impl IKeystoreMaintenance for binder::binder_impl::Binder<BnKeystoreMaintenance> {}

pub fn early_boot_ended() -> binder::Result<()> {
    let service =
        binder::wait_for_interface::<dyn IKeystoreMaintenance>("android.security.maintenance")?;
    let keystore = service.as_binder();
    let mut data = keystore.prepare_transact()?;
    data.mark_sensitive();
    let reply = keystore.submit_transact(maintenance::EARLY_BOOT_ENDED, data, FLAG_CLEAR_BUF)?;
    let status: Status = reply.read()?;
    if status.is_ok() { Ok(()) } else { Err(status) }
}
