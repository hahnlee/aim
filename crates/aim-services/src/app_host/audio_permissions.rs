//! AudioService's `AudioServerPermissionProvider` for the app host
//! (`android-16.0.0_r1`): with `audioserver_permissions`, audioserver
//! checks its callers' permissions against the state its
//! `INativePermissionController` is given, so on audioserver's start the
//! app host gives it each app id's packages and, for each permission it
//! monitors (`PermissionEnum`), the uids holding it. Packages are not
//! added and permissions not changed while the app host runs, so nothing
//! updates it later.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::Parcel;

use super::AppHost;

const DESCRIPTOR: &str = "com.android.media.permission.INativePermissionController";
const POPULATE_PACKAGES_FOR_UIDS: u32 = 1;
const POPULATE_PERMISSION_STATE: u32 = 3;

/// `MONITORED_PERMS`, by `PermissionEnum`.
const MONITORED_PERMS: [&str; 16] = [
    "android.permission.RECORD_AUDIO",
    "android.permission.MODIFY_AUDIO_ROUTING",
    "android.permission.MODIFY_AUDIO_SETTINGS",
    "android.permission.MODIFY_PHONE_STATE",
    "android.permission.MODIFY_DEFAULT_AUDIO_EFFECTS",
    "android.permission.WRITE_SECURE_SETTINGS",
    "android.permission.CALL_AUDIO_INTERCEPTION",
    "android.permission.ACCESS_ULTRASOUND",
    "android.permission.CAPTURE_AUDIO_OUTPUT",
    "android.permission.CAPTURE_MEDIA_OUTPUT",
    "android.permission.CAPTURE_AUDIO_HOTWORD",
    "android.permission.CAPTURE_TUNER_AUDIO_INPUT",
    "android.permission.CAPTURE_VOICE_COMMUNICATION_OUTPUT",
    "android.permission.BLUETOOTH_CONNECT",
    "android.permission.BYPASS_CONCURRENT_RECORD_AUDIO_RESTRICTION",
    "android.permission.MODIFY_AUDIO_SETTINGS_PRIVILEGED",
];

/// The running users (`UserManagerInternal.getUserIds`): the app host's
/// one.
const USERS: [i32; 1] = [0];

impl AppHost {
    /// `onServiceStart`: `controller` given every app id's packages
    /// (`resetNativePackageState`), then each monitored permission's
    /// holders (`getUidsHoldingPerm`, as `checkComponentPermission`
    /// answers).
    pub(super) fn populate_audio_permissions(&self, controller: &Strong) -> Result<(), String> {
        let packages = {
            let (state, _) = self.packages().map_err(|e| format!("{e:?}"))?;
            package_mappings(state.packages.values().map(|ps| (ps.app_id, ps.name.clone())))
        };
        let mut data = Parcel::new();
        data.write_interface_token(DESCRIPTOR);
        write_package_states(&mut data, &packages);
        call(controller, POPULATE_PACKAGES_FOR_UIDS, "populatePackagesForUids", &data)?;
        for (perm, name) in MONITORED_PERMS.iter().enumerate() {
            let mut uids = Vec::new();
            for user in USERS {
                for &app_id in packages.keys() {
                    let uid = user * super::PER_USER_RANGE as i32 + app_id;
                    if self.check_component_permission(Some(name), uid, -1, true).map_err(|e| format!("{e:?}"))? {
                        uids.push(uid);
                    }
                }
            }
            uids.sort_unstable();
            let mut data = Parcel::new();
            data.write_interface_token(DESCRIPTOR);
            // `PermissionEnum`, a byte enum, written as an int.
            data.write_i32(perm as i32);
            data.write_i32(uids.len() as i32);
            for uid in uids {
                data.write_i32(uid);
            }
            call(controller, POPULATE_PERMISSION_STATE, "populatePermissionState", &data)?;
        }
        Ok(())
    }
}

/// `generatePackageMappings`: the packages of each app id.
fn package_mappings(packages: impl Iterator<Item = (i32, String)>) -> BTreeMap<i32, Vec<String>> {
    let mut map: BTreeMap<i32, Vec<String>> = BTreeMap::new();
    for (app_id, name) in packages {
        let names = map.entry(app_id).or_default();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    map
}

/// A `List<UidPackageState>` as the Java proxy writes it
/// (`writeTypedList`): each a structured parcelable, its uid and its
/// package names.
fn write_package_states(p: &mut Parcel, packages: &BTreeMap<i32, Vec<String>>) {
    p.write_i32(packages.len() as i32);
    for (&uid, names) in packages {
        p.write_i32(1);
        let start = p.position();
        p.write_i32(0);
        p.write_i32(uid);
        p.write_i32(names.len() as i32);
        for name in names {
            p.write_string16(Some(name));
        }
        p.set_i32_at(start, (p.position() - start) as i32);
    }
}

fn call(controller: &Strong, code: u32, name: &str, data: &Parcel) -> Result<(), String> {
    let reply = controller.transact(code, data, false).map_err(|s| format!("{name}: {s}"))?;
    match reply.reader().read_exception() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("{name}: {e:?}")),
        Err(s) => Err(format!("{name}: {s}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_group_by_app_id() {
        let map = package_mappings([(10_001, "a".into()), (10_002, "b".into()), (10_001, "c".into())].into_iter());
        assert_eq!(map[&10_001], ["a", "c"]);
        let mut p = Parcel::new();
        write_package_states(&mut p, &map);
        let mut r = p.reader();
        assert_eq!(r.read_i32().unwrap(), 2);
        assert_eq!(r.read_i32().unwrap(), 1);
        let size = r.read_i32().unwrap();
        // Its size, uid and count, then two one-character strings.
        assert_eq!(size, 4 + 4 + 4 + 8 * 2);
        assert_eq!(r.read_i32().unwrap(), 10_001);
        assert_eq!(r.read_i32().unwrap(), 2);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("a"));
    }
}
