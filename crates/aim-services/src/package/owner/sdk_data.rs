//! SDK data requests to original installd, android-16.0.0_r1 ReconcileSdkDataArgs.
//! Copyright (C) 2022 The Android Open Source Project, Apache License 2.0.
use aim_binder_host::parcel::Parcel;
use aim_service_aidl::{WriteParcelable, write_string_list};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdkData {
    pub uuid: Option<String>,
    pub package_name: Option<String>,
    pub sub_dir_names: Option<Vec<Option<String>>>,
    pub user_id: i32,
    pub app_id: i32,
    pub previous_app_id: i32,
    pub se_info: Option<String>,
    pub flags: i32,
}

impl WriteParcelable for SdkData {
    fn write_to(&self, p: &mut Parcel) {
        let start = p.position();
        p.write_i32(0);
        p.write_string16(self.uuid.as_deref());
        p.write_string16(self.package_name.as_deref());
        write_string_list(p, self.sub_dir_names.as_deref());
        p.write_i32(self.user_id);
        p.write_i32(self.app_id);
        p.write_i32(self.previous_app_id);
        p.write_string16(self.se_info.as_deref());
        p.write_i32(self.flags);
        p.set_i32_at(start, (p.position() - start) as i32);
    }
}
