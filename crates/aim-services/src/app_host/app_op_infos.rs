//! `AppOpsManager.sAppOpInfos` at `android-16.0.0_r1`
//! (frameworks/base `core/java/android/app/AppOpsManager.java`, the op
//! codes from frameworks/proto_logging
//! `stats/enums/app_shared/app_op_enums.proto`), each op as its
//! `AppOpInfo.Builder` builds it: its switch, name, permission (some only
//! with an aconfig flag of the image), user restriction and the system's
//! bypass of it, default mode, whether only the app ops managers may read
//! it and whether its notes are always collected. `setDisableReset` is
//! not kept: the app host resets no modes.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use super::appops_service::{
    MODE_ALLOWED, MODE_DEFAULT, MODE_ERRORED, MODE_FOREGROUND, MODE_IGNORED,
};

/// `AppOpsManager.OP_NONE`, the code of the deprecated op 96's entry.
pub(super) const OP_NONE: i32 = -1;
/// `AppOpsManager._NUM_OP`.
pub(super) const NUM_OP: i32 = 163;

/// `AppOpsManager.RestrictionBypass`: what an app needs to bypass an op's
/// user restriction, or what it has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Bypass {
    pub system_uid: bool,
    pub privileged: bool,
    pub record_audio_exempt: bool,
}

const SYSTEM_UID: Bypass = Bypass {
    system_uid: true,
    privileged: false,
    record_audio_exempt: false,
};
const PRIVILEGED: Bypass = Bypass {
    system_uid: false,
    privileged: true,
    record_audio_exempt: false,
};
const RECORD_AUDIO_EXEMPT: Bypass = Bypass {
    system_uid: false,
    privileged: false,
    record_audio_exempt: true,
};
/// `RestrictionBypass.UNRESTRICTED`.
pub(super) const UNRESTRICTED: Bypass = Bypass {
    system_uid: false,
    privileged: true,
    record_audio_exempt: true,
};

/// The aconfig flags an op's permission needs.
const BODY_SENSORS: &str = "android.permission.flags.replace_body_sensor_permission_enabled";
const RANGING: &str = "android.permission.flags.ranging_permission_enabled";
const SYSTEM_PREFERENCES: &str =
    "com.android.settingslib.flags.write_system_preference_permission_enabled";
const XR: &str = "android.xr.xr_manifest_entries";

/// `AppOpInfo.defaultMode`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum DefaultMode {
    Mode(i32),
    /// `getSystemAlertWindowDefault`: ignored on a low-RAM device but a
    /// TV, else MODE_DEFAULT.
    SystemAlertWindow,
}

/// `AppOpInfo`.
#[derive(Clone, Copy, Debug)]
pub(super) struct AppOpInfo {
    pub code: i32,
    pub switch_code: i32,
    pub name: &'static str,
    /// The permission, and the flag it needs.
    pub permission: Option<(&'static str, Option<&'static str>)>,
    pub restriction: Option<&'static str>,
    pub allow_system_restriction_bypass: Option<Bypass>,
    pub default_mode: DefaultMode,
    pub restrict_read: bool,
    pub force_collect_notes: bool,
}

/// `AppOpInfo.Builder`: its own switch, at MODE_DEFAULT.
const fn op(code: i32, name: &'static str) -> AppOpInfo {
    AppOpInfo {
        code,
        switch_code: code,
        name,
        permission: None,
        restriction: None,
        allow_system_restriction_bypass: None,
        default_mode: DefaultMode::Mode(MODE_DEFAULT),
        restrict_read: false,
        force_collect_notes: false,
    }
}

impl AppOpInfo {
    const fn switch(self, switch_code: i32) -> Self {
        AppOpInfo {
            switch_code,
            ..self
        }
    }

    const fn permission(self, permission: &'static str) -> Self {
        AppOpInfo {
            permission: Some((permission, None)),
            ..self
        }
    }

    /// `setPermission(flag ? permission : null)`.
    const fn flagged_permission(self, permission: &'static str, flag: &'static str) -> Self {
        AppOpInfo {
            permission: Some((permission, Some(flag))),
            ..self
        }
    }

    const fn restriction(self, restriction: &'static str) -> Self {
        AppOpInfo {
            restriction: Some(restriction),
            ..self
        }
    }

    const fn bypass(self, bypass: Bypass) -> Self {
        AppOpInfo {
            allow_system_restriction_bypass: Some(bypass),
            ..self
        }
    }

    const fn mode(self, mode: i32) -> Self {
        AppOpInfo {
            default_mode: DefaultMode::Mode(mode),
            ..self
        }
    }

    const fn system_alert_window_default(self) -> Self {
        AppOpInfo {
            default_mode: DefaultMode::SystemAlertWindow,
            ..self
        }
    }

    const fn restrict_read(self) -> Self {
        AppOpInfo {
            restrict_read: true,
            ..self
        }
    }

    const fn force_collect_notes(self) -> Self {
        AppOpInfo {
            force_collect_notes: true,
            ..self
        }
    }
}

/// `sAppOpInfos`, by op code.
pub(super) static APP_OP_INFOS: [AppOpInfo; NUM_OP as usize] = [
    op(0, "android:coarse_location")
        .permission("android.permission.ACCESS_COARSE_LOCATION")
        .restriction("no_share_location")
        .bypass(SYSTEM_UID)
        .mode(MODE_ALLOWED),
    op(1, "android:fine_location")
        .permission("android.permission.ACCESS_FINE_LOCATION")
        .restriction("no_share_location")
        .bypass(SYSTEM_UID)
        .mode(MODE_ALLOWED),
    op(2, "android:gps")
        .switch(0)
        .restriction("no_share_location")
        .mode(MODE_ALLOWED),
    op(3, "android:vibrate")
        .switch(3)
        .permission("android.permission.VIBRATE")
        .mode(MODE_ALLOWED),
    op(4, "android:read_contacts")
        .permission("android.permission.READ_CONTACTS")
        .mode(MODE_ALLOWED),
    op(5, "android:write_contacts")
        .permission("android.permission.WRITE_CONTACTS")
        .mode(MODE_ALLOWED),
    op(6, "android:read_call_log")
        .permission("android.permission.READ_CALL_LOG")
        .restriction("no_outgoing_calls")
        .mode(MODE_ALLOWED),
    op(7, "android:write_call_log")
        .permission("android.permission.WRITE_CALL_LOG")
        .restriction("no_outgoing_calls")
        .mode(MODE_ALLOWED),
    op(8, "android:read_calendar")
        .permission("android.permission.READ_CALENDAR")
        .mode(MODE_ALLOWED),
    op(9, "android:write_calendar")
        .permission("android.permission.WRITE_CALENDAR")
        .mode(MODE_ALLOWED),
    op(10, "android:wifi_scan")
        .switch(0)
        .permission("android.permission.ACCESS_WIFI_STATE")
        .restriction("no_share_location")
        .bypass(PRIVILEGED)
        .mode(MODE_ALLOWED),
    op(11, "android:post_notification")
        .permission("android.permission.POST_NOTIFICATIONS")
        .mode(MODE_ALLOWED),
    op(12, "android:neighboring_cells")
        .switch(0)
        .mode(MODE_ALLOWED),
    op(13, "android:call_phone")
        .switch(13)
        .permission("android.permission.CALL_PHONE")
        .mode(MODE_ALLOWED),
    op(14, "android:read_sms")
        .permission("android.permission.READ_SMS")
        .restriction("no_sms")
        .mode(MODE_ALLOWED),
    op(15, "android:write_sms")
        .restriction("no_sms")
        .mode(MODE_IGNORED),
    op(16, "android:receive_sms")
        .permission("android.permission.RECEIVE_SMS")
        .restriction("no_sms")
        .mode(MODE_ALLOWED),
    op(17, "android:receive_emergency_broadcast")
        .switch(16)
        .permission("android.permission.RECEIVE_EMERGENCY_BROADCAST")
        .mode(MODE_ALLOWED),
    op(18, "android:receive_mms")
        .permission("android.permission.RECEIVE_MMS")
        .restriction("no_sms")
        .mode(MODE_ALLOWED),
    op(19, "android:receive_wap_push")
        .permission("android.permission.RECEIVE_WAP_PUSH")
        .mode(MODE_ALLOWED),
    op(20, "android:send_sms")
        .permission("android.permission.SEND_SMS")
        .restriction("no_sms")
        .mode(MODE_ALLOWED),
    op(21, "android:read_icc_sms")
        .switch(14)
        .permission("android.permission.READ_SMS")
        .restriction("no_sms")
        .mode(MODE_ALLOWED),
    op(22, "android:write_icc_sms")
        .switch(15)
        .restriction("no_sms")
        .mode(MODE_ALLOWED),
    op(23, "android:write_settings").permission("android.permission.WRITE_SETTINGS"),
    op(24, "android:system_alert_window")
        .permission("android.permission.SYSTEM_ALERT_WINDOW")
        .restriction("no_create_windows")
        .bypass(PRIVILEGED)
        .system_alert_window_default(),
    op(25, "android:access_notifications").permission("android.permission.ACCESS_NOTIFICATIONS"),
    op(26, "android:camera")
        .permission("android.permission.CAMERA")
        .restriction("no_camera")
        .mode(MODE_ALLOWED),
    op(27, "android:record_audio")
        .permission("android.permission.RECORD_AUDIO")
        .restriction("no_record_audio")
        .bypass(RECORD_AUDIO_EXEMPT)
        .mode(MODE_ALLOWED),
    op(28, "android:play_audio").mode(MODE_ALLOWED),
    op(29, "android:read_clipboard").mode(MODE_ALLOWED),
    op(30, "android:write_clipboard").mode(MODE_ALLOWED),
    op(31, "android:take_media_buttons").mode(MODE_ALLOWED),
    op(32, "android:take_audio_focus").mode(MODE_FOREGROUND),
    op(33, "android:audio_master_volume")
        .switch(33)
        .restriction("no_adjust_volume")
        .mode(MODE_ALLOWED),
    op(34, "android:audio_voice_volume")
        .restriction("no_adjust_volume")
        .mode(MODE_ALLOWED),
    op(35, "android:audio_ring_volume")
        .restriction("no_adjust_volume")
        .mode(MODE_ALLOWED),
    op(36, "android:audio_media_volume")
        .restriction("no_adjust_volume")
        .mode(MODE_ALLOWED),
    op(37, "android:audio_alarm_volume")
        .restriction("no_adjust_volume")
        .mode(MODE_ALLOWED),
    op(38, "android:audio_notification_volume")
        .switch(38)
        .restriction("no_adjust_volume")
        .mode(MODE_ALLOWED),
    op(39, "android:audio_bluetooth_volume")
        .restriction("no_adjust_volume")
        .mode(MODE_ALLOWED),
    op(40, "android:wake_lock")
        .permission("android.permission.WAKE_LOCK")
        .mode(MODE_ALLOWED),
    op(41, "android:monitor_location")
        .switch(0)
        .restriction("no_share_location")
        .mode(MODE_ALLOWED),
    op(42, "android:monitor_location_high_power")
        .switch(0)
        .restriction("no_share_location")
        .mode(MODE_ALLOWED),
    op(43, "android:get_usage_stats").permission("android.permission.PACKAGE_USAGE_STATS"),
    op(44, "android:mute_microphone")
        .restriction("no_unmute_microphone")
        .mode(MODE_ALLOWED),
    op(45, "android:toast_window")
        .restriction("no_create_windows")
        .bypass(PRIVILEGED)
        .mode(MODE_ALLOWED),
    op(46, "android:project_media").mode(MODE_IGNORED),
    op(47, "android:activate_vpn").mode(MODE_IGNORED),
    op(48, "android:write_wallpaper")
        .restriction("no_wallpaper")
        .mode(MODE_ALLOWED),
    op(49, "android:assist_structure").mode(MODE_ALLOWED),
    op(50, "android:assist_screenshot").mode(MODE_ALLOWED),
    op(51, "android:read_phone_state")
        .permission("android.permission.READ_PHONE_STATE")
        .mode(MODE_ALLOWED),
    op(52, "android:add_voicemail")
        .permission("com.android.voicemail.permission.ADD_VOICEMAIL")
        .mode(MODE_ALLOWED),
    op(53, "android:use_sip")
        .permission("android.permission.USE_SIP")
        .mode(MODE_ALLOWED),
    op(54, "android:process_outgoing_calls")
        .switch(54)
        .permission("android.permission.PROCESS_OUTGOING_CALLS")
        .mode(MODE_ALLOWED),
    op(55, "android:use_fingerprint")
        .permission("android.permission.USE_FINGERPRINT")
        .mode(MODE_ALLOWED),
    op(56, "android:body_sensors")
        .permission("android.permission.BODY_SENSORS")
        .mode(MODE_ALLOWED),
    op(57, "android:read_cell_broadcasts")
        .permission("android.permission.READ_CELL_BROADCASTS")
        .mode(MODE_ALLOWED),
    op(58, "android:mock_location").mode(MODE_ERRORED),
    op(59, "android:read_external_storage")
        .permission("android.permission.READ_EXTERNAL_STORAGE")
        .mode(MODE_ALLOWED),
    op(60, "android:write_external_storage")
        .permission("android.permission.WRITE_EXTERNAL_STORAGE")
        .mode(MODE_ALLOWED),
    op(61, "android:turn_screen_on")
        .permission("android.permission.TURN_SCREEN_ON")
        .mode(MODE_DEFAULT),
    op(62, "android:get_accounts")
        .permission("android.permission.GET_ACCOUNTS")
        .mode(MODE_ALLOWED),
    op(63, "android:run_in_background").mode(MODE_ALLOWED),
    op(64, "android:audio_accessibility_volume")
        .restriction("no_adjust_volume")
        .mode(MODE_ALLOWED),
    op(65, "android:read_phone_numbers")
        .permission("android.permission.READ_PHONE_NUMBERS")
        .mode(MODE_ALLOWED),
    op(66, "android:request_install_packages")
        .switch(66)
        .permission("android.permission.REQUEST_INSTALL_PACKAGES"),
    op(67, "android:picture_in_picture")
        .switch(67)
        .mode(MODE_ALLOWED),
    op(68, "android:instant_app_start_foreground")
        .permission("android.permission.INSTANT_APP_FOREGROUND_SERVICE"),
    op(69, "android:answer_phone_calls")
        .switch(69)
        .permission("android.permission.ANSWER_PHONE_CALLS")
        .mode(MODE_ALLOWED),
    op(70, "android:run_any_in_background").mode(MODE_ALLOWED),
    op(71, "android:change_wifi_state")
        .switch(71)
        .permission("android.permission.CHANGE_WIFI_STATE")
        .mode(MODE_ALLOWED),
    op(72, "android:request_delete_packages")
        .permission("android.permission.REQUEST_DELETE_PACKAGES")
        .mode(MODE_ALLOWED),
    op(73, "android:bind_accessibility_service")
        .permission("android.permission.BIND_ACCESSIBILITY_SERVICE")
        .mode(MODE_ALLOWED),
    op(74, "android:accept_handover")
        .switch(74)
        .permission("android.permission.ACCEPT_HANDOVER")
        .mode(MODE_ALLOWED),
    op(75, "android:manage_ipsec_tunnels")
        .permission("android.permission.MANAGE_IPSEC_TUNNELS")
        .mode(MODE_ERRORED),
    op(76, "android:start_foreground")
        .permission("android.permission.FOREGROUND_SERVICE")
        .mode(MODE_ALLOWED),
    op(77, "android:bluetooth_scan")
        .permission("android.permission.BLUETOOTH_SCAN")
        .bypass(PRIVILEGED)
        .mode(MODE_ALLOWED),
    op(78, "android:use_biometric")
        .permission("android.permission.USE_BIOMETRIC")
        .mode(MODE_ALLOWED),
    op(79, "android:activity_recognition")
        .permission("android.permission.ACTIVITY_RECOGNITION")
        .mode(MODE_ALLOWED),
    op(80, "android:sms_financial_transactions")
        .permission("android.permission.SMS_FINANCIAL_TRANSACTIONS")
        .restriction("no_sms"),
    op(81, "android:read_media_audio")
        .permission("android.permission.READ_MEDIA_AUDIO")
        .mode(MODE_ALLOWED),
    op(82, "android:write_media_audio").mode(MODE_ERRORED),
    op(83, "android:read_media_video")
        .permission("android.permission.READ_MEDIA_VIDEO")
        .mode(MODE_ALLOWED),
    op(84, "android:write_media_video").mode(MODE_ERRORED),
    op(85, "android:read_media_images")
        .permission("android.permission.READ_MEDIA_IMAGES")
        .mode(MODE_ALLOWED),
    op(86, "android:write_media_images").mode(MODE_ERRORED),
    op(87, "android:legacy_storage"),
    op(88, "android:access_accessibility").mode(MODE_ALLOWED),
    op(89, "android:read_device_identifiers").mode(MODE_ERRORED),
    op(90, "android:access_media_location")
        .permission("android.permission.ACCESS_MEDIA_LOCATION")
        .mode(MODE_ALLOWED),
    op(91, "android:query_all_packages"),
    op(92, "android:manage_external_storage")
        .permission("android.permission.MANAGE_EXTERNAL_STORAGE"),
    op(93, "android:interact_across_profiles")
        .permission("android.permission.INTERACT_ACROSS_PROFILES"),
    op(94, "android:activate_platform_vpn").mode(MODE_IGNORED),
    op(95, "android:loader_usage_stats").permission("android.permission.LOADER_USAGE_STATS"),
    op(OP_NONE, "").mode(MODE_IGNORED),
    op(97, "android:auto_revoke_permissions_if_unused"),
    op(98, "android:auto_revoke_managed_by_installer").mode(MODE_ALLOWED),
    op(99, "android:no_isolated_storage").mode(MODE_ERRORED),
    op(100, "android:phone_call_microphone").mode(MODE_ALLOWED),
    op(101, "android:phone_call_camera").mode(MODE_ALLOWED),
    op(102, "android:record_audio_hotword").mode(MODE_ALLOWED),
    op(103, "android:manage_ongoing_calls").permission("android.permission.MANAGE_ONGOING_CALLS"),
    op(104, "android:manage_credentials"),
    op(105, "android:use_icc_auth_with_device_identifier")
        .permission("android.permission.USE_ICC_AUTH_WITH_DEVICE_IDENTIFIER"),
    op(106, "android:record_audio_output").mode(MODE_ALLOWED),
    op(107, "android:schedule_exact_alarm").permission("android.permission.SCHEDULE_EXACT_ALARM"),
    op(108, "android:fine_location_source")
        .switch(1)
        .mode(MODE_ALLOWED),
    op(109, "android:coarse_location_source")
        .switch(0)
        .mode(MODE_ALLOWED),
    op(110, "android:manage_media").permission("android.permission.MANAGE_MEDIA"),
    op(111, "android:bluetooth_connect")
        .permission("android.permission.BLUETOOTH_CONNECT")
        .mode(MODE_ALLOWED),
    op(112, "android:uwb_ranging")
        .permission("android.permission.UWB_RANGING")
        .mode(MODE_ALLOWED),
    op(113, "android:activity_recognition_source")
        .switch(79)
        .mode(MODE_ALLOWED),
    op(114, "android:bluetooth_advertise")
        .permission("android.permission.BLUETOOTH_ADVERTISE")
        .mode(MODE_ALLOWED),
    op(115, "android:record_incoming_phone_audio").mode(MODE_ALLOWED),
    op(116, "android:nearby_wifi_devices")
        .permission("android.permission.NEARBY_WIFI_DEVICES")
        .mode(MODE_ALLOWED),
    op(117, "android:establish_vpn_service").mode(MODE_ALLOWED),
    op(118, "android:establish_vpn_manager").mode(MODE_ALLOWED),
    op(119, "android:access_restricted_settings")
        .mode(MODE_DEFAULT)
        .restrict_read(),
    op(120, "android:receive_ambient_trigger_audio")
        .mode(MODE_ALLOWED)
        .force_collect_notes(),
    op(121, "android:receive_explicit_user_interaction_audio").mode(MODE_ALLOWED),
    op(122, "android:run_user_initiated_jobs").mode(MODE_ALLOWED),
    op(123, "android:read_media_visual_user_selected")
        .permission("android.permission.READ_MEDIA_VISUAL_USER_SELECTED")
        .mode(MODE_ALLOWED),
    op(124, "android:system_exempt_from_suspension"),
    op(125, "android:system_exempt_from_dismissible_notifications"),
    op(126, "android:read_write_health_data").mode(MODE_ALLOWED),
    op(127, "android:foreground_service_special_use")
        .permission("android.permission.FOREGROUND_SERVICE_SPECIAL_USE"),
    op(128, "android:system_exempt_from_power_restrictions"),
    op(129, "android:system_exempt_from_hibernation"),
    op(
        130,
        "android:system_exempt_from_activity_bg_start_restriction",
    ),
    op(
        131,
        "android:capture_consentless_bugreport_on_userdebug_build",
    )
    .permission("android.permission.CAPTURE_CONSENTLESS_BUGREPORT_ON_USERDEBUG_BUILD"),
    op(132, "android:deprecated_2").mode(MODE_IGNORED),
    op(133, "android:use_full_screen_intent")
        .permission("android.permission.USE_FULL_SCREEN_INTENT"),
    op(134, "android:camera_sandboxed").mode(MODE_ALLOWED),
    op(135, "android:record_audio_sandboxed").mode(MODE_ALLOWED),
    op(136, "android:receive_sandbox_trigger_audio")
        .permission("android.permission.RECEIVE_SANDBOX_TRIGGER_AUDIO")
        .mode(MODE_DEFAULT),
    op(137, "android:deprecated_3").mode(MODE_IGNORED),
    op(138, "android:create_accessibility_overlay").mode(MODE_ALLOWED),
    op(139, "android:media_routing_control").permission("android.permission.MEDIA_ROUTING_CONTROL"),
    op(140, "android:enable_mobile_data_by_user").mode(MODE_ALLOWED),
    op(141, "android:reserved_for_testing").mode(MODE_ALLOWED),
    op(142, "android:rapid_clear_notifications_by_listener").mode(MODE_ALLOWED),
    op(143, "android:read_system_grammatical_gender")
        .permission("android.permission.READ_SYSTEM_GRAMMATICAL_GENDER"),
    op(144, "android:deprecated_4").mode(MODE_IGNORED),
    op(145, "android:archive_icon_overlay").mode(MODE_ALLOWED),
    op(146, "android:unarchival_support").mode(MODE_ALLOWED),
    op(147, "android:emergency_location")
        .mode(MODE_ALLOWED)
        .permission("android.permission.LOCATION_BYPASS"),
    op(148, "android:receive_sensitive_notifications").mode(MODE_IGNORED),
    op(149, "android:read_heart_rate")
        .flagged_permission("android.permission.health.READ_HEART_RATE", BODY_SENSORS)
        .mode(MODE_ALLOWED),
    op(150, "android:read_skin_temperature")
        .flagged_permission(
            "android.permission.health.READ_SKIN_TEMPERATURE",
            BODY_SENSORS,
        )
        .mode(MODE_ALLOWED),
    op(151, "android:ranging")
        .flagged_permission("android.permission.RANGING", RANGING)
        .mode(MODE_ALLOWED),
    op(152, "android:read_oxygen_saturation")
        .flagged_permission(
            "android.permission.health.READ_OXYGEN_SATURATION",
            BODY_SENSORS,
        )
        .mode(MODE_ALLOWED),
    op(153, "android:write_system_preferences").flagged_permission(
        "android.permission.WRITE_SYSTEM_PREFERENCES",
        SYSTEM_PREFERENCES,
    ),
    op(154, "android:control_audio").mode(MODE_FOREGROUND),
    op(155, "android:control_audio_partial").mode(MODE_FOREGROUND),
    op(156, "android:eye_tracking_coarse")
        .flagged_permission("android.permission.EYE_TRACKING_COARSE", XR),
    op(157, "android:eye_tracking_fine")
        .flagged_permission("android.permission.EYE_TRACKING_FINE", XR),
    op(158, "android:face_tracking").flagged_permission("android.permission.FACE_TRACKING", XR),
    op(159, "android:hand_tracking").flagged_permission("android.permission.HAND_TRACKING", XR),
    op(160, "android:head_tracking").flagged_permission("android.permission.HEAD_TRACKING", XR),
    op(161, "android:scene_understanding_coarse")
        .flagged_permission("android.permission.SCENE_UNDERSTANDING_COARSE", XR),
    op(162, "android:scene_understanding_fine")
        .flagged_permission("android.permission.SCENE_UNDERSTANDING_FINE", XR),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_op_is_at_its_code_with_a_valid_switch() {
        assert_eq!(APP_OP_INFOS.len(), 163);
        for (i, info) in APP_OP_INFOS.iter().enumerate() {
            if i == 96 {
                // OP_DEPRECATED_1's entry.
                assert_eq!(
                    (info.code, info.switch_code, info.name),
                    (OP_NONE, OP_NONE, "")
                );
                continue;
            }
            assert_eq!(info.code, i as i32);
            assert!(info.name.starts_with("android:"), "{}", info.name);
            let switch = &APP_OP_INFOS[info.switch_code as usize];
            assert_eq!(
                switch.switch_code, switch.code,
                "op {i}'s switch {} switches to another",
                switch.code
            );
        }
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = APP_OP_INFOS
            .iter()
            .map(|i| i.name)
            .filter(|n| !n.is_empty())
            .collect();
        names.sort();
        let len = names.len();
        names.dedup();
        assert_eq!(names.len(), len);
    }

    #[test]
    fn entries_as_the_builder_makes_them() {
        let gps = &APP_OP_INFOS[2];
        assert_eq!(
            (gps.switch_code, gps.restriction, gps.default_mode),
            (
                0,
                Some("no_share_location"),
                DefaultMode::Mode(MODE_ALLOWED)
            )
        );
        let saw = &APP_OP_INFOS[24];
        assert_eq!(
            (saw.allow_system_restriction_bypass, saw.default_mode),
            (Some(PRIVILEGED), DefaultMode::SystemAlertWindow)
        );
        assert_eq!(
            APP_OP_INFOS[87].default_mode,
            DefaultMode::Mode(MODE_DEFAULT)
        );
        assert_eq!(
            APP_OP_INFOS[27].allow_system_restriction_bypass,
            Some(RECORD_AUDIO_EXEMPT)
        );
        assert_eq!(
            APP_OP_INFOS[52].permission,
            Some(("com.android.voicemail.permission.ADD_VOICEMAIL", None))
        );
        assert_eq!(
            APP_OP_INFOS[151].permission,
            Some(("android.permission.RANGING", Some(RANGING)))
        );
        assert!(
            APP_OP_INFOS[119].restrict_read
                && APP_OP_INFOS.iter().filter(|i| i.restrict_read).count() == 1
        );
        assert!(
            APP_OP_INFOS[120].force_collect_notes
                && APP_OP_INFOS
                    .iter()
                    .filter(|i| i.force_collect_notes)
                    .count()
                    == 1
        );
    }
}
