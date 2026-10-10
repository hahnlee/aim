//! Original DomainVerificationSettings.readSettings merge projections.
use aim_services::package::{
    domain_verification::{
        State,
        collector::{self, Kind, Policy},
        owner::{Input, Owner},
        uuid,
    },
    pkg::AndroidPackage,
    settings::ReadError,
    system_config::SystemConfig,
};
use std::{fs, path::Path};
const OLD: &str = "00000000-0000-0000-0000-000000000001";
const SEED: &str = "00000000-0000-0000-0000-000000000002";
const NEW: &str = "00000000-0000-0000-0000-000000000003";

pub fn export(directory: &Path) {
    let seed = format!(
        "<domain-verifications><active><package-state packageName='fixture.domains' id='{SEED}'><state><domain name='h0.example' state='1'/></state><user-states><user-state userId='1' allowLinkHandling='true'><enabled-hosts><host name='old-host'/></enabled-hosts></user-state></user-states></package-state></active></domain-verifications>"
    );
    fs::write(directory.join("domain-read-merge-seed"), seed).unwrap();
    let mut index = 0;
    for same in [false, true] {
        for state in [0, 1, 5, 1024] {
            for allow in [false, true] {
                for code in [false, true] {
                    let id = if same { OLD } else { NEW };
                    let input = format!(
                        "<domain-verifications><active><package-state packageName='fixture.domains' id='{id}' signature='new'><state><domain name='h0.example' state='1'/><domain name='h1.example' state='{state}'/><domain name='undeclared' state='1'/></state><user-states><user-state userId='1' allowLinkHandling='{allow}'><enabled-hosts><host name='new-host'/></enabled-hosts></user-state><user-state userId='2' allowLinkHandling='true'><enabled-hosts><host name='outside'/></enabled-hosts></user-state></user-states></package-state><package-state packageName='pending' id='{NEW}'/></active><restored><package-state packageName='fixture.domains' id='{NEW}' signature='restore'/></restored></domain-verifications>"
                    );
                    fs::write(
                        directory.join(format!("domain-read-merge-{index}.input")),
                        input,
                    )
                    .unwrap();
                    fs::write(
                        directory.join(format!("domain-read-merge-{index}.code")),
                        code.to_string(),
                    )
                    .unwrap();
                    index += 1;
                }
            }
        }
    }
}

fn parsed(bytes: &[u8]) -> aim_services::package::domain_verification::ReadResult {
    let mut reader = aim_android_xml::pull::Reader::new(bytes).unwrap();
    reader.next().unwrap();
    State::read_events(&mut reader, |id| {
        uuid::parse(id, true).map_err(ReadError::File)
    })
    .unwrap()
}

pub fn verify(directory: &Path) {
    let code =
        AndroidPackage::read_cache_entry(&fs::read(directory.join("domain-owner.cache")).unwrap())
            .unwrap();
    let config = SystemConfig::default();
    let valid = collector::collect(
        &code,
        Policy {
            restrict_domains: true,
            linked_app: false,
        },
        Kind::ValidAutoVerify,
    );
    let seed = fs::read(directory.join("domain-read-merge-seed")).unwrap();
    for index in 0..32 {
        let mut owner = Owner::new(State::default(), Default::default());
        owner
            .add(
                Input {
                    id: OLD,
                    name: "fixture.domains",
                    code: Some(&code),
                    signatures: &[],
                    system: false,
                    restrict_domains: true,
                    pre_verified: None,
                },
                &config,
            )
            .unwrap();
        owner
            .read_settings(parsed(&seed), |_| Ok(valid.clone()))
            .unwrap();
        let present = fs::read_to_string(directory.join(format!("domain-read-merge-{index}.code")))
            .unwrap()
            == "true";
        owner
            .read_settings(
                parsed(
                    &fs::read(directory.join(format!("domain-read-merge-{index}.input"))).unwrap(),
                ),
                |_| Ok(if present { valid.clone() } else { vec![] }),
            )
            .unwrap();
        let root = aim_android_xml::read_next(
            &fs::read(directory.join(format!("domain-read-merge-{index}.original"))).unwrap(),
        )
        .unwrap();
        let mut actual = State::default();
        actual.read(&root).unwrap();
        let mut expected = owner.xml_projection();
        super::modern_domain_events::normalize(&mut actual);
        super::modern_domain_events::normalize(&mut expected);
        assert_eq!(
            actual, expected,
            "original domain settings live merge {index}"
        );
    }
}
