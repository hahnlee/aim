//! CE storage protection by the keychain's secret (ADR 0016), as
//! LockSettingsService protects and unlocks an unsecured user's.
use super::*;

/// The pinned `IVold.setCeStorageProtection`, `unlockCeStorage`.
const SET_CE_STORAGE_PROTECTION: u32 = 44;
const UNLOCK_CE_STORAGE: u32 = 46;

/// The secrets, by account, in place of the Mac user's keychain.
#[derive(Clone, Default)]
struct Memory(Arc<Mutex<HashMap<String, Vec<u8>>>>);

impl CeSecrets for Memory {
    fn read(&self, account: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.lock().unwrap().get(account).cloned())
    }

    fn create(&self, account: &str, secret: &[u8]) -> Result<(), String> {
        let previous = self
            .0
            .lock()
            .unwrap()
            .insert(account.into(), secret.to_vec());
        assert!(previous.is_none(), "{account} has a secret");
        Ok(())
    }
}

/// vold's CE calls: their code, user and secret.
struct Vold(Arc<Mutex<Vec<(u32, i32, Vec<u8>)>>>);

impl Service for Vold {
    fn descriptor(&self) -> &str {
        super::super::storage::VOLD_DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        call.data.enforce_interface(self.descriptor())?;
        let user = call.data.read_i32()?;
        let secret = aim_service_aidl::read_byte_array(&mut call.data)?.unwrap();
        assert_eq!(call.data.remaining(), 0);
        self.0.lock().unwrap().push((call.code, user, secret));
        let mut reply = Parcel::new();
        reply.write_no_exception();
        Ok(reply)
    }
}

/// The first boot makes user 0's secret under a new protector and
/// protects the CE key with it once (`initializeSyntheticPassword`); an
/// interrupted migration protects it again with the same secret; each
/// unlock gives vold that secret, and none is given without it.
#[test]
fn the_ce_key_is_protected_once_and_unlocked_by_the_keychain_secret() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let secrets = Memory::default();
        host.set_ce_secrets(Box::new(secrets.clone()));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vold = export(Arc::new(Vold(calls.clone())));
        let settings = LockSettings::new(host.clone());
        assert!(host.unlock_user_key_if_unsecured(&vold, 0).is_err());

        host.protect_ce_storage(&vold, 0).unwrap();
        let protector = settings.long(CURRENT_PROTECTOR_ID_KEY, 0, 0).unwrap();
        assert_ne!(protector, NULL_PROTECTOR_ID);
        assert_eq!(settings.long(PREV_PROTECTOR_ID_KEY, -1, 0).unwrap(), 0);
        assert!(settings.long(LSKF_LAST_CHANGED_TIME_KEY, 0, 0).unwrap() > 0);
        assert!(settings.boolean(MIGRATED_SP_FULL, false, 0).unwrap());
        let account = format!("user 0 protector {protector:016x}");
        let secret = secrets.0.lock().unwrap()[&account].clone();
        assert_eq!(secret.len(), CE_SECRET_LEN);
        assert_eq!(
            *calls.lock().unwrap(),
            [(SET_CE_STORAGE_PROTECTION, 0, secret.clone())]
        );

        // Migrated: nothing again.
        host.protect_ce_storage(&vold, 0).unwrap();
        assert_eq!(calls.lock().unwrap().len(), 1);
        // Interrupted before the migration was recorded: the same secret.
        settings.set_boolean(MIGRATED_SP_FULL, false, 0).unwrap();
        host.protect_ce_storage(&vold, 0).unwrap();
        assert_eq!(secrets.0.lock().unwrap().len(), 1);

        host.unlock_user_key_if_unsecured(&vold, 0).unwrap();
        assert_eq!(
            calls.lock().unwrap()[1..],
            [
                (SET_CE_STORAGE_PROTECTION, 0, secret.clone()),
                (UNLOCK_CE_STORAGE, 0, secret)
            ]
        );

        // The keychain without the secret (another data directory's
        // keychain, or a locked one) unlocks nothing.
        secrets.0.lock().unwrap().clear();
        let error = host.unlock_user_key_if_unsecured(&vold, 0).unwrap_err();
        assert!(error.contains(&account), "{error}");
        assert_eq!(calls.lock().unwrap().len(), 3);
    });
}
