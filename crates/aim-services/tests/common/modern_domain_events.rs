//! Detached original domain persistence read/write projections; live merge is separate.
use aim_android_xml::pull::{Event, Reader};
use aim_services::package::{
    domain_verification::{State, uuid},
    settings::ReadError,
};

pub fn inputs() -> Vec<Vec<u8>> {
    let pkg = "packageName='p' id='00000000-0000-0000-0000-000000000001'";
    let documents = [
        format!("<domain-verifications><active><package-state {pkg}><state><domain name='x' state='2'/><domain name='x' state='3'/><domain state='bad'/></state><user-states><user-state userId='1' allowLinkHandling='true'><enabled-hosts><host name='x'/><host name='x'/><host name=''/></enabled-hosts></user-state></user-states></package-state></active></domain-verifications>"),
        format!("<domain-verifications><unknown><active><unknown><package-state {pkg}><unknown><state><unknown><domain name='x' state='2'/></unknown></state></unknown></package-state></unknown></active></unknown></domain-verifications>"),
        format!("<domain-verifications><active><package-state {pkg}/><package-state {pkg} signature='new'/></active><restored><package-state {pkg} signature='restored'/></restored></domain-verifications>"),
        "<domain-verifications><active><package-state packageName='' id='bad'><package-state packageName='p' id='00000000-0000-0000-0000-000000000001'/></package-state></active></domain-verifications>".into(),
        "<domain-verifications><active><package-state packageName='p' id='bad'/></active></domain-verifications>".into(),
        format!("<domain-verifications><active><package-state {pkg}><state><domain name='x' state='2'/></state><"),
        format!("<domain-verifications><active><package-state {pkg}><uri-relative-filter-groups><domain name='x'><uri-relative-filter-group action='1'><uri-relative-filter uri-part='0' pattern-type='0' filter='/path'/><uri-relative-filter/></uri-relative-filter-group></domain></uri-relative-filter-groups></package-state></active></domain-verifications>"),
    ];
    let mut inputs = Vec::new();
    for document in &documents {
        inputs.push(document.as_bytes().to_vec());
        if let Ok(root) = aim_android_xml::read(document.as_bytes()) {
            inputs.push(aim_android_xml::abx::write(&root).unwrap());
        }
    }
    let binary =
        aim_android_xml::abx::write(&aim_android_xml::read(documents[0].as_bytes()).unwrap())
            .unwrap();
    for end in 1..binary.len() {
        if let Ok(mut reader) = Reader::new(&binary[..end]) {
            if matches!(reader.next(), Ok(Event::Start(_))) {
                inputs.push(binary[..end].to_vec());
            }
        }
    }
    inputs
}

pub fn read(bytes: &[u8]) -> (State, &'static str) {
    let mut reader = Reader::with_document(bytes).unwrap();
    reader.next().unwrap();
    match State::read_events(&mut reader, |id| {
        uuid::parse(id, true).map_err(ReadError::File)
    }) {
        Ok(result) => (result.state, "ok"),
        Err(ReadError::File(_)) => (State::default(), "invalid"),
        Err(error) => panic!("unexpected domain owner error: {error}"),
    }
}

pub fn normalize(state: &mut State) {
    for packages in [&mut state.active, &mut state.restored] {
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        for p in packages {
            p.domains.sort_by(|a, b| a.0.cmp(&b.0));
            p.users.sort_by_key(|u| u.id);
            for u in &mut p.users {
                u.enabled_hosts.sort();
            }
            p.uri_relative_filter_groups.sort_by(|a, b| a.0.cmp(&b.0));
        }
    }
}
