//! Original PackageImpl parcel output from the scan-owned AndroidPackage.
//! Adapted from AOSP android-16.0.0_r1 PackageImpl and Parsed*Impl,
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::*;
use crate::package::intent_filter::{IntentFilter, ParsedIntentInfo, PatternMatcher};
use crate::package::parse::parcel::{Entry, Writer};

/// Cache ABI plus the collected lineage metadata which Signature's Parcel omits.
pub struct FacadeEntry {
    pub cache: Entry,
    pub past_signing_certificates: Option<crate::package::sign::Lineage>,
}

impl AndroidPackage {
    /// Use collected code signing, never reconciled settings/group signing.
    pub fn to_facade_entry(
        &self,
        collected: &crate::package::sign::SigningDetails,
    ) -> std::result::Result<FacadeEntry, String> {
        if self.signing_details.as_ref() != Some(&collected.parcel_details()?) {
            return Err("package and collected signing metadata differ".into());
        }
        Ok(FacadeEntry {
            cache: self.to_cache_entry()?,
            past_signing_certificates: collected.past_signing_certificates.clone(),
        })
    }

    pub fn to_cache_entry(&self) -> std::result::Result<Entry, String> {
        validate(self)?;
        let mut writer = Writer::new();
        self.write(&mut writer);
        Ok(writer.finish())
    }
    /// Required non-null inputs checked by the original cache constructors.
    pub(in crate::package) fn validate_cache_constructor_fields(
        &self,
    ) -> std::result::Result<(), String> {
        let flags = self
            .feature_flag_state
            .as_ref()
            .ok_or("PackageImpl feature flag state is null")?;
        if flags.iter().any(Option::is_none) {
            return Err("PackageImpl feature flag string is null".into());
        }
        if self.processes.iter().flatten().any(|p| p.name.is_none()) {
            return Err("ParsedProcess name is null".into());
        }

        Ok(())
    }

    fn write(&self, w: &mut Writer) {
        w.field("mFeatureFlagState");
        w.optional_strings(self.feature_flag_state.as_deref());
        for (name, v) in [
            ("supportsSmallScreens", self.supports_small_screens),
            ("supportsNormalScreens", self.supports_normal_screens),
            ("supportsLargeScreens", self.supports_large_screens),
            (
                "supportsExtraLargeScreens",
                self.supports_extra_large_screens,
            ),
            ("resizeable", self.resizeable),
            ("anyDensity", self.any_density),
        ] {
            w.field(name);
            for_boolean(w, v);
        }
        w.field("versionCode");
        w.int(self.version_code);
        w.field("versionCodeMajor");
        w.int(self.version_code_major);
        w.field("baseRevisionCode");
        w.int(self.base_revision_code);
        w.field("versionName");
        w.string(self.version_name.as_deref());
        w.field("compileSdkVersion");
        w.int(self.compile_sdk_version);
        w.field("compileSdkVersionCodeName");
        w.string(self.compile_sdk_version_code_name.as_deref());
        w.field("packageName");
        w.string(Some(&self.package_name));
        w.field("mBaseApkPath");
        w.string(self.base_apk_path.as_deref());
        w.field("restrictedAccountType");
        w.string(self.restricted_account_type.as_deref());
        w.field("requiredAccountType");
        w.string(self.required_account_type.as_deref());
        w.field("mEmergencyInstaller");
        w.string(self.emergency_installer.as_deref());
        w.field("overlayTarget");
        w.string(self.overlay_target.as_deref());
        w.field("overlayTargetOverlayableName");
        w.string(self.overlay_target_overlayable_name.as_deref());
        w.field("overlayCategory");
        w.string(self.overlay_category.as_deref());
        w.field("overlayPriority");
        w.int(self.overlay_priority);
        w.field("overlayables");
        string_map(w, self.overlayables.as_deref());
        w.field("sdkLibraryName");
        w.string(self.sdk_library_name.as_deref());
        w.field("sdkLibVersionMajor");
        w.int(self.sdk_lib_version_major);
        w.field("staticSharedLibraryName");
        w.string(self.static_shared_library_name.as_deref());
        w.field("staticSharedLibVersion");
        w.long(self.static_shared_lib_version);
        for (name, v) in [
            ("libraryNames", &self.library_names),
            ("usesLibraries", &self.uses_libraries),
            ("usesOptionalLibraries", &self.uses_optional_libraries),
            ("usesNativeLibraries", &self.uses_native_libraries),
            (
                "usesOptionalNativeLibraries",
                &self.uses_optional_native_libraries,
            ),
            ("usesStaticLibraries", &self.uses_static_libraries),
        ] {
            w.field(name);
            w.strings(Some(v));
        }
        w.field("usesStaticLibrariesVersions");
        w.longs(self.uses_static_libraries_versions.as_deref());
        w.field("usesStaticLibrariesCertDigests");
        digests(w, self.uses_static_libraries_cert_digests.as_deref());
        w.field("usesSdkLibraries");
        w.strings(Some(&self.uses_sdk_libraries));
        w.field("usesSdkLibrariesVersionsMajor");
        w.longs(self.uses_sdk_libraries_versions_major.as_deref());
        w.field("usesSdkLibrariesCertDigests");
        digests(w, self.uses_sdk_libraries_cert_digests.as_deref());
        w.field("usesSdkLibrariesOptional");
        w.bools(self.uses_sdk_libraries_optional.as_deref());
        w.field("sharedUserId");
        w.string(self.shared_user_id.as_deref());
        w.field("sharedUserLabel");
        w.int(self.shared_user_label);
        w.field("configPreferences");
        typed(w, self.config_preferences.as_deref(), |w, c| {
            w.int(c.req_touch_screen);
            w.int(c.req_keyboard_type);
            w.int(c.req_navigation);
            w.int(c.req_input_features);
            w.int(c.req_gl_es_version);
        });
        w.field("reqFeatures");
        typed(w, self.req_features.as_deref(), |w, f| f.write(w));
        w.field("featureGroups");
        typed(w, self.feature_groups.as_deref(), |w, g| match g {
            // `writeTypedArray`.
            Some(features) => typed(w, Some(features.as_slice()), |w, f| f.write(w)),
            None => w.int(-1),
        });
        w.field("restrictUpdateHash");
        w.bytes(self.restrict_update_hash.as_deref());
        w.field("originalPackages");
        w.optional_strings(self.original_packages.as_deref());
        w.field("adoptPermissions");
        w.strings(Some(&self.adopt_permissions));
        w.field("requestedPermissions");
        w.strings(Some(&self.requested_permissions));
        w.field("usesPermissions");
        w.list("usesPermissions", &self.uses_permissions, |w, p| p.write(w));
        w.field("implicitPermissions");
        w.strings(Some(&self.implicit_permissions));
        w.field("upgradeKeySets");
        w.strings(Some(&self.upgrade_key_sets));
        w.field("keySetMapping");
        array(w, self.key_set_mapping.as_deref(), |w, (alias, keys)| {
            w.string(alias.as_deref());
            array(w, keys.as_deref(), |w, key| serialized(w, key.as_ref()));
        });
        w.field("protectedBroadcasts");
        w.strings(Some(&self.protected_broadcasts));
        w.field("activities");
        w.list("activities", &self.activities, |w, a| a.write(w));
        w.field("apexSystemServices");
        w.list("apexSystemServices", &self.apex_system_services, |w, s| {
            s.write(w)
        });
        w.field("receivers");
        w.list("receivers", &self.receivers, |w, a| a.write(w));
        w.field("services");
        w.list("services", &self.services, |w, s| s.write(w));
        w.field("providers");
        w.list("providers", &self.providers, |w, p| p.write(w));
        w.field("attributions");
        w.list("attributions", &self.attributions, |w, a| a.write(w));
        w.field("permissions");
        w.list("permissions", &self.permissions, |w, p| p.write(w));
        w.field("permissionGroups");
        w.list("permissionGroups", &self.permission_groups, |w, g| {
            g.write(w)
        });
        w.field("instrumentations");
        w.list("instrumentations", &self.instrumentations, |w, i| {
            i.write(w)
        });
        w.field("preferredActivityFilters");
        w.int(self.preferred_activity_filters.len() as i32);
        for (i, (class, info)) in self.preferred_activity_filters.iter().enumerate() {
            w.scope(format!("preferredActivityFilters[{i}]"), |w| {
                w.string(class.as_deref());
                w.string(Some(
                    "com.android.internal.pm.pkg.component.ParsedIntentInfoImpl",
                ));
                write_intent_info(w, info);
            });
        }
        w.field("processes");
        processes(w, self.processes.as_deref());
        w.field("metaData");
        metadata(w, self.meta_data.as_ref());
        w.field("volumeUuid");
        w.string(self.volume_uuid.as_deref());
        w.field("signingDetails");
        signing(w, self.signing_details.as_ref());
        w.field("mPath");
        w.string(self.path.as_deref());
        w.field("queriesIntents");
        w.list("queriesIntents", &self.queries_intents, |w, i| {
            write_intent(w, i)
        });
        w.field("queriesPackages");
        w.strings(Some(&self.queries_packages));
        w.field("queriesProviders");
        w.strings(Some(&self.queries_providers));
        w.field("appComponentFactory");
        w.string(self.app_component_factory.as_deref());
        w.field("backupAgentName");
        w.string(self.backup_agent_name.as_deref());
        w.field("banner");
        w.int(self.banner);
        w.field("category");
        w.int(self.category);
        w.field("classLoaderName");
        w.string(self.class_loader_name.as_deref());
        w.field("className");
        w.string(self.class_name.as_deref());
        for (name, v) in [
            ("compatibleWidthLimitDp", self.compatible_width_limit_dp),
            ("descriptionRes", self.description_res),
            ("fullBackupContent", self.full_backup_content),
            ("dataExtractionRules", self.data_extraction_rules),
            ("iconRes", self.icon_res),
            ("installLocation", self.install_location),
            ("labelRes", self.label_res),
            ("largestWidthLimitDp", self.largest_width_limit_dp),
            ("logo", self.logo),
        ] {
            w.field(name);
            w.int(v);
        }
        w.field("manageSpaceActivityName");
        w.string(self.manage_space_activity_name.as_deref());
        w.field("maxAspectRatio");
        w.float(self.max_aspect_ratio);
        w.field("minAspectRatio");
        w.float(self.min_aspect_ratio);
        w.field("minSdkVersion");
        w.int(self.min_sdk_version);
        w.field("maxSdkVersion");
        w.int(self.max_sdk_version);
        w.field("networkSecurityConfigRes");
        w.int(self.network_security_config_res);
        w.field("nonLocalizedLabel");
        w.char_sequence(self.non_localized_label.as_deref());
        w.field("permission");
        w.string(self.permission.as_deref());
        w.field("processName");
        w.string(self.process_name.as_deref());
        w.field("requiresSmallestWidthDp");
        w.int(self.requires_smallest_width_dp);
        w.field("roundIconRes");
        w.int(self.round_icon_res);
        w.field("targetSandboxVersion");
        w.int(self.target_sandbox_version);
        w.field("targetSdkVersion");
        w.int(self.target_sdk_version);
        w.field("taskAffinity");
        w.string(self.task_affinity.as_deref());
        w.field("theme");
        w.int(self.theme);
        w.field("uiOptions");
        w.int(self.ui_options);
        w.field("zygotePreloadName");
        w.string(self.zygote_preload_name.as_deref());
        w.field("splitClassLoaderNames");
        match &self.split_class_loader_names {
            Some(v) => {
                w.int(v.len() as i32);
                v.iter().for_each(|s| w.string(s.as_deref()));
            }
            None => w.int(-1),
        }
        w.field("splitCodePaths");
        w.optional_strings(self.split_code_paths.as_deref());
        w.field("splitDependencies");
        match &self.split_dependencies {
            None => w.int(-1),
            Some(deps) => {
                w.int(deps.len() as i32);
                for (index, values) in deps {
                    w.int(*index);
                    w.int(18); // Parcel.VAL_INTARRAY
                    w.ints(values.as_deref());
                }
            }
        }
        w.field("splitFlags");
        w.ints(self.split_flags.as_deref());
        w.field("splitNames");
        w.optional_strings(self.split_names.as_deref());
        w.field("splitRevisionCodes");
        w.ints(self.split_revision_codes.as_deref());
        w.field("resizeableActivity");
        for_boolean(w, self.resizeable_activity);
        w.field("autoRevokePermissions");
        w.int(self.auto_revoke_permissions);
        w.field("mimeGroups");
        w.strings(Some(&self.mime_groups));
        w.field("gwpAsanMode");
        w.int(self.gwp_asan_mode);
        w.field("minExtensionVersions");
        match &self.min_extension_versions {
            Some(v) => {
                w.int(v.len() as i32);
                for &(k, m) in v {
                    w.int(k);
                    w.int(m);
                }
            }
            None => w.int(-1),
        }
        w.field("mProperties");
        properties(w, self.properties.as_deref());
        w.field("memtagMode");
        w.int(self.memtag_mode);
        w.field("nativeHeapZeroInitialized");
        w.int(self.native_heap_zero_initialized);
        w.field("requestRawExternalStorageAccess");
        for_boolean(w, self.request_raw_external_storage_access);
        w.field("mLocaleConfigRes");
        w.int(self.locale_config_res);
        w.field("mKnownActivityEmbeddingCerts");
        w.optional_strings(self.known_activity_embedding_certs.as_deref());
        w.field("manifestPackageName");
        w.string(self.manifest_package_name.as_deref());
        for value in [&self.native_library_dir, &self.native_library_root_dir] {
            w.string(value.as_deref());
        }
        w.bool(self.native_library_root_requires_isa);
        for value in [
            &self.primary_cpu_abi,
            &self.secondary_cpu_abi,
            &self.secondary_native_library_dir,
        ] {
            w.string(value.as_deref());
        }
        w.int(self.uid);
        w.field("mBooleans");
        w.long(self.booleans as i64);
        w.field("mBooleans2");
        w.long(self.booleans2 as i64);
        w.field("mAllowCrossUidActivitySwitchFromBelow");
        w.bool(self.allow_cross_uid_activity_switch_from_below);
        w.field("mIntentMatchingFlags");
        w.int(self.intent_matching_flags);
        w.field("mAlternateLauncherIconResIds");
        w.ints(self.alternate_launcher_icon_res_ids.as_deref());
        w.field("mAlternateLauncherLabelResIds");
        w.ints(self.alternate_launcher_label_res_ids.as_deref());
        w.field("mPageSizeAppCompatFlags");
        w.int(self.page_size_app_compat_flags);
    }
}

impl Component {
    fn write(&self, w: &mut Writer) {
        w.field("name");
        w.string(Some(&self.name));
        w.field("icon");
        w.int(self.icon);
        w.field("labelRes");
        w.int(self.label_res);
        w.field("nonLocalizedLabel");
        w.char_sequence(self.non_localized_label.as_deref());
        w.field("logo");
        w.int(self.logo);
        w.field("banner");
        w.int(self.banner);
        w.field("descriptionRes");
        w.int(self.description_res);
        w.field("flags");
        w.int(self.flags);
        w.field("packageName");
        w.string(Some(&self.package_name));
        w.field("intents");
        w.list("intents", &self.intents, write_intent_info);
        w.field("metaData");
        metadata(w, self.meta_data.as_ref());
        w.field("mProperties");
        properties(w, self.properties.as_deref());
    }
}

impl MainComponent {
    fn write(&self, w: &mut Writer) {
        self.component.write(w);
        w.field("processName");
        w.string(self.process_name.as_deref());
        w.field("directBootAware");
        w.bool(self.direct_boot_aware);
        w.field("enabled");
        w.bool(self.enabled);
        w.field("exported");
        w.bool(self.exported);
        w.field("order");
        w.int(self.order);
        w.field("splitName");
        w.string(self.split_name.as_deref());
        w.field("attributionTags");
        w.optional_strings(self.attribution_tags.as_deref());
        w.field("mIntentMatchingFlags");
        w.int(self.intent_matching_flags);
    }
}

impl Activity {
    fn write(&self, w: &mut Writer) {
        self.main.write(w);
        w.field("theme");
        w.int(self.theme);
        w.field("uiOptions");
        w.int(self.ui_options);
        w.field("targetActivity");
        w.string(self.target_activity.as_deref());
        w.field("parentActivityName");
        w.string(self.parent_activity_name.as_deref());
        w.field("taskAffinity");
        w.string(self.task_affinity.as_deref());
        w.field("privateFlags");
        w.int(self.private_flags);
        w.field("permission");
        w.string(self.permission.as_deref());
        for (name, v) in [
            ("launchMode", self.launch_mode),
            ("documentLaunchMode", self.document_launch_mode),
            ("maxRecents", self.max_recents),
            ("configChanges", self.config_changes),
            ("softInputMode", self.soft_input_mode),
            ("persistableMode", self.persistable_mode),
            ("lockTaskLaunchMode", self.lock_task_launch_mode),
            ("screenOrientation", self.screen_orientation),
            ("resizeMode", self.resize_mode),
        ] {
            w.field(name);
            w.int(v);
        }
        w.field("maxAspectRatio");
        float_value(w, self.max_aspect_ratio);
        w.field("minAspectRatio");
        float_value(w, self.min_aspect_ratio);
        w.field("supportsSizeChanges");
        w.bool(self.supports_size_changes);
        w.field("requestedVrComponent");
        w.string(self.requested_vr_component.as_deref());
        w.field("rotationAnimation");
        w.int(self.rotation_animation);
        w.field("colorMode");
        w.int(self.color_mode);
        // `getMetaData` a second time.
        w.field("metaData2");
        metadata(
            w,
            Some(
                self.main
                    .component
                    .meta_data
                    .as_ref()
                    .unwrap_or(&MetaData::default()),
            ),
        );
        w.field("windowLayout");
        match &self.window_layout {
            Some(l) => {
                w.int(1);
                w.int(l.width);
                w.float(l.width_fraction);
                w.int(l.height);
                w.float(l.height_fraction);
                w.int(l.gravity);
                w.int(l.min_width);
                w.int(l.min_height);
                w.string(l.affinity.as_deref());
            }
            None => w.bool(false),
        }
        w.field("mKnownActivityEmbeddingCerts");
        match &self.known_activity_embedding_certs {
            Some(s) => w.optional_strings(Some(s)),
            None => w.int(-1),
        }
        w.field("mRequiredDisplayCategory");
        w.string(self.required_display_category.as_deref());
        w.field("mRequireContentUriPermissionFromCaller");
        w.int(self.require_content_uri_permission_from_caller);
    }
}

impl Service {
    fn write(&self, w: &mut Writer) {
        self.main.write(w);
        w.field("foregroundServiceType");
        w.int(self.foreground_service_type);
        w.field("permission");
        w.string(self.permission.as_deref());
    }
}

impl Provider {
    fn write(&self, w: &mut Writer) {
        self.main.write(w);
        w.field("authority");
        w.string(self.authority.as_deref());
        w.field("syncable");
        w.bool(self.syncable);
        w.field("readPermission");
        w.string(self.read_permission.as_deref());
        w.field("writePermission");
        w.string(self.write_permission.as_deref());
        w.field("grantUriPermissions");
        w.bool(self.grant_uri_permissions);
        w.field("forceUriPermissions");
        w.bool(self.force_uri_permissions);
        w.field("multiProcess");
        w.bool(self.multi_process);
        w.field("initOrder");
        w.int(self.init_order);
        w.field("uriPermissionPatterns");
        typed(w, self.uri_permission_patterns.as_deref(), write_pattern);
        w.field("pathPermissions");
        typed(w, self.path_permissions.as_deref(), |w, p| {
            write_pattern(w, &p.pattern);
            w.string(p.read_permission.as_deref());
            w.string(p.write_permission.as_deref());
        });
    }
}

impl PermissionGroup {
    fn write(&self, w: &mut Writer) {
        self.component.write(w);
        for (name, v) in [
            ("requestDetailRes", self.request_detail_res),
            ("backgroundRequestRes", self.background_request_res),
            (
                "backgroundRequestDetailRes",
                self.background_request_detail_res,
            ),
            ("requestRes", self.request_res),
            ("priority", self.priority),
        ] {
            w.field(name);
            w.int(v);
        }
    }
}

impl Permission {
    fn write(&self, w: &mut Writer) {
        self.component.write(w);
        w.field("backgroundPermission");
        w.string(self.background_permission.as_deref());
        w.field("group");
        w.string(self.group.as_deref());
        w.field("requestRes");
        w.int(self.request_res);
        w.field("protectionLevel");
        w.int(self.protection_level);
        w.field("tree");
        w.bool(self.tree);
        w.field("parsedPermissionGroup");
        match &self.parsed_permission_group {
            Some(group) => {
                w.string(Some(
                    "com.android.internal.pm.pkg.component.ParsedPermissionGroupImpl",
                ));
                group.write(w);
            }
            None => w.string(None),
        }
        w.field("knownCerts");
        match &self.known_certs {
            Some(s) => w.optional_strings(Some(s)),
            None => w.int(-1),
        }
    }
}

impl Instrumentation {
    fn write(&self, w: &mut Writer) {
        self.component.write(w);
        w.field("targetPackage");
        w.string(self.target_package.as_deref());
        w.field("targetProcesses");
        w.string(self.target_processes.as_deref());
        w.field("handleProfiling");
        w.bool(self.handle_profiling);
        w.field("functionalTest");
        w.bool(self.functional_test);
    }
}

impl Attribution {
    fn write(&self, w: &mut Writer) {
        w.field("tag");
        w.string(self.tag.as_deref());
        w.field("label");
        w.int(self.label);
        w.field("inheritFrom");
        w.optional_strings(self.inherit_from.as_deref());
    }
}

impl Process {
    fn write(&self, w: &mut Writer) {
        w.field("flg");
        w.int(if self.use_embedded_dex { 0x40 } else { 0 });
        w.field("name");
        w.string(self.name.as_deref());
        w.field("appClassNamesByPackage");
        w.int(self.app_class_names_by_package.len() as i32);
        for (k, v) in self.app_class_names_by_package.iter() {
            w.string_value(k);
            match v {
                Some(v) => w.string_value(v),
                None => w.null_value(),
            }
        }
        w.field("deniedPermissions");
        w.strings(Some(&self.denied_permissions));
        w.field("gwpAsanMode");
        w.int(self.gwp_asan_mode);
        w.field("memtagMode");
        w.int(self.memtag_mode);
        w.field("nativeHeapZeroInitialized");
        w.int(self.native_heap_zero_initialized);
    }
}

impl ApexSystemService {
    fn write(&self, w: &mut Writer) {
        let mut flg = 0;
        if self.jar_path.is_some() {
            flg |= 0x2;
        }
        if self.min_sdk_version.is_some() {
            flg |= 0x4;
        }
        if self.max_sdk_version.is_some() {
            flg |= 0x8;
        }
        w.field("flg");
        w.int(flg);
        w.field("name");
        w.string(self.name.as_deref());
        for (name, v) in [
            ("jarPath", &self.jar_path),
            ("minSdkVersion", &self.min_sdk_version),
            ("maxSdkVersion", &self.max_sdk_version),
        ] {
            w.field(name);
            w.string(v.as_deref());
        }
        w.field("initOrder");
        w.int(self.init_order);
    }
}

impl UsesPermission {
    fn write(&self, w: &mut Writer) {
        w.field("name");
        w.string(self.name.as_deref());
        w.field("usesPermissionFlags");
        w.int(self.flags);
    }
}

impl FeatureInfo {
    fn write(&self, w: &mut Writer) {
        w.string(self.name.as_deref());
        w.int(self.version);
        w.int(self.req_gl_es_version);
        w.int(self.flags);
    }
}

fn array<T>(w: &mut Writer, values: Option<&[T]>, mut write: impl FnMut(&mut Writer, &T)) {
    let Some(values) = values else {
        return w.int(-1);
    };
    w.int(values.len() as i32);
    for value in values {
        write(w, value);
    }
}
fn typed<T>(w: &mut Writer, values: Option<&[T]>, mut write: impl FnMut(&mut Writer, &T)) {
    array(w, values, |w, value| {
        w.int(1);
        write(w, value);
    });
}
fn for_boolean(w: &mut Writer, value: Option<bool>) {
    w.int(match value {
        None => 1,
        Some(false) => 0,
        Some(true) => -1,
    });
}
fn float_value(w: &mut Writer, value: Option<f32>) {
    match value {
        Some(value) => w.float_value(value),
        None => w.null_value(),
    }
}
fn digests(w: &mut Writer, values: Option<&[Option<Vec<Option<String>>>]>) {
    array(w, values, |w, value| w.optional_strings(value.as_deref()));
}
fn serialized(w: &mut Writer, value: Option<&Serialized>) {
    match value {
        Some(value) => {
            w.string(Some(&value.class));
            w.bytes(Some(&value.bytes));
        }
        None => w.string(None),
    }
}
fn signing(w: &mut Writer, value: Option<&SigningDetails>) {
    w.string(Some("android.content.pm.SigningDetails"));
    w.bool(value.is_none());
    if let Some(value) = value {
        typed(w, value.signatures.as_deref(), |w, value| {
            w.bytes(Some(value))
        });
        w.int(value.scheme_version);
        array(w, value.public_keys.as_deref(), |w, value| match value {
            Some(value) => w.prefixed(VAL_SERIALIZABLE, |w| serialized(w, Some(value))),
            None => w.null_value(),
        });
        typed(w, value.past_signing_certificates.as_deref(), |w, value| {
            w.bytes(Some(value))
        });
    }
}
fn string_map(w: &mut Writer, values: Option<&[(String, Option<String>)]>) {
    array(w, values, |w, (key, value)| {
        w.string_value(key);
        match value {
            Some(value) => w.string_value(value),
            None => w.null_value(),
        }
    });
}
fn metadata(w: &mut Writer, values: Option<&MetaData>) {
    w.bundle_entries(values.map(|v| v.0.as_slice()), |w, (key, value)| {
        w.string(Some(key));
        match value {
            Value::Null => w.null_value(),
            Value::String(value) => {
                w.int(VAL_STRING);
                w.string(value.as_deref());
            }
            Value::Int(value) => {
                w.int(VAL_INTEGER);
                w.int(*value);
            }
            Value::Long(value) => {
                w.int(VAL_LONG);
                w.long(*value);
            }
            Value::Float(value) => w.float_value(*value),
            Value::Double(value) => {
                w.int(8);
                w.double(*value);
            }
            Value::Bool(value) => {
                w.int(VAL_BOOLEAN);
                w.bool(*value);
            }
        }
    });
}
fn properties(w: &mut Writer, values: Option<&[(String, Property)]>) {
    array(w, values, |w, (key, value)| {
        w.string_value(key);
        w.prefixed(VAL_PARCELABLE, |w| {
            w.string(Some("android.content.pm.PackageManager$Property"));
            w.string(value.name.as_deref());
            let kind = match value.value {
                PropertyValue::Bool(_) => 1,
                PropertyValue::Float(_) => 2,
                PropertyValue::Int(_) => 3,
                PropertyValue::Resource(_) => 4,
                PropertyValue::String(_) => 5,
                PropertyValue::Unknown(kind) => kind,
            };
            w.int(kind);
            w.string(value.package_name.as_deref());
            w.string(value.class_name.as_deref());
            match &value.value {
                PropertyValue::Bool(value) => w.bool(*value),
                PropertyValue::Float(value) => w.float(*value),
                PropertyValue::Int(value) | PropertyValue::Resource(value) => w.int(*value),
                PropertyValue::String(value) => w.string(value.as_deref()),
                PropertyValue::Unknown(_) => {}
            }
        });
    });
}
fn processes(w: &mut Writer, values: Option<&[Process]>) {
    array(w, values, |w, value| {
        match &value.map_key {
            Some(name) => w.string_value(name),
            None => w.null_value(),
        }
        w.prefixed(VAL_PARCELABLE, |w| {
            w.string(Some(
                "com.android.internal.pm.pkg.component.ParsedProcessImpl",
            ));
            value.write(w);
        });
    });
}
fn write_pattern(w: &mut Writer, value: &PatternMatcher) {
    value.write_cache(w);
}
fn write_intent_info(w: &mut Writer, value: &ParsedIntentInfo) {
    w.int(
        i32::from(value.has_default)
            | if value.non_localized_label.is_some() {
                4
            } else {
                0
            },
    );
    w.int(value.label_res);
    if let Some(label) = &value.non_localized_label {
        w.char_sequence(Some(label));
    }
    w.int(value.icon);
    w.int(1);
    write_filter(w, &value.filter);
}
fn write_filter(w: &mut Writer, value: &IntentFilter) {
    w.strings(Some(&value.actions));
    for values in [
        &value.categories,
        &value.schemes,
        &value.static_types,
        &value.types,
        &value.mime_groups,
    ] {
        w.bool(values.is_some());
        if let Some(values) = values {
            w.strings(Some(values));
        }
    }
    let ssps = value.ssps.as_deref().unwrap_or_default();
    w.int(ssps.len() as i32);
    for pattern in ssps {
        write_pattern(w, pattern);
    }
    let authorities = value.authorities.as_deref().unwrap_or_default();
    w.int(authorities.len() as i32);
    for value in authorities {
        w.string(Some(&value.orig_host));
        w.string(Some(&value.host));
        w.bool(value.wild);
        w.int(value.port);
    }
    let paths = value.paths.as_deref().unwrap_or_default();
    w.int(paths.len() as i32);
    for pattern in paths {
        write_pattern(w, pattern);
    }
    w.int(value.priority);
    w.bool(value.has_static_partial_types);
    w.bool(value.has_dynamic_partial_types);
    w.bool(value.auto_verify);
    w.int(value.instant_app_visibility);
    w.int(value.order);
    w.bool(value.extras.is_some());
    if value.extras.is_some() {
        w.int(0);
    }
    let groups = value
        .uri_relative_filter_groups
        .as_deref()
        .unwrap_or_default();
    w.int(groups.len() as i32);
    for group in groups {
        w.int(group.action);
        w.int(group.filters.len() as i32);
        for filter in &group.filters {
            w.int(filter.uri_part);
            w.int(filter.pattern_type);
            w.string(Some(&filter.filter));
        }
    }
}
fn write_intent(w: &mut Writer, value: &Intent) {
    w.string(value.action.as_deref());
    match &value.data {
        Some(uri) => uri.write_cache(w),
        None => w.int(0),
    }
    w.string(value.ty.as_deref());
    w.string(value.identifier.as_deref());
    w.int(value.flags);
    w.int(value.extended_flags);
    w.string(value.package.as_deref());
    match &value.component {
        Some(value) => {
            w.string(Some(&value.package));
            w.string(Some(&value.class));
        }
        None => w.string(None),
    }
    w.int(0); // Source bounds are not part of a manifest query intent.
    match &value.categories {
        Some(values) => w.strings(Some(values)),
        None => w.int(0),
    }
    w.bool(value.selector.is_some());
    if let Some(value) = &value.selector {
        write_intent(w, value);
    }
    w.int(0);
    w.int(-2);
    w.int(-1);
    w.int(0);
    w.int(0);
}
fn validate(pkg: &AndroidPackage) -> std::result::Result<(), String> {
    pkg.validate_cache_constructor_fields()?;
    let check_properties =
        |values: Option<&[(String, Property)]>| -> std::result::Result<(), String> {
            if values
                .into_iter()
                .flatten()
                .any(|(_, v)| matches!(v.value, PropertyValue::Unknown(_)))
            {
                Err("PackageImpl contains an unknown property type".into())
            } else {
                Ok(())
            }
        };
    let check_filter = |value: &ParsedIntentInfo| -> std::result::Result<(), String> {
        if value
            .filter
            .extras
            .as_ref()
            .is_some_and(|v| !v.is_empty() && v.as_slice() != 0i32.to_le_bytes())
        {
            Err("pooled IntentFilter extras require decoded PersistableBundle values (#723)".into())
        } else {
            Ok(())
        }
    };
    check_properties(pkg.properties.as_deref())?;
    let components = pkg
        .activities
        .iter()
        .chain(&pkg.receivers)
        .map(|v| &v.main.component)
        .chain(pkg.services.iter().map(|v| &v.main.component))
        .chain(pkg.providers.iter().map(|v| &v.main.component))
        .chain(pkg.permissions.iter().map(|v| &v.component))
        .chain(pkg.permission_groups.iter().map(|v| &v.component))
        .chain(pkg.instrumentations.iter().map(|v| &v.component))
        .chain(
            pkg.permissions
                .iter()
                .filter_map(|v| v.parsed_permission_group.as_ref().map(|v| &v.component)),
        );
    for component in components {
        check_properties(component.properties.as_deref())?;
        for value in &component.intents {
            check_filter(value)?;
        }
    }
    for (_, value) in &pkg.preferred_activity_filters {
        check_filter(value)?;
    }
    Ok(())
}
