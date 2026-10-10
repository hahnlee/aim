//! Constructor shared UID inputs applied before incremental settings reads.
use aim_services::package::{
    owner::{app_ids::AppIds, legacy_permissions::InstallRead, shared_users::Bootstrap},
    settings::{PackageReadAttempt, Settings},
    system_config::SystemConfig,
};
use std::{collections::BTreeMap, fs, path::Path};
pub fn export(directory: &Path) {
    let docs = [
        "<packages/>",
        "<packages><shared-user name='android.uid.system' userId='1000' system='false'><perms><item name='retained'/></perms></shared-user></packages>",
        "<packages><package name='q' codePath='/q' sharedUserId='1000' domainSetId='00000000-0000-0000-0000-000000000001'><perms><item name='early'/></perms></package></packages>",
        "<packages><shared-user name='android.uid.system' userId='1001'><perms><item name='rejected'/></perms></shared-user><package name='q' codePath='/q' sharedUserId='2901' domainSetId='00000000-0000-0000-0000-000000000001'><perms><item name='vendor'/></perms></package></packages>",
    ];
    let mut inputs = Vec::new();
    for xml in docs {
        inputs.push(xml.as_bytes().to_vec());
        inputs.push(
            aim_android_xml::abx::write(&aim_android_xml::read(xml.as_bytes()).unwrap()).unwrap(),
        );
    }
    for (index, bytes) in inputs.iter().enumerate() {
        fs::write(
            directory.join(format!("shared-seed-event-{index}.xml")),
            bytes,
        )
        .unwrap();
        let mut config = SystemConfig::default();
        config.oem_defined_uids = vec![("android.uid.vendor.fixture".into(), 2901)];
        let boot = Bootstrap::new(&config);
        let mut settings = Settings::default();
        let mut ids = AppIds::default();
        let mut attempt = PackageReadAttempt::default();
        let mut packages = BTreeMap::new();
        let mut groups = BTreeMap::new();
        let mut factories = BTreeMap::new();
        let mut owners = InstallRead {
            users: &[10, 0],
            packages: &mut packages,
            factories: &mut factories,
            shared_users: &mut groups,
            remaining: &mut super::install_read_events::Remaining::default(),
        };
        settings
            .initialize_shared_bootstrap(&boot, &mut ids, &mut owners)
            .unwrap();
        assert_eq!(ids, boot.ids);
        assert!(boot.rejected.is_empty());
        settings
            .read_owned_document(bytes, &mut ids, &mut attempt, true, &mut owners)
            .unwrap();
        assert!(
            settings
                .initialize_shared_bootstrap(&boot, &mut ids, &mut owners)
                .is_err()
        );
        for group in &settings.shared_users {
            fs::write(
                directory.join(format!(
                    "shared-seed-event-{index}-{}.permissions",
                    group.app_id
                )),
                groups[&group.name]
                    .project(group.app_id, &[10, 0])
                    .unwrap()
                    .bytes(),
            )
            .unwrap();
            fs::write(
                directory.join(format!(
                    "shared-seed-event-{index}-{}.metadata",
                    group.app_id
                )),
                format!("{}|{}", group.name, group.flags),
            )
            .unwrap();
        }
    }
}
