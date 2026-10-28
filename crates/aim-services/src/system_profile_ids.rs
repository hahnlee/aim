//! UserManager.getEnabledProfileIds at android-16.0.0_r1: cache by its
//! CachedProperty API's nonce, invalidated for user-list and FLAG_DISABLED
//! changes. The bridge supplies the same store apps receive (#432, #460).

use super::*;

const ENABLED_PROFILES_NONCE: &str = "user_manager_users_enabled";

#[derive(Default)]
pub(super) struct Profiles {
    source: Weak<NonceSource>,
    nonce: i64,
    users: HashMap<i32, Vec<i32>>,
}

impl System {
    /// `UserManager.getEnabledProfileIds(user)`, including hidden profiles.
    pub fn profile_ids(self: &Arc<Self>, user_id: i32) -> Result<Vec<i32>> {
        self.cached_profiles(user_id, || {
            let args = um::GetProfileIds {
                user_id,
                enabled_only: true,
            };
            self.call(
                "user",
                um::GET_PROFILE_IDS,
                |p| args.write(p),
                um::read_get_profile_ids_reply,
            )
            .map(Option::unwrap_or_default)
        })
    }

    fn cached_profiles(
        &self,
        user: i32,
        ask: impl FnOnce() -> Result<Vec<i32>>,
    ) -> Result<Vec<i32>> {
        let source = self.nonces.lock().unwrap().clone();
        let Some(source) = source else {
            return ask();
        };
        let Some(nonce) = source.nonces.get(ENABLED_PROFILES_NONCE) else {
            return ask();
        };
        let owner = Arc::downgrade(&source);
        {
            let mut kept = self.profiles.lock().unwrap();
            if kept.nonce != nonce || !kept.source.ptr_eq(&owner) {
                *kept = Profiles {
                    source: owner.clone(),
                    nonce,
                    users: HashMap::new(),
                };
            }
            if let Some(ids) = kept.users.get(&user) {
                return Ok(ids.clone());
            }
        }
        let ids = ask()?;
        let mut kept = self.profiles.lock().unwrap();
        if kept.nonce == nonce && kept.source.ptr_eq(&owner) {
            kept.users.insert(user, ids.clone());
        }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_driver::{Credentials, Device, Driver};
    use std::os::fd::AsFd;
    use std::os::unix::fs::FileExt;

    struct Fixture {
        driver: Arc<Driver>,
        system: Arc<System>,
    }

    impl Fixture {
        fn new() -> Self {
            let driver = Driver::new();
            let process = LocalProcess::open(
                &driver,
                Device::Binder,
                Credentials {
                    pid: 97002,
                    euid: 1000,
                    security_context: None,
                },
            );
            Self {
                driver,
                system: System::new(process, &[]),
            }
        }

        fn attach(&self, nonce: i64) -> std::fs::File {
            let file = crate::nonces::tests::named_nonce_file(ENABLED_PROFILES_NONCE, nonce);
            let retained = aim_binder_host::server::file_from_fd(file.as_fd()).unwrap();
            *self.system.nonces.lock().unwrap() = Some(Arc::new(NonceSource {
                bridge: Arc::new(self.system.process.strong(0)),
                nonces: Nonces::map(&retained).unwrap(),
            }));
            file
        }

        fn advance(&self, file: &std::fs::File, nonce: i64) {
            // NonceStore's first nonce follows the 24-byte header.
            file.write_all_at(&nonce.to_ne_bytes(), (8 + 512 * 4 + 8 + 24) as u64)
                .unwrap();
        }

        fn query(&self, user: i32, ids: &[i32]) -> Vec<i32> {
            self.system
                .cached_profiles(user, || Ok(ids.to_vec()))
                .unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.driver.release(self.system.process.proc_handle());
        }
    }

    #[test]
    fn enabled_profiles_follow_the_owner_nonce_and_user() {
        let f = Fixture::new();
        let file = f.attach(42);
        let mut result = f.query(0, &[0, 10]);
        result.clear();
        assert_eq!(f.query(0, &[]), [0, 10]);
        assert_eq!(f.query(11, &[11, 12]), [11, 12]);
        // User creation/removal and enabling/disabling invalidate this API.
        f.advance(&file, 43);
        assert_eq!(f.query(0, &[0]), [0]);
        assert!(f.query(11, &[]).is_empty());
        assert!(f.query(11, &[11]).is_empty());
    }

    #[test]
    fn enabled_profiles_bypass_without_a_nonce_and_retry_errors() {
        let f = Fixture::new();
        assert_eq!(f.query(0, &[0]), [0]);
        assert_eq!(f.query(0, &[0, 10]), [0, 10]);
        let file = f.attach(42);
        assert!(
            f.system
                .cached_profiles(0, || Err(Exception::security("denied")))
                .is_err()
        );
        assert_eq!(f.query(0, &[0]), [0]);
        for nonce in 0..=3 {
            f.advance(&file, nonce);
            assert_eq!(f.query(0, &[0, 10]), [0, 10]);
            assert_eq!(f.query(0, &[0]), [0]);
        }
        f.attach(42);
        assert_eq!(f.query(0, &[0, 11]), [0, 11]);
        f.system.nonces.lock().unwrap().take();
        assert_eq!(f.query(0, &[0]), [0]);
    }

    #[test]
    fn enabled_profiles_old_reply_cannot_fill_a_replacement_store() {
        let f = Fixture::new();
        f.attach(42);
        let old = f
            .system
            .cached_profiles(0, || {
                f.attach(42);
                assert_eq!(f.query(0, &[0, 11]), [0, 11]);
                Ok(vec![0, 10])
            })
            .unwrap();
        assert_eq!(old, [0, 10]);
        assert_eq!(f.query(0, &[]), [0, 11]);
        // A change during a miss is also seen before the next cache hit.
        let file = f.attach(43);
        f.system
            .cached_profiles(0, || {
                f.advance(&file, 44);
                Ok(vec![0, 10])
            })
            .unwrap();
        assert_eq!(f.query(0, &[0]), [0]);
    }
}
