//! `grammatical_inflection`: GrammaticalInflectionService for the app host
//! (`android-16.0.0_r1`): the system's grammatical gender of a user, as
//! `onUserUnlocked` reads it from `system_ce/<user>/grammatical_inflection/user_settings.xml`
//! (here when it is first asked), to a caller holding
//! `READ_SYSTEM_GRAMMATICAL_GENDER` (`checkPermissionForDataDelivery`)
//! while the `system_terms_of_address_enabled` flag is on. A persisted
//! gender, which the original applies to the global configuration, and
//! the writes are not implemented (#1758).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::android_app_igrammaticalinflectionmanager as gi;

use super::permission_checker::{self, AttributionSourceState};
use super::{AppHost, reply};

/// `android.app.Flags.systemTermsOfAddressEnabled`.
const SYSTEM_TERMS_OF_ADDRESS_ENABLED: &str = "android.app.system_terms_of_address_enabled";
const READ_SYSTEM_GRAMMATICAL_GENDER: &str = "android.permission.READ_SYSTEM_GRAMMATICAL_GENDER";
const CHANGE_CONFIGURATION: &str = "android.permission.CHANGE_CONFIGURATION";
/// `Configuration.GRAMMATICAL_GENDER_NOT_SPECIFIED`.
const NOT_SPECIFIED: i32 = 0;
/// `GrammaticalInflectionManager.VALID_GRAMMATICAL_GENDER_VALUES`.
const VALID_GENDERS: [i32; 4] = [NOT_SPECIFIED, 1, 2, 3];
const TAG_GRAMMATICAL_INFLECTION: &str = "grammatical_inflection";
const ATTR_NAME: &str = "grammatical_gender";

/// `getGrammaticalGenderFromXml`: the attribute of the first
/// `<grammatical_inflection>`, or not specified without one.
fn gender_from_xml(element: &aim_android_xml::Element) -> Result<Option<i32>, String> {
    if element.name == TAG_GRAMMATICAL_INFLECTION {
        return element
            .int(ATTR_NAME)?
            .map(Some)
            .ok_or_else(|| format!("missing {ATTR_NAME}"));
    }
    for child in element.children() {
        if let Some(gender) = gender_from_xml(child)? {
            return Ok(Some(gender));
        }
    }
    Ok(None)
}

pub struct GrammaticalInflection {
    host: Arc<AppHost>,
    /// `mGrammaticalGenderCache`: each user's gender read from its file.
    cache: Mutex<HashMap<i32, i32>>,
}

impl GrammaticalInflection {
    pub fn new(host: Arc<AppHost>) -> Self {
        GrammaticalInflection {
            host,
            cache: Default::default(),
        }
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.host
            .unimplemented_named("grammatical_inflection", &format!("{what} (#1758)"))
    }

    /// `checkSystemTermsOfAddressIsEnabled`.
    fn enabled(&self) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state
            .system
            .flags
            .iter()
            .any(|(n, on)| n == SYSTEM_TERMS_OF_ADDRESS_ENABLED && *on))
    }

    /// `checkSystemGrammaticalGenderPermission`.
    fn permitted(&self, source: &AttributionSourceState) -> Result<bool, Exception> {
        permission_checker::data_delivery(&self.host, READ_SYSTEM_GRAMMATICAL_GENDER, source)
    }

    /// `getSystemGrammaticalGender(userId)`: the user's file read once, as
    /// `onUserUnlocked` reads it; one unreadable is logged and not kept.
    fn system_gender(&self, user: i32) -> Result<i32, Exception> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(&gender) = cache.get(&user) {
            return Ok(gender);
        }
        let file = self.host.data()?.join(format!(
            "system_ce/{user}/{TAG_GRAMMATICAL_INFLECTION}/user_settings.xml"
        ));
        let read = match super::device_policy::read_atomic_bytes(&file) {
            Ok(None) => return Ok(NOT_SPECIFIED),
            Ok(Some(bytes)) => {
                aim_android_xml::read_next(&bytes).and_then(|root| gender_from_xml(&root))
            }
            Err(e) => Err(e),
        };
        let gender = match read {
            Ok(gender) => gender.unwrap_or(NOT_SPECIFIED),
            Err(e) => {
                eprintln!(
                    "app host: grammatical_inflection: Failed to parse XML configuration from {}: {e}",
                    file.display()
                );
                return Ok(NOT_SPECIFIED);
            }
        };
        if gender != NOT_SPECIFIED {
            return Err(self.unimplemented("a persisted system grammatical gender"));
        }
        cache.insert(user, gender);
        Ok(gender)
    }

    /// `enforceCallerPermissions`.
    fn enforce_caller(&self, uid: i32) -> Result<(), Exception> {
        if !matches!(uid, 0 | 1000 | 2000)
            && self
                .host
                .check_permission(Some(CHANGE_CONFIGURATION), uid)?
                != 0
        {
            return Err(Exception::security(format!(
                "Caller must be system, shell, root or hold CHANGE_CONFIGURATION permission.: Neither user {uid} nor current process has {CHANGE_CONFIGURATION}."
            )));
        }
        Ok(())
    }
}

impl Service for GrammaticalInflection {
    fn descriptor(&self) -> &str {
        gi::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            gi::GET_SYSTEM_GRAMMATICAL_GENDER => {
                let a =
                    gi::GetSystemGrammaticalGender::<AttributionSourceState>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                (|| {
                    let source = a
                        .attribution_source
                        .as_ref()
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "attributionSource"))?;
                    if !self.permitted(source)? {
                        return Err(Exception::security(format!(
                            "AttributionSource: AttributionSource {{ uid = {}, packageName = {} }} does not have READ_SYSTEM_GRAMMATICAL_GENDER permission.",
                            source.uid,
                            source.package.as_deref().unwrap_or("null")
                        )));
                    }
                    let gender = if self.enabled()? {
                        self.system_gender(a.user_id)?
                    } else {
                        NOT_SPECIFIED
                    };
                    gi::write_get_system_grammatical_gender_reply(&mut p, gender);
                    Ok(())
                })()
            }
            gi::PEEK_SYSTEM_GRAMMATICAL_GENDER_BY_USER_ID => {
                let a = gi::PeekSystemGrammaticalGenderByUserId::<AttributionSourceState>::read(
                    &mut call.data,
                )
                .map_err(|_| BAD_VALUE)?;
                (|| {
                    let source = a
                        .attribution_source
                        .as_ref()
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "attributionSource"))?;
                    let gender = if self.enabled()? && self.permitted(source)? {
                        self.system_gender(a.user_id)?
                    } else {
                        NOT_SPECIFIED
                    };
                    gi::write_peek_system_grammatical_gender_by_user_id_reply(&mut p, gender);
                    Ok(())
                })()
            }
            gi::SET_SYSTEM_WIDE_GRAMMATICAL_GENDER => {
                let a = gi::SetSystemWideGrammaticalGender::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    self.enforce_caller(uid)?;
                    if !self.enabled()? {
                        gi::write_set_system_wide_grammatical_gender_reply(&mut p);
                        return Ok(());
                    }
                    if !VALID_GENDERS.contains(&a.gender) {
                        return Err(Exception::illegal_argument("Unknown grammatical gender"));
                    }
                    Err(self.unimplemented("setSystemWideGrammaticalGender"))
                })()
            }
            code => {
                let name = gi::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self.unimplemented(name))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `getGrammaticalGenderFromXml`: the first `<grammatical_inflection>`'s
    /// attribute, not specified without one, an error without the attribute.
    #[test]
    fn genders_are_read_as_the_service_parses_them() {
        let gender =
            |xml: &str| gender_from_xml(&aim_android_xml::read_next(xml.as_bytes()).unwrap());
        assert_eq!(
            gender(r#"<grammatical_inflection grammatical_gender="2" />"#),
            Ok(Some(2))
        );
        assert_eq!(
            gender(r#"<a><b /><grammatical_inflection grammatical_gender="3" /></a>"#),
            Ok(Some(3))
        );
        assert_eq!(gender("<a />"), Ok(None));
        assert!(gender("<grammatical_inflection />").is_err());
    }
}
