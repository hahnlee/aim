//! `user`: UserManager for the app host, which runs user 0 alone,
//! unlocked and visible, with the restrictions UserManagerService last
//! persisted for it (`/data/system/users/0.xml`'s `<restrictions>`).

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::android_os_iusermanager as um;

use super::{AppHost, reply};

/// The only user.
const SYSTEM_USER: i32 = 0;

pub struct UserManager {
    host: Arc<AppHost>,
    restrictions: OnceLock<BTreeSet<String>>,
}

impl UserManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        UserManager { host, restrictions: OnceLock::new() }
    }

    /// User 0's base restrictions.
    fn restrictions(&self) -> Result<&BTreeSet<String>, Exception> {
        if let Some(r) = self.restrictions.get() {
            return Ok(r);
        }
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let path = self.host.data()?.join("system/users/0.xml");
        let bytes = std::fs::read(&path).map_err(|e| state(format!("{}: {e}", path.display())))?;
        let root = aim_android_xml::read(&bytes).map_err(|e| state(format!("{}: {e}", path.display())))?;
        let restrictions = root
            .children()
            .find(|e| e.name == "restrictions")
            .map(|r| r.attrs.iter().filter(|(_, v)| matches!(v, aim_android_xml::Value::Bool(true))).map(|(k, _)| k.clone()).collect())
            .unwrap_or_default();
        Ok(self.restrictions.get_or_init(|| restrictions))
    }
}

impl Service for UserManager {
    fn descriptor(&self) -> &str {
        um::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            um::IS_USER_UNLOCKING_OR_UNLOCKED => {
                let args = um::IsUserUnlockingOrUnlocked::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                um::write_is_user_unlocking_or_unlocked_reply(&mut p, args.user_id == SYSTEM_USER);
            }
            um::IS_USER_RUNNING => {
                let args = um::IsUserRunning::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                um::write_is_user_running_reply(&mut p, args.user_id == SYSTEM_USER);
            }
            um::IS_USER_VISIBLE => {
                let args = um::IsUserVisible::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                um::write_is_user_visible_reply(&mut p, args.user_id == SYSTEM_USER);
            }
            um::GET_PROFILE_IDS => {
                let args = um::GetProfileIds::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let ids = (args.user_id == SYSTEM_USER).then(|| vec![SYSTEM_USER]).unwrap_or_default();
                um::write_get_profile_ids_reply(&mut p, &Some(ids));
            }
            um::GET_CREDENTIAL_OWNER_PROFILE => {
                let args = um::GetCredentialOwnerProfile::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                um::write_get_credential_owner_profile_reply(&mut p, args.user_id);
            }
            um::HAS_USER_RESTRICTION => {
                let args = um::HasUserRestriction::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let restricted = match self.restrictions() {
                    Ok(r) => args.user_id == SYSTEM_USER && args.restriction_key.is_some_and(|k| r.contains(&k)),
                    Err(e) => return reply(Err(e)),
                };
                um::write_has_user_restriction_reply(&mut p, restricted);
            }
            code => return reply(Err(self.host.unimplemented("user", code))),
        }
        Ok(p)
    }
}
