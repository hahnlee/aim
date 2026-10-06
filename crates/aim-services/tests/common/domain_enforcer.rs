//! Expectations for the original domain Enforcer, with explicit controlled owners.
use aim_binder_host::parcel::Parcel;
use aim_services::package::domain_verification::enforcer::*;
use std::{fs, path::Path};
struct Fixture {
    mask: i32,
    verifier: bool,
    missing: i32,
    hidden: bool,
}
impl Owners for Fixture {
    fn permission(&self, _: i32, permission: &str) -> Result<bool, String> {
        let index = [
            DUMP,
            QUERY_ALL,
            AGENT,
            LEGACY_AGENT,
            CROSS_USER,
            UPDATE,
            CROSS_USER_FULL,
            PREFERRED,
        ]
        .iter()
        .position(|p| *p == permission)
        .unwrap();
        Ok(self.mask & (1 << index) != 0)
    }
    fn verifier(&self, _: i32) -> Result<bool, String> {
        Ok(self.verifier)
    }
    fn user_exists(&self, user: i32) -> Result<bool, String> {
        Ok([0, 10].contains(&user) && user != self.missing)
    }
    fn filtered(&self, package: &str, _: i32, _: i32) -> Result<bool, String> {
        assert_eq!(package, "fixture.domains");
        Ok(self.hidden)
    }
}
pub fn export(directory: &Path) {
    let mut cases = Parcel::new();
    let masks = [0, 1, 2, 4, 8, 16, 32, 64, 128, 6, 10, 48, 50, 192, 144, 255];
    let scenarios = [
        (0, -1, false),
        (10, -1, false),
        (0, 0, false),
        (10, 10, false),
        (0, -1, true),
        (10, -1, true),
    ];
    cases.write_i32((4 * 2 * masks.len() * scenarios.len() * 9) as i32);
    for uid in [0, 1000, 2000, 10001] {
        for verifier in [false, true] {
            for mask in masks {
                for (target, missing, hidden) in scenarios {
                    for kind in 0..9 {
                        let fixture = Fixture {
                            mask,
                            verifier,
                            missing,
                            hidden,
                        };
                        let op = match kind {
                            0 => Operation::Internal,
                            1 => Operation::Info,
                            2 => Operation::Verifier,
                            3 => Operation::UserQuery("fixture.domains", target),
                            4 => Operation::UserSelect(Some("fixture.domains"), target),
                            5 => Operation::Owners(target),
                            6 => Operation::LegacySelect("fixture.domains", target),
                            7 => Operation::LegacyQuery("fixture.domains", target),
                            _ => Operation::UserSelect(None, target),
                        };
                        let expected = match authorize(&fixture, uid, 0, op) {
                            Ok(true) => 1,
                            Ok(false) => 0,
                            Err(Error::Security) => -1,
                            Err(Error::Owner(error)) => panic!("unexpected missing owner: {error}"),
                        };
                        for value in [
                            uid,
                            i32::from(verifier),
                            mask,
                            target,
                            missing,
                            i32::from(hidden),
                            kind,
                            expected,
                        ] {
                            cases.write_i32(value);
                        }
                    }
                }
            }
        }
    }
    fs::write(directory.join("domain-enforcer.input"), cases.data()).unwrap();
}
