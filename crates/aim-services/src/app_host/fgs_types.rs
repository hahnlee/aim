//! Foreground service types' policies (`ForegroundServiceTypePolicy` at
//! `android-16.0.0_r1`, its `DefaultForegroundServiceTypePolicy`): what
//! each type needs (all of some permissions, any of others), whether a
//! missing one is enforced, and the compat changes that deprecate or
//! disable a type; `checkForegroundServiceTypePolicy` decides a start with
//! a type from them, as `ActiveServices.validateForegroundServiceType`
//! asks it. The permission enforcement flags are at their defaults, which
//! the original keeps until DeviceConfig changes one. Roles, USB devices and accessories, and the custom
//! permissions `ActiveServices` sets (`systemExempted`'s system
//! exemptions, `mediaProjection`'s projection) are not checked (#1552).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::parcel::Exception;

/// `ServiceInfo.FOREGROUND_SERVICE_TYPE_*`.
pub(super) const TYPE_MANIFEST: i32 = -1;
pub(super) const TYPE_NONE: i32 = 0;
pub(super) const TYPE_DATA_SYNC: i32 = 1 << 0;
pub(super) const TYPE_MEDIA_PLAYBACK: i32 = 1 << 1;
pub(super) const TYPE_PHONE_CALL: i32 = 1 << 2;
const TYPE_LOCATION: i32 = 1 << 3;
pub(super) const TYPE_CONNECTED_DEVICE: i32 = 1 << 4;
pub(super) const TYPE_MEDIA_PROJECTION: i32 = 1 << 5;
const TYPE_CAMERA: i32 = 1 << 6;
const TYPE_MICROPHONE: i32 = 1 << 7;
const TYPE_HEALTH: i32 = 1 << 8;
const TYPE_REMOTE_MESSAGING: i32 = 1 << 9;
const TYPE_SYSTEM_EXEMPTED: i32 = 1 << 10;
pub(super) const TYPE_SHORT_SERVICE: i32 = 1 << 11;
const TYPE_FILE_MANAGEMENT: i32 = 1 << 12;
pub(super) const TYPE_MEDIA_PROCESSING: i32 = 1 << 13;
pub(super) const TYPE_SPECIAL_USE: i32 = 1 << 30;

/// The compat changes of the policies.
const FGS_TYPE_NONE_DEPRECATION_CHANGE_ID: i64 = 255_042_465;
const FGS_TYPE_NONE_DISABLED_CHANGE_ID: i64 = 255_038_118;
const FGS_TYPE_DATA_SYNC_DEPRECATION_CHANGE_ID: i64 = 255_039_210;
const FGS_TYPE_DATA_SYNC_DISABLED_CHANGE_ID: i64 = 255_659_651;
const FGS_TYPE_PERMISSION_CHANGE_ID: i64 = 254_662_522;
/// `ForegroundServiceTypePolicyInfo.INVALID_CHANGE_ID`.
const INVALID_CHANGE_ID: i64 = 0;
/// `AppOpsManager.OP_PROJECT_MEDIA`, `OP_ACTIVATE_VPN`.
const OP_PROJECT_MEDIA: i32 = 46;
const OP_ACTIVATE_VPN: i32 = 47;
/// The aconfig flag that replaces BODY_SENSORS with Health Connect's
/// permissions.
const REPLACE_BODY_SENSOR: &str = "android.permission.flags.replace_body_sensor_permission_enabled";

/// `ServiceInfo.foregroundServiceTypeToLabel`.
pub(super) fn label(ty: i32) -> &'static str {
    match ty {
        TYPE_MANIFEST => "manifest",
        TYPE_NONE => "none",
        TYPE_DATA_SYNC => "dataSync",
        TYPE_MEDIA_PLAYBACK => "mediaPlayback",
        TYPE_PHONE_CALL => "phoneCall",
        TYPE_LOCATION => "location",
        TYPE_CONNECTED_DEVICE => "connectedDevice",
        TYPE_MEDIA_PROJECTION => "mediaProjection",
        TYPE_CAMERA => "camera",
        TYPE_MICROPHONE => "microphone",
        TYPE_HEALTH => "health",
        TYPE_REMOTE_MESSAGING => "remoteMessaging",
        TYPE_SYSTEM_EXEMPTED => "systemExempted",
        TYPE_SHORT_SERVICE => "shortService",
        TYPE_FILE_MANAGEMENT => "fileManagement",
        TYPE_MEDIA_PROCESSING => "mediaProcessing",
        TYPE_SPECIAL_USE => "specialUse",
        _ => "unknown",
    }
}

/// A permission a type needs (`ForegroundServiceTypePermission`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Permission {
    /// `RegularPermission`.
    Regular(&'static str),
    /// `AppOpPermission`, by its op and the op's public name.
    AppOp(i32, &'static str),
    /// `RolePermission`.
    Role(&'static str),
    /// `UsbDevicePermission`, `UsbAccessoryPermission`.
    UsbDevice,
    UsbAccessory,
}

impl Permission {
    /// Its name, as `toString` gives it.
    fn name(&self) -> &'static str {
        match self {
            Permission::Regular(n) | Permission::AppOp(_, n) | Permission::Role(n) => n,
            Permission::UsbDevice => "USB Device",
            Permission::UsbAccessory => "USB Accessory",
        }
    }
}

/// The custom permissions `ActiveServices` sets on policies.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Custom {
    /// `SystemExemptedFgsTypePermission`.
    SystemExempted,
    /// `MediaProjectionFgsTypeCustomPermission`.
    MediaProjection,
}

/// `ForegroundServiceTypePolicyInfo`.
#[derive(Debug, PartialEq)]
pub(super) struct Policy {
    pub ty: i32,
    deprecation: i64,
    disabled: i64,
    all_of: Option<&'static [Permission]>,
    any_of: Option<&'static [Permission]>,
    /// `mPermissionEnforcementFlagValue`, at its default.
    enforced: bool,
    pub foreground_only: bool,
    custom: Option<Custom>,
}

impl Policy {
    /// `toPermissionString`.
    pub(super) fn permission_string(&self) -> String {
        let list = |all: bool, ps: &[Permission]| {
            let names: Vec<&str> = ps.iter().map(Permission::name).collect();
            format!("allOf={all} [{}]", names.join(", "))
        };
        let mut s = String::new();
        if let Some(all) = self.all_of {
            s += &format!("all of the permissions {} ", list(true, all));
        }
        if let Some(any) = self.any_of {
            s += &format!("any of the permissions {} ", list(false, any));
        }
        s
    }
}

use Permission::{AppOp, Regular, Role, UsbAccessory, UsbDevice};

const fn policy(
    ty: i32,
    changes: (i64, i64),
    all_of: Option<&'static [Permission]>,
    any_of: Option<&'static [Permission]>,
    enforced: bool,
    foreground_only: bool,
) -> Policy {
    Policy {
        ty,
        deprecation: changes.0,
        disabled: changes.1,
        all_of,
        any_of,
        enforced,
        foreground_only,
        custom: None,
    }
}

const NO_CHANGES: (i64, i64) = (INVALID_CHANGE_ID, INVALID_CHANGE_ID);
const TYPE_NONE_CHANGES: (i64, i64) = (
    FGS_TYPE_NONE_DEPRECATION_CHANGE_ID,
    FGS_TYPE_NONE_DISABLED_CHANGE_ID,
);

const HEALTH_ANY_OF: &[Permission] = &[
    Regular("android.permission.ACTIVITY_RECOGNITION"),
    Regular("android.permission.HIGH_SAMPLING_RATE_SENSORS"),
    Regular("android.permission.BODY_SENSORS"),
];
const HEALTH_CONNECT_ANY_OF: &[Permission] = &[
    Regular("android.permission.ACTIVITY_RECOGNITION"),
    Regular("android.permission.HIGH_SAMPLING_RATE_SENSORS"),
    Regular("android.permission.health.READ_HEART_RATE"),
    Regular("android.permission.health.READ_SKIN_TEMPERATURE"),
    Regular("android.permission.health.READ_OXYGEN_SATURATION"),
];

/// The default policy's table (`mForegroundServiceTypePolicies`), the
/// health type's any-of permissions as `getAllowedHealthPermissions` gives
/// them without the body sensor flag.
static POLICIES: &[Policy] = &[
    policy(TYPE_MANIFEST, TYPE_NONE_CHANGES, None, None, false, false),
    policy(TYPE_NONE, TYPE_NONE_CHANGES, None, None, false, false),
    policy(
        TYPE_DATA_SYNC,
        (
            FGS_TYPE_DATA_SYNC_DEPRECATION_CHANGE_ID,
            FGS_TYPE_DATA_SYNC_DISABLED_CHANGE_ID,
        ),
        Some(&[Regular("android.permission.FOREGROUND_SERVICE_DATA_SYNC")]),
        None,
        true,
        false,
    ),
    policy(
        TYPE_MEDIA_PLAYBACK,
        NO_CHANGES,
        Some(&[Regular(
            "android.permission.FOREGROUND_SERVICE_MEDIA_PLAYBACK",
        )]),
        None,
        true,
        false,
    ),
    policy(
        TYPE_PHONE_CALL,
        NO_CHANGES,
        Some(&[Regular("android.permission.FOREGROUND_SERVICE_PHONE_CALL")]),
        Some(&[
            Regular("android.permission.MANAGE_OWN_CALLS"),
            Role("android.app.role.DIALER"),
        ]),
        true,
        false,
    ),
    policy(
        TYPE_LOCATION,
        NO_CHANGES,
        Some(&[Regular("android.permission.FOREGROUND_SERVICE_LOCATION")]),
        Some(&[
            Regular("android.permission.ACCESS_COARSE_LOCATION"),
            Regular("android.permission.ACCESS_FINE_LOCATION"),
        ]),
        true,
        true,
    ),
    policy(
        TYPE_CONNECTED_DEVICE,
        NO_CHANGES,
        Some(&[Regular(
            "android.permission.FOREGROUND_SERVICE_CONNECTED_DEVICE",
        )]),
        Some(&[
            Regular("android.permission.BLUETOOTH_ADVERTISE"),
            Regular("android.permission.BLUETOOTH_CONNECT"),
            Regular("android.permission.BLUETOOTH_SCAN"),
            Regular("android.permission.CHANGE_NETWORK_STATE"),
            Regular("android.permission.CHANGE_WIFI_STATE"),
            Regular("android.permission.CHANGE_WIFI_MULTICAST_STATE"),
            Regular("android.permission.NFC"),
            Regular("android.permission.TRANSMIT_IR"),
            Regular("android.permission.UWB_RANGING"),
            Regular("android.permission.RANGING"),
            UsbDevice,
            UsbAccessory,
        ]),
        true,
        false,
    ),
    Policy {
        custom: Some(Custom::MediaProjection),
        ..policy(
            TYPE_MEDIA_PROJECTION,
            NO_CHANGES,
            Some(&[Regular(
                "android.permission.FOREGROUND_SERVICE_MEDIA_PROJECTION",
            )]),
            Some(&[
                Regular("android.permission.CAPTURE_VIDEO_OUTPUT"),
                AppOp(OP_PROJECT_MEDIA, "android:project_media"),
            ]),
            true,
            false,
        )
    },
    policy(
        TYPE_CAMERA,
        NO_CHANGES,
        Some(&[Regular("android.permission.FOREGROUND_SERVICE_CAMERA")]),
        Some(&[
            Regular("android.permission.CAMERA"),
            Regular("android.permission.SYSTEM_CAMERA"),
        ]),
        true,
        true,
    ),
    policy(
        TYPE_MICROPHONE,
        NO_CHANGES,
        Some(&[Regular("android.permission.FOREGROUND_SERVICE_MICROPHONE")]),
        Some(&[
            Regular("android.permission.CAPTURE_AUDIO_HOTWORD"),
            Regular("android.permission.CAPTURE_AUDIO_OUTPUT"),
            Regular("android.permission.CAPTURE_MEDIA_OUTPUT"),
            Regular("android.permission.CAPTURE_TUNER_AUDIO_INPUT"),
            Regular("android.permission.CAPTURE_VOICE_COMMUNICATION_OUTPUT"),
            Regular("android.permission.RECORD_AUDIO"),
        ]),
        true,
        true,
    ),
    policy(
        TYPE_HEALTH,
        NO_CHANGES,
        Some(&[Regular("android.permission.FOREGROUND_SERVICE_HEALTH")]),
        Some(HEALTH_ANY_OF),
        true,
        false,
    ),
    policy(
        TYPE_REMOTE_MESSAGING,
        NO_CHANGES,
        Some(&[Regular(
            "android.permission.FOREGROUND_SERVICE_REMOTE_MESSAGING",
        )]),
        None,
        true,
        false,
    ),
    Policy {
        custom: Some(Custom::SystemExempted),
        ..policy(
            TYPE_SYSTEM_EXEMPTED,
            NO_CHANGES,
            Some(&[Regular(
                "android.permission.FOREGROUND_SERVICE_SYSTEM_EXEMPTED",
            )]),
            Some(&[
                Regular("android.permission.SCHEDULE_EXACT_ALARM"),
                Regular("android.permission.USE_EXACT_ALARM"),
                AppOp(OP_ACTIVATE_VPN, "android:activate_vpn"),
            ]),
            true,
            false,
        )
    },
    policy(TYPE_SHORT_SERVICE, NO_CHANGES, None, None, false, false),
    policy(
        TYPE_FILE_MANAGEMENT,
        NO_CHANGES,
        Some(&[Regular(
            "android.permission.FOREGROUND_SERVICE_FILE_MANAGEMENT",
        )]),
        None,
        false,
        false,
    ),
    policy(
        TYPE_MEDIA_PROCESSING,
        NO_CHANGES,
        Some(&[Regular(
            "android.permission.FOREGROUND_SERVICE_MEDIA_PROCESSING",
        )]),
        None,
        true,
        false,
    ),
    policy(
        TYPE_SPECIAL_USE,
        NO_CHANGES,
        Some(&[Regular("android.permission.FOREGROUND_SERVICE_SPECIAL_USE")]),
        None,
        true,
        false,
    ),
];

/// `getForegroundServiceTypePolicyInfo`: the policy of `ty`, else of
/// `default_to`.
pub(super) fn policy_of(ty: i32, default_to: i32) -> Result<&'static Policy, Exception> {
    let find = |t: i32| POLICIES.iter().find(|p| p.ty == t);
    find(ty).or_else(|| find(default_to)).ok_or_else(|| {
        Exception::illegal_argument(format!("Invalid default fgs type {default_to}"))
    })
}

/// `checkForegroundServiceTypePolicy`'s result codes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Check {
    Ok,
    Deprecated,
    Disabled,
    PermissionDeniedPermissive,
    PermissionDeniedEnforced,
}

/// What the check asks of the system, for the caller.
pub(super) trait Checker {
    /// `CompatChanges.isChangeEnabled(change, callerUid)`.
    fn change_enabled(&self, change: i64) -> Result<bool, Exception>;
    /// Whether an aconfig flag of the image is on.
    fn flag(&self, name: &str) -> Result<bool, Exception>;
    /// `RegularPermission.checkPermission`.
    fn regular(&self, permission: &str, allow_while_in_use: bool) -> Result<bool, Exception>;
    /// `AppOpPermission.checkPermission`.
    fn app_op(&self, op: i32, allow_while_in_use: bool) -> Result<bool, Exception>;
    /// What is not implemented (#1552).
    fn unimplemented(&self, what: &str) -> Exception;
}

/// `ForegroundServiceTypePermission.checkPermission` of `p`.
fn granted(p: &Permission, c: &impl Checker, wiu: bool) -> Result<bool, Exception> {
    match *p {
        Regular(name) => c.regular(name, wiu),
        AppOp(op, _) => c.app_op(op, wiu),
        Role(_) => Err(c.unimplemented("a foreground service type's role permission")),
        UsbDevice | UsbAccessory => {
            Err(c.unimplemented("a foreground service type's USB permission"))
        }
    }
}

/// `checkForegroundServiceTypePolicy` of `policy` for a caller allowed
/// while-in-use permissions or not (`wiu`).
pub(super) fn check(policy: &Policy, c: &impl Checker, wiu: bool) -> Result<Check, Exception> {
    if policy.disabled != INVALID_CHANGE_ID && c.change_enabled(policy.disabled)? {
        return Ok(Check::Disabled);
    }
    let mut ok = true;
    if let Some(all) = policy.all_of {
        for p in all {
            if !granted(p, c, wiu)? {
                ok = false;
                break;
            }
        }
    }
    if ok {
        let mut check_custom = true;
        if let Some(any) = policy.any_of {
            let any = if policy.ty == TYPE_HEALTH && c.flag(REPLACE_BODY_SENSOR)? {
                HEALTH_CONNECT_ANY_OF
            } else {
                any
            };
            ok = false;
            for p in any {
                if granted(p, c, wiu)? {
                    ok = true;
                    break;
                }
            }
            check_custom = !ok;
        }
        if check_custom && policy.custom.is_some() {
            return Err(c.unimplemented(&format!(
                "the custom permission of foreground service type {}",
                label(policy.ty)
            )));
        }
    }
    if !ok {
        return Ok(
            if policy.enforced && c.change_enabled(FGS_TYPE_PERMISSION_CHANGE_ID)? {
                Check::PermissionDeniedEnforced
            } else {
                Check::PermissionDeniedPermissive
            },
        );
    }
    if policy.deprecation != INVALID_CHANGE_ID && c.change_enabled(policy.deprecation)? {
        return Ok(Check::Deprecated);
    }
    Ok(Check::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A caller holding `held`, with the compat changes `changes` enabled.
    struct Fake {
        held: Vec<&'static str>,
        changes: Vec<i64>,
    }

    impl Checker for Fake {
        fn change_enabled(&self, change: i64) -> Result<bool, Exception> {
            Ok(self.changes.contains(&change))
        }
        fn flag(&self, _: &str) -> Result<bool, Exception> {
            Ok(false)
        }
        fn regular(&self, permission: &str, _: bool) -> Result<bool, Exception> {
            Ok(self.held.contains(&permission))
        }
        fn app_op(&self, _: i32, _: bool) -> Result<bool, Exception> {
            Ok(false)
        }
        fn unimplemented(&self, what: &str) -> Exception {
            Exception::new(
                aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,
                what.to_string(),
            )
        }
    }

    const U: [i64; 3] = [
        FGS_TYPE_PERMISSION_CHANGE_ID,
        FGS_TYPE_NONE_DEPRECATION_CHANGE_ID,
        FGS_TYPE_NONE_DISABLED_CHANGE_ID,
    ];

    #[test]
    fn a_type_needs_its_permission_enforced_from_u() {
        let special = policy_of(TYPE_SPECIAL_USE, TYPE_NONE).unwrap();
        let held = Fake {
            held: vec!["android.permission.FOREGROUND_SERVICE_SPECIAL_USE"],
            changes: U.to_vec(),
        };
        assert_eq!(check(special, &held, true).unwrap(), Check::Ok);
        let missing = Fake {
            held: vec![],
            changes: U.to_vec(),
        };
        assert_eq!(
            check(special, &missing, true).unwrap(),
            Check::PermissionDeniedEnforced
        );
        // Before U the missing permission is only logged.
        let old = Fake {
            held: vec![],
            changes: vec![],
        };
        assert_eq!(
            check(special, &old, true).unwrap(),
            Check::PermissionDeniedPermissive
        );
        // fileManagement's is never enforced.
        let files = policy_of(TYPE_FILE_MANAGEMENT, TYPE_NONE).unwrap();
        assert_eq!(
            check(files, &missing, true).unwrap(),
            Check::PermissionDeniedPermissive
        );
    }

    #[test]
    fn no_type_is_disabled_from_u_and_deprecated_before() {
        let none = policy_of(TYPE_NONE, TYPE_NONE).unwrap();
        assert_eq!(
            check(
                none,
                &Fake {
                    held: vec![],
                    changes: U.to_vec()
                },
                true
            )
            .unwrap(),
            Check::Disabled
        );
        let deprecated = Fake {
            held: vec![],
            changes: vec![FGS_TYPE_NONE_DEPRECATION_CHANGE_ID],
        };
        assert_eq!(check(none, &deprecated, true).unwrap(), Check::Deprecated);
        assert_eq!(
            check(
                none,
                &Fake {
                    held: vec![],
                    changes: vec![]
                },
                true
            )
            .unwrap(),
            Check::Ok
        );
        // shortService needs nothing.
        let short = policy_of(TYPE_SHORT_SERVICE, TYPE_NONE).unwrap();
        assert_eq!(
            check(
                short,
                &Fake {
                    held: vec![],
                    changes: U.to_vec()
                },
                true
            )
            .unwrap(),
            Check::Ok
        );
    }

    #[test]
    fn any_of_one_permission_suffices_and_custom_ones_are_not_checked() {
        let exempted = policy_of(TYPE_SYSTEM_EXEMPTED, TYPE_NONE).unwrap();
        let alarm = Fake {
            held: vec![
                "android.permission.FOREGROUND_SERVICE_SYSTEM_EXEMPTED",
                "android.permission.USE_EXACT_ALARM",
            ],
            changes: U.to_vec(),
        };
        assert_eq!(check(exempted, &alarm, true).unwrap(), Check::Ok);
        let neither = Fake {
            held: vec!["android.permission.FOREGROUND_SERVICE_SYSTEM_EXEMPTED"],
            changes: U.to_vec(),
        };
        assert!(check(exempted, &neither, true).is_err());
        // Without the type's own permission, none of the others is asked.
        let none = Fake {
            held: vec![],
            changes: U.to_vec(),
        };
        assert_eq!(
            check(exempted, &none, true).unwrap(),
            Check::PermissionDeniedEnforced
        );
    }

    #[test]
    fn an_unknown_type_takes_the_default_and_labels_as_the_original() {
        assert_eq!(
            policy_of(1 << 20, TYPE_SPECIAL_USE).unwrap().ty,
            TYPE_SPECIAL_USE
        );
        assert!(policy_of(1 << 20, TYPE_NONE).unwrap().ty == TYPE_NONE);
        assert!(policy_of(1 << 20, 1 << 21).is_err());
        assert_eq!(
            (label(TYPE_SPECIAL_USE), label(TYPE_MANIFEST), label(3)),
            ("specialUse", "manifest", "unknown")
        );
        assert_eq!(
            policy_of(TYPE_MEDIA_PLAYBACK, TYPE_NONE)
                .unwrap()
                .permission_string(),
            "all of the permissions allOf=true [android.permission.FOREGROUND_SERVICE_MEDIA_PLAYBACK] "
        );
    }
}
