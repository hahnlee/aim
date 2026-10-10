//! Original disabled-system setting event publication and permission owners.
use aim_services::package::{
    owner::{
        app_ids::{AppIds, Owner},
        legacy_permissions::InstallRead,
    },
    settings::{PackageReadAttempt, ReadError, Settings},
};
use std::{collections::BTreeMap, fs, path::Path};
pub fn export(directory: &Path) {
    let active = "<package name='p' codePath='/p' userId='10001' domainSetId='00000000-0000-0000-0000-000000000001'><perms><item name='active'/></perms></package>";
    let group =
        "<shared-user name='g' userId='10002'><perms><item name='group'/></perms></shared-user>";
    let factory = "<updated-package name='f' codePath='/system/priv-app/f' userId='10003' version='7'><perms><item name='factory'/></perms><uses-static-lib name='lib' version='4'/></updated-package>";
    let documents=[
        format!("<packages>{active}{group}{factory}</packages>"),
        format!("<packages>{active}{} </packages>",factory.replace("name='f'","name='p'").replace("10003","10001")),
        format!("<packages>{group}{} </packages>",factory.replace("userId='10003'","sharedUserId='10002'")),
        format!("<packages>{active}{} </packages>",factory.replace("userId='10003'","sharedUserId='10001'")),
        format!("<packages>{}{group}</packages>",factory.replace("userId='10003'","sharedUserId='10002'")),
        format!("<packages>{factory}{} </packages>",factory.replace("version='7'","version='8'").replace("/priv-app/","/app/").replace("name='factory'","name='replacement'")),
        format!("<packages>{group}<updated-package name='f' codePath='/system/app/f' sharedUserId='10002'><perms><item name='partial'/></perms><unknown><"),
        "<packages><updated-package name='f' codePath='/system/app/f' sharedUserId='-1'><perms><item name='negative'/></perms><sigs><item index='0' key='not-a-cert'/></sigs></updated-package></packages>".into(),
    ];
    let mut inputs = Vec::new();
    for xml in documents {
        inputs.push(xml.as_bytes().to_vec());
        if let Ok(root) = aim_android_xml::read(xml.as_bytes()) {
            inputs.push(aim_android_xml::abx::write(&root).unwrap());
        }
    }
    for (index, bytes) in inputs.iter().enumerate() {
        fs::write(directory.join(format!("factory-event-{index}.xml")), bytes).unwrap();
        let mut state = Settings::default();
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
        let read = state.read_owned_document(bytes, &mut ids, &mut attempt, true, &mut owners);
        if matches!(read, Err(ReadError::File(_))) {
            state
                .read_owned_document(b"<packages/>", &mut ids, &mut attempt, true, &mut owners)
                .unwrap();
        } else {
            read.unwrap();
        }
        if let Ok(root) = aim_android_xml::read(bytes) {
            let ast = Settings::parse(&root).unwrap();
            let normalize = |mut factories: Vec<aim_services::package::settings::Package>| {
                // These IDs are constructor/runtime inputs rather than saved XML.
                for p in &mut factories {
                    p.domain_set_id = None;
                    p.shared_user_app_id = None;
                }
                factories
            };
            assert_eq!(
                normalize(ast.disabled_system_packages),
                normalize(state.disabled_system_packages.clone()),
                "factory AST/stream import differs on case {index}"
            );
        }
        for name in ["p", "f"] {
            let setting = state
                .disabled_system_packages
                .iter()
                .find(|p| p.name == name);
            let trace = setting
                .map(|p| {
                    format!(
                        "{}|{}|{}|{}|{}|{}|{}|{}",
                        p.flags,
                        p.private_flags,
                        p.app_id,
                        p.shared_user,
                        p.domain_set_id.as_deref().unwrap(),
                        p.version_code,
                        p.code_path,
                        p.install_permissions_fixed
                    )
                })
                .unwrap_or_else(|| "absent".into());
            fs::write(
                directory.join(format!("factory-event-{index}-{name}.metadata")),
                trace,
            )
            .unwrap();
            if let Some(p) = setting {
                fs::write(
                    directory.join(format!("factory-event-{index}-{name}.permissions")),
                    factories[name].project(p.app_id, &[10, 0]).unwrap().bytes(),
                )
                .unwrap();
            }
        }
        for id in [10001, 10002, 10003] {
            let state = match ids.get(id) {
                Some(Owner::Package(name)) => Some(&packages[name]),
                Some(Owner::SharedUser(name)) => Some(&groups[name]),
                None => None,
                Some(_) => panic!("detached outside factory fixture"),
            };
            if let Some(state) = state {
                fs::write(
                    directory.join(format!("factory-event-{index}-{id}.uid")),
                    state.project(id, &[10, 0]).unwrap().bytes(),
                )
                .unwrap();
            }
        }
    }
}
