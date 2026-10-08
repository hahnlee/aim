//! PackageInstaller.SessionParams/SessionInfo wire order at android-16.0.0_r1.
//! Copyright AOSP, Apache License 2.0. Parcelable blobs retain their wire form.
use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader, Result};
use aim_service_aidl::{
    ReadParcelable, WriteParcelable, read_int_array, read_string_list, write_int_array,
    write_string_list,
};

/// A validated known Parcelable, including its class name. Binder offsets are
/// retained for the caller's capability owner; FD-bearing icons need that owner.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Object {
    pub bytes: Vec<u8>,
    pub objects: Vec<u64>,
}
#[derive(Clone, Copy)]
enum Kind {
    Bitmap,
    Uri,
    DataLoader,
}
/// The actual original presentation owner returns a by-value Bitmap parcel.
pub fn recovered_bitmap(bytes: &[u8]) -> Result<Option<Object>> {
    let mut reader = Reader::new(bytes, &[]);
    let value = object(&mut reader, Kind::Bitmap)?;
    if reader.remaining() != 0 { return Err(BAD_VALUE) }
    Ok(value)
}
fn object(r: &mut Reader<'_>, kind: Kind) -> Result<Option<Object>> {
    let start = r.position();
    let Some(class) = r.read_string16()? else {
        return Ok(None);
    };
    match kind {
        Kind::Uri => {
            if !matches!(
                class.as_str(),
                "android.net.Uri$StringUri"
                    | "android.net.Uri$OpaqueUri"
                    | "android.net.Uri$HierarchicalUri"
            ) {
                return Err(BAD_VALUE);
            }
            crate::package::uri::Uri::read(r, &mut crate::package::intent_filter::Plain)?
                .ok_or(BAD_VALUE)?;
        }
        Kind::DataLoader => {
            if class != "android.content.pm.DataLoaderParamsParcel" {
                return Err(BAD_VALUE);
            }
            let at = r.position();
            let size = r.read_i32()?;
            if size < 4 {
                return Err(BAD_VALUE);
            }
            let end = at.checked_add(size as usize).ok_or(BAD_VALUE)?;
            if end > r.position() + r.remaining() {
                return Err(BAD_VALUE);
            }
            if r.position() < end {
                r.read_i32()?;
            }
            for _ in 0..3 {
                if r.position() < end {
                    r.read_string16()?;
                }
            }
            if r.position() > end {
                return Err(BAD_VALUE);
            }
            r.set_position(end);
        }
        Kind::Bitmap => {
            if class != "android.graphics.Bitmap" {
                return Err(BAD_VALUE);
            }
            bitmap(r, 0)?;
        }
    }
    let (bytes, objects) = r.since(start);
    Ok(Some(Object {
        bytes: bytes.to_vec(),
        objects,
    }))
}
/// Bitmap CREATOR's native payload followed by Java gainmap metadata.
/// Gainmap contents use typed Bitmap; native gainmap info is 17 floats + base type.
fn bitmap(r: &mut Reader<'_>, depth: usize) -> Result<()> {
    if depth > 128 {
        return Err(BAD_VALUE);
    }
    r.skip(12)?;
    bytes(r)?;
    r.skip(24)?;
    match r.read_i32()? {
        0 => bytes(r)?,
        1 => {
            r.read_i32()?;
            if r.read_i32()? != 0 {
                r.read_i32()?;
                r.read_fd()?;
            }
        }
        _ => return Err(BAD_VALUE),
    }
    if r.read_bool()? && r.read_i32()? != 0 {
        if r.read_i32()? != 0 {
            bitmap(r, depth + 1)?;
        }
        r.skip(72)?;
    }
    Ok(())
}
fn bytes(r: &mut Reader<'_>) -> Result<()> {
    let n = r.read_i32()?;
    if n >= 0 {
        r.skip((n as usize).checked_add(3).ok_or(BAD_VALUE)? & !3)?;
    }
    Ok(())
}
fn write_object(p: &mut Parcel, value: &Option<Object>) {
    match value {
        None => p.write_string16(None),
        Some(value) => p.write_raw(&value.bytes, &value.objects),
    }
}
fn map(r: &mut Reader<'_>) -> Result<Vec<(Option<String>, Option<i32>)>> {
    let n = r.read_i32()?;
    if n < 0 {
        return Ok(Vec::new());
    }
    if n as usize > r.remaining() / 8 {
        return Err(BAD_VALUE);
    }
    let mut values = Vec::new();
    for _ in 0..n {
        let key = match r.read_i32()? {
            -1 => None,
            0 => r.read_string16()?,
            _ => return Err(BAD_VALUE),
        };
        let value = match r.read_i32()? {
            -1 => None,
            1 => Some(r.read_i32()?),
            _ => return Err(BAD_VALUE),
        };
        if let Some((_, old)) = values.iter_mut().find(|(name, _)| *name == key) {
            *old = value;
        } else {
            values.push((key, value));
        }
    }
    values.sort_by_key(|(key, _)| key.as_deref().map_or(0, crate::package::info::java_hash));
    Ok(values)
}
fn write_map(p: &mut Parcel, values: &[(Option<String>, Option<i32>)]) {
    p.write_i32(values.len() as i32);
    for (key, value) in values {
        match key {
            None => p.write_i32(-1),
            Some(key) => {
                p.write_i32(0);
                p.write_string16(Some(key));
            }
        }
        match value {
            None => p.write_i32(-1),
            Some(value) => {
                p.write_i32(1);
                p.write_i32(*value);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct SessionParams {
    pub mode: i32,
    pub install_flags: i32,
    pub install_location: i32,
    pub install_reason: i32,
    pub install_scenario: i32,
    pub size_bytes: i64,
    pub app_package_name: Option<String>,
    pub app_icon: Option<Object>,
    pub app_label: Option<String>,
    pub originating_uri: Option<Object>,
    pub originating_uid: i32,
    pub referrer_uri: Option<Object>,
    pub abi_override: Option<String>,
    pub volume_uuid: Option<String>,
    pub permission_states: Vec<(Option<String>, Option<i32>)>,
    pub whitelisted_restricted_permissions: Option<Vec<Option<String>>>,
    pub auto_revoke_permissions_mode: i32,
    pub installer_package_name: Option<String>,
    pub multi_package: bool,
    pub staged: bool,
    pub force_queryable_override: bool,
    pub required_installed_version_code: i64,
    pub data_loader_params: Option<Object>,
    pub rollback_data_policy: i32,
    pub rollback_lifetime_millis: i64,
    pub rollback_impact_level: i32,
    pub require_user_action: i32,
    pub package_source: i32,
    pub application_enabled_setting_persistent: bool,
    pub development_install_flags: i32,
    pub unarchive_id: i32,
    pub dexopt_compiler_filter: Option<String>,
    pub auto_install_dependencies_enabled: bool,
}
impl SessionParams {
    pub fn has_capabilities(&self) -> bool {
        [
            &self.app_icon,
            &self.originating_uri,
            &self.referrer_uri,
            &self.data_loader_params,
        ]
        .iter()
        .any(|value| {
            value
                .as_ref()
                .is_some_and(|value| !value.objects.is_empty())
        })
    }
}
impl ReadParcelable for SessionParams {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            mode: r.read_i32()?,
            install_flags: r.read_i32()?,
            install_location: r.read_i32()?,
            install_reason: r.read_i32()?,
            install_scenario: r.read_i32()?,
            size_bytes: r.read_i64()?,
            app_package_name: r.read_string16()?,
            app_icon: object(r, Kind::Bitmap)?,
            app_label: r.read_string16()?,
            originating_uri: object(r, Kind::Uri)?,
            originating_uid: r.read_i32()?,
            referrer_uri: object(r, Kind::Uri)?,
            abi_override: r.read_string16()?,
            volume_uuid: r.read_string16()?,
            permission_states: map(r)?,
            whitelisted_restricted_permissions: read_string_list(r)?,
            auto_revoke_permissions_mode: r.read_i32()?,
            installer_package_name: r.read_string16()?,
            multi_package: r.read_bool()?,
            staged: r.read_bool()?,
            force_queryable_override: r.read_bool()?,
            required_installed_version_code: r.read_i64()?,
            data_loader_params: object(r, Kind::DataLoader)?,
            rollback_data_policy: r.read_i32()?,
            rollback_lifetime_millis: r.read_i64()?,
            rollback_impact_level: r.read_i32()?,
            require_user_action: r.read_i32()?,
            package_source: r.read_i32()?,
            application_enabled_setting_persistent: r.read_bool()?,
            development_install_flags: r.read_i32()?,
            unarchive_id: r.read_i32()?,
            dexopt_compiler_filter: r.read_string16()?,
            auto_install_dependencies_enabled: r.read_bool()?,
        })
    }
}
impl WriteParcelable for SessionParams {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.mode);
        p.write_i32(self.install_flags);
        p.write_i32(self.install_location);
        p.write_i32(self.install_reason);
        p.write_i32(self.install_scenario);
        p.write_i64(self.size_bytes);
        p.write_string16(self.app_package_name.as_deref());
        write_object(p, &self.app_icon);
        p.write_string16(self.app_label.as_deref());
        write_object(p, &self.originating_uri);
        p.write_i32(self.originating_uid);
        write_object(p, &self.referrer_uri);
        p.write_string16(self.abi_override.as_deref());
        p.write_string16(self.volume_uuid.as_deref());
        write_map(p, &self.permission_states);
        write_string_list(p, self.whitelisted_restricted_permissions.as_deref());
        p.write_i32(self.auto_revoke_permissions_mode);
        p.write_string16(self.installer_package_name.as_deref());
        p.write_bool(self.multi_package);
        p.write_bool(self.staged);
        p.write_bool(self.force_queryable_override);
        p.write_i64(self.required_installed_version_code);
        write_object(p, &self.data_loader_params);
        p.write_i32(self.rollback_data_policy);
        p.write_i64(self.rollback_lifetime_millis);
        p.write_i32(self.rollback_impact_level);
        p.write_i32(self.require_user_action);
        p.write_i32(self.package_source);
        p.write_bool(self.application_enabled_setting_persistent);
        p.write_i32(self.development_install_flags);
        p.write_i32(self.unarchive_id);
        p.write_string16(self.dexopt_compiler_filter.as_deref());
        p.write_bool(self.auto_install_dependencies_enabled);
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct SessionInfo {
    pub session_id: i32,
    pub user_id: i32,
    pub installer_package_name: Option<String>,
    pub installer_attribution_tag: Option<String>,
    pub resolved_base_code_path: Option<String>,
    pub progress: f32,
    pub sealed: bool,
    pub active: bool,
    pub mode: i32,
    pub install_reason: i32,
    pub install_scenario: i32,
    pub size_bytes: i64,
    pub app_package_name: Option<String>,
    pub app_icon: Option<Object>,
    pub app_label: Option<String>,
    pub install_location: i32,
    pub originating_uri: Option<Object>,
    pub originating_uid: i32,
    pub referrer_uri: Option<Object>,
    pub granted_runtime_permissions: Option<Vec<Option<String>>>,
    pub whitelisted_restricted_permissions: Option<Vec<Option<String>>>,
    pub auto_revoke_permissions_mode: i32,
    pub install_flags: i32,
    pub multi_package: bool,
    pub staged: bool,
    pub force_queryable: bool,
    pub parent_session_id: i32,
    pub child_session_ids: Option<Vec<i32>>,
    pub session_applied: bool,
    pub session_ready: bool,
    pub session_failed: bool,
    pub session_error_code: i32,
    pub session_error_message: Option<String>,
    pub committed: bool,
    pub preapproval_requested: bool,
    pub rollback_data_policy: i32,
    pub rollback_lifetime_millis: i64,
    pub rollback_impact_level: i32,
    pub created_millis: i64,
    pub require_user_action: i32,
    pub installer_uid: i32,
    pub package_source: i32,
    pub application_enabled_setting_persistent: bool,
    pub pending_user_action_reason: i32,
    pub auto_installing_dependencies_enabled: bool,
}
impl ReadParcelable for SessionInfo {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            session_id: r.read_i32()?,
            user_id: r.read_i32()?,
            installer_package_name: r.read_string16()?,
            installer_attribution_tag: r.read_string16()?,
            resolved_base_code_path: r.read_string16()?,
            progress: r.read_f32()?,
            sealed: r.read_bool()?,
            active: r.read_bool()?,
            mode: r.read_i32()?,
            install_reason: r.read_i32()?,
            install_scenario: r.read_i32()?,
            size_bytes: r.read_i64()?,
            app_package_name: r.read_string16()?,
            app_icon: object(r, Kind::Bitmap)?,
            app_label: r.read_string16()?,
            install_location: r.read_i32()?,
            originating_uri: object(r, Kind::Uri)?,
            originating_uid: r.read_i32()?,
            referrer_uri: object(r, Kind::Uri)?,
            granted_runtime_permissions: read_string_list(r)?,
            whitelisted_restricted_permissions: read_string_list(r)?,
            auto_revoke_permissions_mode: r.read_i32()?,
            install_flags: r.read_i32()?,
            multi_package: r.read_bool()?,
            staged: r.read_bool()?,
            force_queryable: r.read_bool()?,
            parent_session_id: r.read_i32()?,
            child_session_ids: Some(read_int_array(r)?.unwrap_or_default()),
            session_applied: r.read_bool()?,
            session_ready: r.read_bool()?,
            session_failed: r.read_bool()?,
            session_error_code: r.read_i32()?,
            session_error_message: r.read_string16()?,
            committed: r.read_bool()?,
            preapproval_requested: r.read_bool()?,
            rollback_data_policy: r.read_i32()?,
            rollback_lifetime_millis: r.read_i64()?,
            rollback_impact_level: r.read_i32()?,
            created_millis: r.read_i64()?,
            require_user_action: r.read_i32()?,
            installer_uid: r.read_i32()?,
            package_source: r.read_i32()?,
            application_enabled_setting_persistent: r.read_bool()?,
            pending_user_action_reason: r.read_i32()?,
            auto_installing_dependencies_enabled: r.read_bool()?,
        })
    }
}
impl WriteParcelable for SessionInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.session_id);
        p.write_i32(self.user_id);
        p.write_string16(self.installer_package_name.as_deref());
        p.write_string16(self.installer_attribution_tag.as_deref());
        p.write_string16(self.resolved_base_code_path.as_deref());
        p.write_f32(self.progress);
        p.write_bool(self.sealed);
        p.write_bool(self.active);
        p.write_i32(self.mode);
        p.write_i32(self.install_reason);
        p.write_i32(self.install_scenario);
        p.write_i64(self.size_bytes);
        p.write_string16(self.app_package_name.as_deref());
        write_object(p, &self.app_icon);
        p.write_string16(self.app_label.as_deref());
        p.write_i32(self.install_location);
        write_object(p, &self.originating_uri);
        p.write_i32(self.originating_uid);
        write_object(p, &self.referrer_uri);
        write_string_list(p, self.granted_runtime_permissions.as_deref());
        write_string_list(p, self.whitelisted_restricted_permissions.as_deref());
        p.write_i32(self.auto_revoke_permissions_mode);
        p.write_i32(self.install_flags);
        p.write_bool(self.multi_package);
        p.write_bool(self.staged);
        p.write_bool(self.force_queryable);
        p.write_i32(self.parent_session_id);
        write_int_array(p, self.child_session_ids.as_deref());
        p.write_bool(self.session_applied);
        p.write_bool(self.session_ready);
        p.write_bool(self.session_failed);
        p.write_i32(self.session_error_code);
        p.write_string16(self.session_error_message.as_deref());
        p.write_bool(self.committed);
        p.write_bool(self.preapproval_requested);
        p.write_i32(self.rollback_data_policy);
        p.write_i64(self.rollback_lifetime_millis);
        p.write_i32(self.rollback_impact_level);
        p.write_i64(self.created_millis);
        p.write_i32(self.require_user_action);
        p.write_i32(self.installer_uid);
        p.write_i32(self.package_source);
        p.write_bool(self.application_enabled_setting_persistent);
        p.write_i32(self.pending_user_action_reason);
        p.write_bool(self.auto_installing_dependencies_enabled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permission_map_replaces_duplicates_retains_null_and_collision_order() {
        let mut parcel = Parcel::new();
        write_map(
            &mut parcel,
            &[
                (Some("BB".into()), Some(1)),
                (None, None),
                (Some("Aa".into()), Some(2)),
                (Some("BB".into()), Some(0)),
            ],
        );
        let mut reader = Reader::new(parcel.data(), parcel.objects());
        assert_eq!(
            map(&mut reader).unwrap(),
            vec![
                (None, None),
                (Some("BB".into()), Some(0)),
                (Some("Aa".into()), Some(2))
            ]
        );
        assert_eq!(reader.remaining(), 0);
    }
    #[test]
    fn data_loader_sized_payload_preserves_future_fields_and_refuses_crossing_boundary() {
        let mut parcel = Parcel::new();
        parcel.write_string16(Some("android.content.pm.DataLoaderParamsParcel"));
        let start = parcel.position();
        parcel.write_i32(0);
        parcel.write_i32(1);
        parcel.write_string16(Some("p"));
        parcel.write_string16(Some("c"));
        parcel.write_string16(Some("args"));
        parcel.write_i64(99);
        parcel.set_i32_at(start, (parcel.position() - start) as i32);
        parcel.write_i32(123);
        let mut reader = Reader::new(parcel.data(), parcel.objects());
        let value = object(&mut reader, Kind::DataLoader).unwrap().unwrap();
        assert_eq!(reader.read_i32().unwrap(), 123);
        let mut copy = Parcel::new();
        write_object(&mut copy, &Some(value));
        assert_eq!(copy.data(), &parcel.data()[..parcel.data().len() - 4]);
        parcel.set_i32_at(start, 6);
        let mut reader = Reader::new(parcel.data(), parcel.objects());
        assert_eq!(object(&mut reader, Kind::DataLoader), Err(BAD_VALUE));
    }
    #[test]
    fn bitmap_fd_is_marked_as_a_capability_not_only_payload_bytes() {
        let mut parcel = Parcel::new();
        parcel.write_string16(Some("android.graphics.Bitmap"));
        for value in [0, 4, 2, -1, 1, 1, 4, 160] {
            parcel.write_i32(value);
        }
        parcel.write_i64(3);
        parcel.write_i32(1);
        parcel.write_i32(4);
        parcel.write_i32(1);
        parcel.write_i32(0);
        let file = std::fs::File::open("/dev/null").unwrap();
        parcel.write_file(
            aim_binder_host::server::file_from_fd(std::os::fd::AsFd::as_fd(&file)).unwrap(),
        );
        parcel.write_bool(false);
        let mut reader = Reader::new(parcel.data(), parcel.objects());
        let value = object(&mut reader, Kind::Bitmap).unwrap().unwrap();
        assert_eq!(value.objects.len(), 1);
        assert_eq!(reader.remaining(), 0);
    }
    #[test]
    fn bitmap_inline_payload_is_retained_without_color_conversion() {
        let mut parcel = Parcel::new();
        parcel.write_string16(Some("android.graphics.Bitmap"));
        for value in [0, 4, 2, -1, 1, 1, 4, 160] {
            parcel.write_i32(value);
        }
        parcel.write_i64(3);
        parcel.write_i32(0);
        parcel.write_i32(4);
        parcel.write_i32(0x01020304);
        parcel.write_bool(false);
        let mut reader = Reader::new(parcel.data(), parcel.objects());
        let value = object(&mut reader, Kind::Bitmap).unwrap().unwrap();
        assert_eq!(value.bytes, parcel.data());
        assert_eq!(reader.remaining(), 0);
        let mut truncated =
            Reader::new(&parcel.data()[..parcel.data().len() - 1], parcel.objects());
        assert!(object(&mut truncated, Kind::Bitmap).is_err());
    }
}

#[derive(Clone, Debug)]
pub struct DataLoader {
    pub kind: i32,
    pub package: Option<String>,
    pub class: Option<String>,
    pub arguments: Option<String>,
}
impl DataLoader {
    pub fn from_object(object: &Object) -> Result<Self> {
        let mut reader = Reader::new(&object.bytes, &object.objects);
        if reader.read_string16()?.as_deref() != Some("android.content.pm.DataLoaderParamsParcel") {
            return Err(BAD_VALUE);
        }
        let start = reader.position();
        let size = reader.read_i32()?;
        if size < 4 {
            return Err(BAD_VALUE);
        }
        let end = start.checked_add(size as usize).ok_or(BAD_VALUE)?;
        if end > object.bytes.len() {
            return Err(BAD_VALUE);
        }
        let mut result = Self {
            kind: 0,
            package: Some(String::new()),
            class: Some(String::new()),
            arguments: Some(String::new()),
        };
        if reader.position() < end {
            result.kind = reader.read_i32()?;
        }
        if reader.position() < end {
            result.package = reader.read_string16()?;
        }
        if reader.position() < end {
            result.class = reader.read_string16()?;
        }
        if reader.position() < end {
            result.arguments = reader.read_string16()?;
        }
        if reader.position() > end {
            return Err(BAD_VALUE);
        }
        Ok(result)
    }
    pub fn object(&self) -> Object {
        let mut payload = Parcel::new();
        payload.write_i32(self.kind);
        payload.write_string16(self.package.as_deref());
        payload.write_string16(self.class.as_deref());
        payload.write_string16(self.arguments.as_deref());
        let mut parcel = Parcel::new();
        parcel.write_string16(Some("android.content.pm.DataLoaderParamsParcel"));
        parcel.write_i32((payload.data().len() + 4) as i32);
        parcel.write_raw(payload.data(), &[]);
        Object {
            bytes: parcel.data().to_vec(),
            objects: vec![],
        }
    }
}
