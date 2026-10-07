//! Install-permission event binding against original Settings.readSettingsLPw.
use aim_android_xml::{Element, pull::Reader};
use aim_services::package::{
    owner::{
        app_ids::{AppIds, Owner},
        legacy_permissions::{InstallRead, Migration},
    },
    settings::{Package, PackageReadAttempt, ReadError, ReadOwners, Settings, SharedUser},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

pub fn export(directory: &Path) {
    let own = "<package name='p' codePath='/p' userId='10001' domainSetId='00000000-0000-0000-0000-000000000001'><perms><item name='own'/></perms></package>";
    let group =
        "<shared-user name='g' userId='10002'><perms><item name='group'/></perms></shared-user>";
    let shared = "<package name='q' codePath='/q' sharedUserId='10002' domainSetId='00000000-0000-0000-0000-000000000002'><perms><item name='shared' flags='17'/></perms></package>";
    let documents = [
        format!("<packages>{own}{group}{shared}</packages>"),
        format!("<packages>{shared}{group}</packages>"),
        format!("<packages>{own}{} </packages>", shared.replace("10002", "10001")),
        format!("<packages>{group}<package name='q' codePath='/q' sharedUserId='10002' domainSetId='00000000-0000-0000-0000-000000000002'><perms><item name='partial'/><unknown><"),
        "<packages><package name='p' codePath='/p' userId='10001' domainSetId='00000000-0000-0000-0000-000000000001'><perms><item name='outer'><item name='inner'/></item><unknown><item name='ignored'/></unknown></perms></package></packages>".into(),
    ];
    let mut inputs = Vec::new();
    for text in documents {
        inputs.push(text.as_bytes().to_vec());
        if let Ok(root) = aim_android_xml::read(text.as_bytes()) {
            inputs.push(aim_android_xml::abx::write(&root).unwrap());
        }
    }
    for (index, bytes) in inputs.iter().enumerate() {
        fs::write(
            directory.join(format!("install-binding-{index}.xml")),
            bytes,
        )
        .unwrap();
        let mut settings = Settings::default();
        let mut ids = AppIds::default();
        let mut attempt = PackageReadAttempt::default();
        // Explicit original SettingBase constructors for this controlled fixture.
        let mut packages = BTreeMap::from([
            ("p".into(), Migration::default()),
            ("q".into(), Migration::default()),
        ]);
        let mut groups = BTreeMap::from([("g".into(), Migration::default())]);
        let mut fixed = BTreeSet::new();
        let mut owners = InstallRead {
            users: &[10, 0],
            packages: &mut packages,
            shared_users: &mut groups,
            install_permissions_fixed: &mut fixed,
            remaining: &mut Remaining,
        };
        let read = settings.read_owned_document(bytes, &mut ids, &mut attempt, true, &mut owners);
        if matches!(read, Err(ReadError::File(_))) {
            settings
                .read_owned_document(b"<packages/>", &mut ids, &mut attempt, true, &mut owners)
                .unwrap();
        } else {
            read.unwrap();
        }
        for id in [10001, 10002] {
            let state = match ids.get(id) {
                Some(Owner::Package(name)) => Some(&packages[name]),
                Some(Owner::SharedUser(name)) => Some(&groups[name]),
                None => None,
                Some(_) => panic!("unexpected detached fixture"),
            };
            if let Some(state) = state {
                fs::write(
                    directory.join(format!("install-binding-{index}-{id}.input")),
                    state.project(id, &[10, 0]).unwrap().bytes(),
                )
                .unwrap();
            }
        }
        let flag = if settings.packages.iter().any(|p| p.name == "p") {
            fixed.contains("p").to_string()
        } else {
            "absent".into()
        };
        fs::write(
            directory.join(format!("install-binding-{index}.fixed")),
            flag,
        )
        .unwrap();
    }
}

struct Remaining;
impl ReadOwners for Remaining {
    fn package_child(
        &mut self,
        _: &mut Package,
        _: &mut Reader<'_>,
        _: &Element,
        _: &AppIds,
    ) -> Result<bool, ReadError> {
        Ok(false)
    }
    fn shared_child(
        &mut self,
        _: &mut SharedUser,
        _: &mut Reader<'_>,
        _: &Element,
    ) -> Result<bool, ReadError> {
        Ok(false)
    }
    fn public_key(&mut self, _: &[u8]) -> Result<Option<Vec<u8>>, ReadError> {
        panic!("key outside install binding fixture")
    }
    fn global_record(
        &mut self,
        _: &mut Settings,
        _: &mut Reader<'_>,
        _: &Element,
    ) -> Result<bool, ReadError> {
        Ok(false)
    }
}
