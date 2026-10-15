//! Complete unparsed shared-setting instance, distinct from the current name owner.
use crate::package::owner::{app_ids::DetachedSetting, shared_users::SharedUser};
use aim_binder_host::parcel::Parcel;
use aim_service_aidl::{write_byte_array, WriteParcelable};

pub(super) fn captured(
    version: u64,
    group_name: &str,
    group: &SharedUser,
    value: &DetachedSetting,
) -> Result<Vec<u8>, String> {
    let mut signing = Parcel::new();
    super::endpoint::PackageSigningState::retained(version, &value.package, group_name, group)?
        .write_to(&mut signing);
    write(version, value, signing.data())
}

pub(super) fn detached(version: u64, value: &DetachedSetting) -> Result<Vec<u8>, String> {
    use crate::package::sign::SigningDetails;
    let setting = &value.package;
    if setting.shared_user {
        return Err("detached UID slot has a shared setting".into());
    }
    let details = setting
        .signatures
        .as_ref()
        .map(SigningDetails::from_saved)
        .transpose()?;
    let mut signing = Parcel::new();
    signing.write_i64(version as i64);
    signing.write_string16(Some(&setting.name));
    signing.write_i32(setting.app_id);
    signing.write_bool(false);
    signing.write_string16(None);
    signing.write_i32(0);
    signing.write_string16(None);
    super::endpoint::write_signing(&mut signing, details.as_ref());
    super::endpoint::write_signing(&mut signing, None);
    write(version, value, signing.data())
}

fn write(version: u64, value: &DetachedSetting, signing: &[u8]) -> Result<Vec<u8>, String> {
    let setting = &value.package;
    let legacy = value
        .legacy
        .as_ref()
        .ok_or("retained legacy owner is not captured")?;
    let fixed = value
        .install_fixed
        .ok_or("retained fixed owner is not captured")?;
    let runtime = value
        .runtime
        .as_ref()
        .ok_or("retained runtime owner is not captured")?;
    if setting.leaving_shared_user.is_none() {
        return Err("retained leaving-shared owner is not captured".into());
    }
    let mut p = Parcel::new();
    let metadata =
        super::setting_record::write(version, setting, false, Some(legacy), Some(fixed))?;
    write_byte_array(&mut p, Some(&metadata));
    write_byte_array(&mut p, Some(signing));
    let runtime = super::runtime_record::write(version, setting, false, false, runtime)?;
    write_byte_array(&mut p, Some(&runtime));
    let mut transient = Parcel::new();
    transient.write_i64(version as i64);
    transient.write_string16(Some(&setting.name));
    transient.write_i32(setting.app_id);
    transient.write_bool(false);
    transient.write_bool(setting.transient.hidden_until_installed);
    transient.write_bool(setting.transient.updated_system_app);
    transient.write_bool(setting.transient.apk_in_updated_apex);
    transient.write_string16(setting.transient.apex_module_name.as_deref());
    write_byte_array(&mut p, Some(transient.data()));
    p.write_i32(i32::try_from(value.users.len()).map_err(|_| "too many retained users")?);
    for (id, user) in &value.users {
        let bytes = super::user_record::write_capture(version, setting, false, *id, user)?;
        write_byte_array(&mut p, Some(&bytes));
    }
    if p.data().len() > i32::MAX as usize {
        return Err("retained setting exceeds transport range".into());
    }
    Ok(p.data().to_vec())
}
