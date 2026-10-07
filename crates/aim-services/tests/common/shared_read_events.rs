//! Package/shared UID read-order projections against original Settings.readLPw.
use aim_android_xml::{Element, pull::Reader};
use aim_services::package::{
    owner::app_ids::AppIds,
    settings::{Package, PackageReadAttempt, ReadError, ReadOwners, Settings, SharedUser},
};

pub fn inputs() -> Vec<Vec<u8>> {
    let package = "<package name='p' codePath='/p' sharedUserId='10001' publicFlags='0' domainSetId='00000000-0000-0000-0000-000000000001'/>";
    let group = "<shared-user name='g' userId='10001' system='true'/>";
    let documents = [
        format!("<packages>{package}{group}</packages>"),
        format!("<packages>{group}{package}</packages>"),
        format!("<packages>{package}</packages>"),
        format!(
            "<packages><package name='q' codePath='/q' userId='10001' domainSetId='00000000-0000-0000-0000-000000000001'/>{group}{package}</packages>"
        ),
        format!(
            "<packages>{group}<shared-user name='g' userId='10001' system='false'/>{package}</packages>"
        ),
        format!("<packages>{group}<shared-user name='g' userId='10002'/>{package}</packages>"),
        format!("<packages>{group}<shared-user name='other' userId='10001'/>{package}</packages>"),
        format!("<packages>{package}<shared-user name='g' userId='10001' system='true'><"),
        format!(
            "<packages>{group}<package name='p' codePath='/p' sharedUserId='10001' publicFlags='0' domainSetId='00000000-0000-0000-0000-000000000001'><"
        ),
        format!("<packages>{group}<shared-user name='g' userId='10001'><sigs><"),
        format!(
            "<packages>{group}<shared-user name='g' userId='10001'><unknown><shared-user name='hidden' userId='10002'/></unknown></shared-user>{package}</packages>"
        ),
    ];
    let mut inputs = Vec::new();
    for document in documents {
        let bytes = document.into_bytes();
        inputs.push(bytes.clone());
        if let Ok(root) = aim_android_xml::read(&bytes) {
            inputs.push(aim_android_xml::abx::write(&root).unwrap());
        }
    }
    let bytes = aim_android_xml::abx::write(
        &aim_android_xml::read(format!("<packages>{package}{group}</packages>").as_bytes())
            .unwrap(),
    )
    .unwrap();
    for end in 4..bytes.len() {
        inputs.push(bytes[..end].to_vec());
    }
    inputs
}

pub fn read(bytes: &[u8]) -> Settings {
    let mut settings = Settings::default();
    let mut ids = AppIds::default();
    let mut attempt = PackageReadAttempt::default();
    let result = settings.read_owned_document(bytes, &mut ids, &mut attempt, true, &mut ProjectionOwners);
    if result.is_err() {
        // failRead re-enters with an empty reserve and clears attempt tables.
        attempt = PackageReadAttempt::default();
    }
    // The projection excludes group membership/user/legacy binding side effects.
    attempt
        .resolve_pending(&mut settings, &mut ids, |_, _, _, _| Ok(()))
        .unwrap();
    settings
}

pub fn trace(settings: &Settings) -> String {
    let mut packages = settings
        .packages
        .iter()
        .map(|p| format!("{}:{}:{}:{}", p.name, p.app_id, p.shared_user, p.code_path))
        .collect::<Vec<_>>();
    let mut groups = settings
        .shared_users
        .iter()
        .map(|g| format!("{}:{}:{}", g.name, g.app_id, g.flags))
        .collect::<Vec<_>>();
    packages.sort();
    groups.sort();
    format!("{}|{}", packages.join(";"), groups.join(";"))
}

// This projection deliberately excludes external legacy/user/global owners.
struct ProjectionOwners;
impl ReadOwners for ProjectionOwners {
    fn package_child(
        &mut self,
        _: &mut Package,
        _: &mut Reader<'_>,
        _: &Element,
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
        panic!("key outside shared UID projection")
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
