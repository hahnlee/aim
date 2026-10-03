//! Detached LegacyPermissionDataProvider projection for the C facade (#836).
//! Pinned android-16.0.0_r1 LegacyPermissionState, Copyright (C) The Android
//! Open Source Project, Apache License 2.0. The original permission service
//! owns flags/grants; access.abx and runtime-permissions.xml are not substitutes.
use aim_binder_host::parcel::{Exception, Parcel, Reader};
use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bridge;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permission {
    pub name: Option<String>,
    pub runtime: bool,
    pub granted: bool,
    pub flags: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub id: i32,
    pub missing: bool,
    pub permissions: Vec<Permission>,
}

/// Explicit users include pre-created users. Missing users and missing state
/// are different: an absent user is outside this projection's known inventory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    app_id: i32,
    users: Vec<User>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Input(String),
    Transport(i32),
    Owner(Exception),
}

pub fn validate(app_id: i32, users: &[i32]) -> Result<(), Error> {
    if !(0..100_000).contains(&app_id) || users.is_empty() {
        return Err(Error::Input(
            "permission capture requires app ID and resolved users".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    if users.iter().any(|user| *user < 0 || !seen.insert(*user)) {
        return Err(Error::Input("invalid permission user inventory".into()));
    }
    Ok(())
}

impl State {
    pub fn app_id(&self) -> i32 {
        self.app_id
    }
    pub fn users(&self) -> &[User] {
        &self.users
    }
    pub fn user(&self, user: i32) -> Option<&User> {
        self.users.iter().find(|entry| entry.id == user)
    }

    pub fn read(bytes: &[u8], app_id: i32, users: &[i32]) -> Result<Self, Error> {
        validate(app_id, users)?;
        let parse = || -> Result<Self, String> {
            let mut r = Reader::new(bytes, &[]);
            let mut int = || r.read_i32().map_err(|e| format!("permission capture: {e}"));
            if int()? != app_id || usize::try_from(int()?).ok() != Some(users.len()) {
                return Err("permission capture identity differs".into());
            }
            let mut states = Vec::with_capacity(users.len());
            for &user in users {
                if r.read_i32().map_err(|e| e.to_string())? != user {
                    return Err("permission user inventory differs".into());
                }
                let missing = boolean(&mut r)?;
                let count = r.read_i32().map_err(|e| e.to_string())?;
                if count < 0 || count as usize > r.remaining() / 16 {
                    return Err("invalid permission count".into());
                }
                let mut permissions = Vec::with_capacity(count as usize);
                let mut names = BTreeSet::new();
                let mut previous_hash = None;
                for _ in 0..count {
                    let name = r.read_string16().map_err(|e| e.to_string())?;
                    let hash = name
                        .as_deref()
                        .map(crate::package::parse::parcel::java_hash)
                        .unwrap_or(0);
                    if previous_hash.is_some_and(|previous| previous > hash) {
                        return Err("permission order differs from original ArrayMap".into());
                    }
                    previous_hash = Some(hash);
                    if !names.insert(name.clone()) {
                        return Err("duplicate permission name".into());
                    }
                    permissions.push(Permission {
                        name,
                        runtime: boolean(&mut r)?,
                        granted: boolean(&mut r)?,
                        flags: r.read_i32().map_err(|e| e.to_string())?,
                    });
                }
                states.push(User {
                    id: user,
                    missing,
                    permissions,
                });
            }
            if r.remaining() != 0 {
                return Err("permission capture has trailing data".into());
            }
            let state = Self {
                app_id,
                users: states,
            };
            if state.bytes() != bytes {
                return Err("noncanonical permission capture".into());
            }
            Ok(state)
        };
        parse().map_err(Error::Input)
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut p = Parcel::new();
        p.write_i32(self.app_id);
        p.write_i32(self.users.len() as i32);
        for user in &self.users {
            p.write_i32(user.id);
            p.write_bool(user.missing);
            p.write_i32(user.permissions.len() as i32);
            for permission in &user.permissions {
                p.write_string16(permission.name.as_deref());
                p.write_bool(permission.runtime);
                p.write_bool(permission.granted);
                p.write_i32(permission.flags);
            }
        }
        p.data().to_vec()
    }
}

fn boolean(r: &mut Reader<'_>) -> Result<bool, String> {
    match r.read_i32().map_err(|e| e.to_string())? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err("invalid permission boolean".into()),
    }
}

impl crate::package::bootstrap::Bridge {
    pub fn legacy_permissions(&self, app_id: i32, users: &[i32]) -> Result<State, Error> {
        validate(app_id, users)?;
        let mut request = Parcel::new();
        bridge::GetLegacyPermissionState {
            app_id,
            user_ids: Some(users.to_vec()),
        }
        .write(&mut request);
        let reply = self
            .owner
            .transact(bridge::GET_LEGACY_PERMISSION_STATE, &request, false)
            .map_err(Error::Transport)?;
        let mut reader = reply.reader();
        let bytes = bridge::read_get_legacy_permission_state_reply(&mut reader)
            .map_err(Error::Transport)?
            .map_err(Error::Owner)?
            .ok_or_else(|| Error::Input("original permission owner returned null state".into()))?;
        if reader.remaining() != 0 {
            return Err(Error::Input(
                "permission owner reply has trailing data".into(),
            ));
        }
        State::read(&bytes, app_id, users)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> State {
        State {
            app_id: 10042,
            users: vec![
                User {
                    id: 10,
                    missing: true,
                    permissions: vec![
                        Permission {
                            name: None,
                            runtime: false,
                            granted: false,
                            flags: i32::MIN,
                        },
                        Permission {
                            name: Some("".into()),
                            runtime: true,
                            granted: true,
                            flags: -1,
                        },
                        Permission {
                            name: Some("BB".into()),
                            runtime: true,
                            granted: false,
                            flags: 17,
                        },
                        Permission {
                            name: Some("Aa".into()),
                            runtime: false,
                            granted: true,
                            flags: i32::MAX,
                        },
                    ],
                },
                User {
                    id: 0,
                    missing: false,
                    permissions: vec![],
                },
            ],
        }
    }

    #[test]
    fn capture_retains_original_flags_order_nullable_names_and_missing_users() {
        let state = fixture();
        let mut bytes = state.bytes();
        let captured = State::read(&bytes, 10042, &[10, 0]).unwrap();
        assert_eq!(captured, state);
        assert!(captured.user(10).unwrap().missing);
        assert_eq!(captured.user(0).unwrap().permissions, vec![]);
        assert_eq!(captured.user(11), None);
        bytes.fill(0);
        let mut candidate = captured.clone();
        candidate.users[0].permissions.clear();
        assert_eq!(captured, state);
        assert_eq!(
            State::read(&captured.bytes(), 10042, &[10, 0]).unwrap(),
            state
        );
    }

    #[test]
    fn malformed_or_unresolved_captures_never_become_empty_state() {
        let good = fixture().bytes();
        for (app, users) in [
            (-1, vec![0]),
            (100000, vec![0]),
            (10042, vec![]),
            (10042, vec![-1]),
            (10042, vec![0, 0]),
            (10042, vec![0, 10]),
        ] {
            assert!(State::read(&good, app, &users).is_err());
        }
        for length in 0..good.len() {
            assert!(State::read(&good[..length], 10042, &[10, 0]).is_err());
        }
        let mut unordered = fixture();
        unordered.users[0].permissions.swap(0, 2);
        assert!(State::read(&unordered.bytes(), 10042, &[10, 0]).is_err());
        let mut noncanonical = good.clone();
        noncanonical[20..24].copy_from_slice(&(-2i32).to_le_bytes());
        assert!(State::read(&noncanonical, 10042, &[10, 0]).is_err());
        let mut extra = good.clone();
        extra.extend_from_slice(&0i32.to_le_bytes());
        assert!(State::read(&extra, 10042, &[10, 0]).is_err());
        let mut wrong = good.clone();
        wrong[12..16].copy_from_slice(&2i32.to_le_bytes());
        assert!(State::read(&wrong, 10042, &[10, 0]).is_err());
        wrong[16..20].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(State::read(&wrong, 10042, &[10, 0]).is_err());
        let mut duplicate = fixture();
        let first = duplicate.users[0].permissions[0].clone();
        duplicate.users[0].permissions.push(first);
        assert!(State::read(&duplicate.bytes(), 10042, &[10, 0]).is_err());
    }
}
