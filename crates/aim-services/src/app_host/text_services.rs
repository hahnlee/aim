//! `textservices`: TextServicesManagerService for the app host
//! (`android-16.0.0_r1`): a user's spell checkers, the services for
//! `SpellCheckerService.SERVICE_INTERFACE` that require
//! `BIND_TEXT_SERVICE`, each described by the `<spell-checker>` its
//! `android.view.textservice.scs` meta-data names, with its subtypes; the
//! selected one and its subtype in the user's `Settings.Secure`
//! (`selected_spell_checker`, `selected_spell_checker_subtype`,
//! `spell_checker_enabled`), a system spell checker selected for the
//! device's locale when none is (`findAvailSystemSpellCheckerLocked`);
//! and the sessions of `getSpellCheckerService`: the spell checker bound
//! as the user (`BIND_AUTO_CREATE | BIND_IMPORTANT_BACKGROUND`), each
//! request's session asked of it once connected and handed to the
//! client's listener, the binding dropped with its last client. A user's
//! data is made as it unlocks (`onUnlockUser`), or when first asked for
//! once it is unlocking. Its package monitor (`TextServicesMonitor`, of
//! all users) finds a user's spell checkers again when a package changes,
//! and selects a system one if none is selected.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::com_android_internal_textservice_ispellcheckerservice as scs;
use aim_service_aidl::com_android_internal_textservice_ispellcheckerservicecallback as scs_callback;
use aim_service_aidl::com_android_internal_textservice_itextservicesmanager as tsm;
use aim_service_aidl::com_android_internal_textservice_itextservicessessionlistener as ts_listener;

use super::kept::{KeptBundle, RawBundle};
use super::package_writes::{PackageChanged, PackageMonitor};
use super::services::HostConnection;
use super::settings_provider::{Caller, SettingsProvider};
use super::{AppHost, PER_USER_RANGE, reply};
use crate::bundle::Value;
use crate::package::component_resolver::{Info, ResolveInfo};
use crate::package::info::flags::GET_META_DATA;
use crate::package::intent::Intent;
use crate::package::pkg::Value as MetaValue;

/// `SpellCheckerService.SERVICE_INTERFACE`.
const SERVICE_INTERFACE: &str = "android.service.textservice.SpellCheckerService";
/// `SpellCheckerSession.SERVICE_META_DATA`.
const SERVICE_META_DATA: &str = "android.view.textservice.scs";
const BIND_TEXT_SERVICE: &str = "android.permission.BIND_TEXT_SERVICE";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
/// The `Settings.Secure` keys.
const SELECTED_SPELL_CHECKER: &str = "selected_spell_checker";
const SELECTED_SPELL_CHECKER_SUBTYPE: &str = "selected_spell_checker_subtype";
const SPELL_CHECKER_ENABLED: &str = "spell_checker_enabled";
/// `SpellCheckerSubtype.SUBTYPE_ID_NONE`.
const SUBTYPE_ID_NONE: i32 = 0;
/// `Context.BIND_AUTO_CREATE`, `BIND_IMPORTANT_BACKGROUND`.
const BIND_AUTO_CREATE: i64 = 0x1;
const BIND_IMPORTANT_BACKGROUND: i64 = 0x80_0000;
/// `ApplicationInfo.FLAG_SYSTEM`.
const FLAG_SYSTEM: i32 = 1;
const SYSTEM_UID: i32 = 1000;
/// `UserHandle.USER_ALL`.
const USER_ALL: i32 = -1;

/// `java.util.Locale`, as far as spell checkers compare them: its
/// language, script, country, variant and extensions.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Locale {
    pub language: String,
    pub script: String,
    pub country: String,
    pub variant: String,
    pub extensions: String,
}

impl Locale {
    /// `new Locale(language, country, variant)`: the language in lower
    /// case, the country in upper case.
    pub fn new(language: &str, country: &str, variant: &str) -> Locale {
        Locale {
            language: language.to_ascii_lowercase(),
            country: country.to_ascii_uppercase(),
            variant: variant.into(),
            ..Default::default()
        }
    }

    /// `Locale.forLanguageTag`: the well-formed subtags of `tag`, up to
    /// the first that is not; `und` is no language.
    pub fn for_language_tag(tag: &str) -> Locale {
        let mut locale = Locale::default();
        let mut parts = tag.split(['-', '_']).peekable();
        let alpha = |s: &str| s.chars().all(|c| c.is_ascii_alphabetic());
        let alnum = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric());
        match parts.peek() {
            Some(l) if (2..=8).contains(&l.len()) && alpha(l) => {
                let l = parts.next().unwrap().to_ascii_lowercase();
                locale.language = if l == "und" { String::new() } else { l };
            }
            _ => return locale,
        }
        // Extended language subtags are dropped (`LanguageTag.parse`).
        while parts.peek().is_some_and(|e| e.len() == 3 && alpha(e)) {
            parts.next();
        }
        if let Some(s) = parts.next_if(|s| s.len() == 4 && alpha(s)) {
            locale.script = format!(
                "{}{}",
                s[..1].to_ascii_uppercase(),
                s[1..].to_ascii_lowercase()
            );
        }
        if let Some(r) = parts.next_if(|r| {
            (r.len() == 2 && alpha(r)) || (r.len() == 3 && r.chars().all(|c| c.is_ascii_digit()))
        }) {
            locale.country = r.to_ascii_uppercase();
        }
        let mut variants = Vec::new();
        while let Some(v) = parts.next_if(|v| {
            alnum(v)
                && ((5..=8).contains(&v.len())
                    || (v.len() == 4 && v.as_bytes()[0].is_ascii_digit()))
        }) {
            variants.push(v.to_string());
        }
        locale.variant = variants.join("_");
        let rest: Vec<&str> = parts.collect();
        if rest.first().is_some_and(|s| s.len() == 1 && alnum(s)) {
            locale.extensions = rest.join("-").to_ascii_lowercase();
        }
        locale
    }

    /// `SubtypeLocaleUtils.constructLocaleFromString`: the locale a
    /// subtype's `subtypeLocale` names (`tl` is `fil`), none for an empty
    /// one.
    pub fn from_subtype_string(s: &str) -> Option<Locale> {
        if s.is_empty() {
            return None;
        }
        let mut params: Vec<&str> = s.splitn(3, '_').collect();
        if params[0] == "tl" {
            params[0] = "fil";
        }
        Some(match params[..] {
            [l] => Locale::new(l, "", ""),
            [l, c] => Locale::new(l, c, ""),
            [l, c, v] => Locale::new(l, c, v),
            _ => return None,
        })
    }

    /// The device's locale, the configuration's first
    /// (`persist.sys.locale`, a list of language tags).
    pub fn of_device(locales: &str) -> Option<Locale> {
        locales.split(',').next().map(Locale::for_language_tag)
    }
}

/// `LocaleUtils.getSuitableLocalesForSpellChecker`: the locales to look
/// for a system spell checker in, in order.
fn suitable_locales(system: Option<&Locale>) -> Vec<Locale> {
    let (lcv, lc, l) = match system {
        Some(s) => {
            let (has_l, has_c, has_v) = (
                !s.language.is_empty(),
                !s.country.is_empty(),
                !s.variant.is_empty(),
            );
            (
                (has_l && has_c && has_v).then(|| Locale::new(&s.language, &s.country, &s.variant)),
                (has_l && has_c).then(|| Locale::new(&s.language, &s.country, "")),
                has_l.then(|| Locale::new(&s.language, "", "")),
            )
        }
        None => (None, None, None),
    };
    let (english, us, uk) = (
        Locale::new("en", "", ""),
        Locale::new("en", "US", ""),
        Locale::new("en", "GB", ""),
    );
    let mut locales: Vec<Locale> = lcv.into_iter().collect();
    if l.as_ref() == Some(&english) {
        match lc {
            Some(lc) => {
                let (not_us, not_uk) = (lc != us, lc != uk);
                locales.push(lc);
                if not_us {
                    locales.push(us);
                }
                if not_uk {
                    locales.push(uk);
                }
                locales.push(english);
            }
            None => locales.extend([english, us, uk]),
        }
    } else {
        locales.extend(lc);
        locales.extend(l);
        locales.extend([us, uk, english]);
    }
    locales
}

/// `String.hashCode`.
fn java_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

/// `SpellCheckerSubtype`.
#[derive(Clone, Debug, PartialEq)]
struct Subtype {
    name_res: i32,
    locale: String,
    language_tag: String,
    extra_value: String,
    id: i32,
}

impl Subtype {
    /// `hashCode`: its id, else `Arrays.hashCode({locale, extraValue})`.
    fn hash_code(&self) -> i32 {
        if self.id != SUBTYPE_ID_NONE {
            return self.id;
        }
        let h = 31i32.wrapping_add(java_hash(&self.locale));
        h.wrapping_mul(31)
            .wrapping_add(java_hash(&self.extra_value))
    }

    /// `getLocaleObject`.
    fn locale_object(&self) -> Option<Locale> {
        if !self.language_tag.is_empty() {
            return Some(Locale::for_language_tag(&self.language_tag));
        }
        Locale::from_subtype_string(&self.locale)
    }
}

impl WriteParcelable for Subtype {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.name_res);
        p.write_string16(Some(&self.locale));
        p.write_string16(Some(&self.language_tag));
        p.write_string16(Some(&self.extra_value));
        p.write_i32(self.id);
    }
}

/// `SpellCheckerInfo`.
#[derive(Clone, Debug)]
struct Checker {
    id: String,
    label: i32,
    settings_activity: Option<String>,
    service: ResolveInfo,
    subtypes: Vec<Subtype>,
    package: String,
    class: String,
    system: bool,
}

impl WriteParcelable for Checker {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.label);
        p.write_string16(Some(&self.id));
        p.write_string16(self.settings_activity.as_deref());
        self.service.write_to(p);
        // `writeTypedList`.
        p.write_i32(self.subtypes.len() as i32);
        for s in &self.subtypes {
            p.write_i32(1);
            s.write_to(p);
        }
    }
}

/// `SpellCheckerInfo(context, service)`'s parse of the `<spell-checker>`
/// element `e` and its subtypes.
fn parse_checker(
    e: &aim_apps::res::Element,
    ctx: &crate::package::parse::attrs::Ctx<'_>,
) -> Result<(i32, Option<String>, Vec<Subtype>), String> {
    if e.name != "spell-checker" {
        return Err("Meta-data does not start with spell-checker tag".into());
    }
    let a = ctx.obtain(e);
    let label = a.resource_id("label", 0);
    let settings_activity = a.string("settingsActivity");
    // Every element below it, in document order, is a subtype.
    fn descendants<'e>(e: &'e aim_apps::res::Element, out: &mut Vec<&'e aim_apps::res::Element>) {
        for c in &e.children {
            out.push(c);
            descendants(c, out);
        }
    }
    let mut elements = Vec::new();
    descendants(e, &mut elements);
    let mut subtypes = Vec::new();
    for s in elements {
        if s.name != "subtype" {
            return Err("Meta-data in spell-checker does not start with subtype tag".into());
        }
        let a = ctx.obtain(s);
        subtypes.push(Subtype {
            name_res: a.resource_id("label", 0),
            locale: a.string("subtypeLocale").unwrap_or_default(),
            language_tag: a.string("languageTag").unwrap_or_default(),
            extra_value: a.string("subtypeExtraValue").unwrap_or_default(),
            id: a.int("subtypeId", 0),
        });
    }
    Ok((label, settings_activity, subtypes))
}

/// `SessionRequest`.
struct Request {
    locale: Option<String>,
    ts_listener: Arc<Strong>,
    sc_listener: Arc<Strong>,
    bundle: Option<KeptBundle>,
    supported_attributes: i32,
}

/// A client's `ISpellCheckerSessionListener` in a group's
/// `RemoteCallbackList`, and its death notification.
struct Listener {
    strong: Arc<Strong>,
    death: u64,
}

/// `SpellCheckerBindGroup`.
struct Group {
    connection: Arc<Connection>,
    unbind_called: bool,
    spell_checker: Option<Strong>,
    connected: bool,
    pending: Vec<Arc<Request>>,
    ongoing: Vec<Arc<Request>>,
    listeners: Vec<Listener>,
}

/// `TextServicesData`: a user's spell checkers, in the order found, and
/// the bind groups by spell checker id.
#[derive(Default)]
struct UserData {
    checkers: Vec<Arc<Checker>>,
    groups: HashMap<String, Group>,
}

pub struct TextServices {
    host: Arc<AppHost>,
    settings: Arc<SettingsProvider>,
    /// `mUserData`, under `mLock`.
    users: Mutex<HashMap<i32, UserData>>,
    /// The main thread the service connections are called on.
    handler: Mutex<mpsc::Sender<Box<dyn FnOnce() + Send>>>,
    me: Weak<TextServices>,
}

impl TextServices {
    pub fn new(host: Arc<AppHost>, settings: Arc<SettingsProvider>) -> Arc<TextServices> {
        let (sender, jobs) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
        std::thread::Builder::new()
            .name("textservices".into())
            .spawn(move || jobs.into_iter().for_each(|job| job()))
            .expect("the textservices thread");
        let service = Arc::new_cyclic(|me| TextServices {
            host,
            settings,
            users: Mutex::default(),
            handler: Mutex::new(sender),
            me: me.clone(),
        });
        // `mMonitor.register(context, null, UserHandle.ALL, true)`.
        let monitor = service.clone() as Arc<dyn PackageMonitor>;
        service
            .host
            .package_monitors
            .register(USER_ALL, Arc::downgrade(&monitor));
        service
    }

    /// `TextServicesMonitor.onSomePackagesChanged`: the user's spell
    /// checkers found again (`initializeTextServicesData`) and, while
    /// enabled, a system one selected if none was. A changed package is not
    /// disappearing (`isPackageDisappearing`), so a selected one stays.
    fn packages_changed(&self, user: i32) -> Result<(), Exception> {
        let mut users = self.users.lock().unwrap();
        let Some(data) = users.get_mut(&user) else {
            return Ok(());
        };
        let selected = self.current(data, user)?;
        data.checkers = self.find_checkers(user)?;
        if !self.enabled(user)? {
            return Ok(());
        }
        if selected.is_none() {
            let sci = self.avail_system_checker(&data.checkers);
            self.set_current(sci.as_deref(), user)?;
        }
        Ok(())
    }

    fn post(&self, job: impl FnOnce() + Send + 'static) {
        if self.handler.lock().unwrap().send(Box::new(job)).is_err() {
            eprintln!("app host: textservices: the handler stopped");
        }
    }

    // `Settings.Secure` of a user, as the system server's resolver reads
    // and writes it.

    fn get_secure(&self, name: &str, user: i32) -> Result<Option<String>, Exception> {
        let caller = Caller {
            uid: SYSTEM_UID,
            package: Some("android".into()),
        };
        let extras = HashMap::from([("_user".to_string(), Value::Int(user))]);
        let got = self
            .settings
            .call(&caller, "GET_secure", Some(name), &extras, None)?;
        Ok(match got.as_deref() {
            Some([(_, Value::String(value)), ..]) => value.clone(),
            _ => None,
        })
    }

    fn put_secure(&self, name: &str, value: &str, user: i32) -> Result<(), Exception> {
        let caller = Caller {
            uid: SYSTEM_UID,
            package: Some("android".into()),
        };
        let extras = HashMap::from([
            ("_user".to_string(), Value::Int(user)),
            ("value".to_string(), Value::String(Some(value.to_string()))),
        ]);
        self.settings
            .call(&caller, "PUT_secure", Some(name), &extras, None)
            .map(|_| ())
    }

    /// `getIntForUser`: the default for an absent or unparsable value.
    fn secure_int(&self, name: &str, default: i32, user: i32) -> Result<i32, Exception> {
        Ok(self
            .get_secure(name, user)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(default))
    }

    /// `isSpellCheckerEnabled`.
    fn enabled(&self, user: i32) -> Result<bool, Exception> {
        Ok(self.secure_int(SPELL_CHECKER_ENABLED, 1, user)? == 1)
    }

    /// `getCurrentSpellChecker`: the selected id's spell checker.
    fn current(&self, data: &UserData, user: i32) -> Result<Option<Arc<Checker>>, Exception> {
        let id = self
            .get_secure(SELECTED_SPELL_CHECKER, user)?
            .unwrap_or_default();
        if id.is_empty() {
            return Ok(None);
        }
        Ok(data.checkers.iter().find(|c| c.id == id).cloned())
    }

    /// `setCurrentSpellChecker`, as the system.
    fn set_current(&self, checker: Option<&Checker>, user: i32) -> Result<(), Exception> {
        let id = checker.map_or("", |c| &c.id);
        self.put_secure(SELECTED_SPELL_CHECKER, id, user)?;
        self.put_secure(
            SELECTED_SPELL_CHECKER_SUBTYPE,
            &SUBTYPE_ID_NONE.to_string(),
            user,
        )
    }

    /// The device's locale (`getResources().getConfiguration().locale`).
    fn system_locale(&self) -> Option<Locale> {
        Locale::of_device(&self.host.property("persist.sys.locale")?)
    }

    /// `initializeTextServicesData`: the user's spell checkers.
    fn find_checkers(&self, user: i32) -> Result<Vec<Arc<Checker>>, Exception> {
        let (state, resolution) = self.host.live_packages()?;
        let intent = Intent {
            action: Some(SERVICE_INTERFACE.into()),
            ..Default::default()
        };
        let found = resolution
            .query_intent_services(&intent, None, GET_META_DATA, user, SYSTEM_UID)
            .map_err(|e| match e {
                crate::package::resolve::ResolutionError::Original(e) => e,
                e => Exception::new(
                    aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,
                    format!("queryIntentServicesAsUser: {e:?}"),
                ),
            })?;
        let inputs = self.host.platform()?;
        let root = self.host.root()?;
        let mut checkers = Vec::new();
        for r in found {
            let Info::Service(si) = &r.info else { continue };
            let package = si.info.item.package_name.clone().unwrap_or_default();
            let class = si.info.item.name.clone().unwrap_or_default();
            let name = super::services::short_name(&package, &class);
            if si.permission.as_deref() != Some(BIND_TEXT_SERVICE) {
                eprintln!(
                    "app host: textservices: Skipping text service ComponentInfo{{{package}/{class}}}: it does not require the permission {BIND_TEXT_SERVICE}"
                );
                continue;
            }
            let id = si
                .info
                .item
                .meta_data
                .as_ref()
                .and_then(|m| m.0.iter().find(|(k, _)| k == SERVICE_META_DATA))
                .and_then(|(_, v)| match v {
                    MetaValue::Int(id) => Some(*id as u32),
                    _ => None,
                })
                .filter(|id| *id != 0);
            let pkg = state.packages.get(&package).and_then(|ps| ps.pkg.clone());
            let parsed = match (id, pkg) {
                (Some(id), Some(pkg)) => self
                    .host
                    .framework()?
                    .xml_resource(root, &inputs.data, &pkg, id, parse_checker)
                    .and_then(|r| r),
                (None, _) => Err(format!("No {SERVICE_META_DATA} meta-data")),
                (_, None) => Err(format!("Unable to create context for: {package}")),
            };
            match parsed {
                Ok((label, settings_activity, subtypes)) if !subtypes.is_empty() => {
                    checkers.push(Arc::new(Checker {
                        id: name,
                        label,
                        settings_activity,
                        system: si.info.application_info.flags & FLAG_SYSTEM != 0,
                        service: r.clone(),
                        subtypes,
                        package,
                        class,
                    }))
                }
                Ok(_) => eprintln!(
                    "app host: textservices: Skipping text service ComponentInfo{{{package}/{class}}}: it does not contain subtypes."
                ),
                Err(e) => eprintln!(
                    "app host: textservices: Unable to load the spell checker ComponentInfo{{{package}/{class}}}: {e}"
                ),
            }
        }
        Ok(checkers)
    }

    /// `findAvailSystemSpellCheckerLocked(null, tsd)`: a system spell
    /// checker with a subtype of the first suitable locale, else the first.
    fn avail_system_checker(&self, checkers: &[Arc<Checker>]) -> Option<Arc<Checker>> {
        let system: Vec<&Arc<Checker>> = checkers.iter().filter(|c| c.system).collect();
        if system.is_empty() {
            eprintln!("app host: textservices: no available spell checker services found");
            return None;
        }
        for locale in suitable_locales(self.system_locale().as_ref()) {
            for c in &system {
                if c.subtypes
                    .iter()
                    .any(|s| Locale::from_subtype_string(&s.locale).as_ref() == Some(&locale))
                {
                    return Some((*c).clone());
                }
            }
        }
        if system.len() > 1 {
            eprintln!(
                "app host: textservices: more than one spell checker service found, picking first"
            );
        }
        Some(system[0].clone())
    }

    /// `getDataFromCallingUserIdLocked`, the user's data made as
    /// `onUnlockUser`'s `initializeInternalStateLocked` makes it once it is
    /// unlocking; none before.
    fn with_data<T>(
        &self,
        user: i32,
        f: impl FnOnce(&mut UserData) -> Result<T, Exception>,
    ) -> Result<Option<T>, Exception> {
        let mut users = self.users.lock().unwrap();
        let data = match users.entry(user) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                if !self.host.user_unlocking_or_unlocked(user)? {
                    return Ok(None);
                }
                let data = UserData {
                    checkers: self.find_checkers(user)?,
                    groups: HashMap::new(),
                };
                if self.current(&data, user)?.is_none() {
                    let sci = self.avail_system_checker(&data.checkers);
                    self.set_current(sci.as_deref(), user)?;
                }
                e.insert(data)
            }
        };
        f(data).map(Some)
    }

    /// `onUnlockUser`: the user's data made (`initializeInternalStateLocked`).
    pub fn on_user_unlocking(&self, user: i32) {
        if let Err(e) = self.with_data(user, |_| Ok(())) {
            eprintln!("app host: textservices: user {user}: {}", e.message);
        }
    }

    /// `verifyUser`.
    fn verify_user(&self, call: &Call<'_>, user: i32) -> Result<(), Exception> {
        let uid = call.sender_euid as i32;
        let calling_user = uid / PER_USER_RANGE as i32;
        if user != calling_user
            && self
                .host
                .check_permission(Some(INTERACT_ACROSS_USERS_FULL), uid)?
                != 0
        {
            return Err(Exception::security(format!(
                "Cross-user interaction requires INTERACT_ACROSS_USERS_FULL. userId={user} callingUserId={calling_user}: Neither user {uid} nor current process has {INTERACT_ACROSS_USERS_FULL}."
            )));
        }
        Ok(())
    }

    /// `canCallerAccessSpellChecker`: the current one, or one whose
    /// package the caller may see (`filterAppAccess`).
    fn can_access(
        &self,
        checker: &Checker,
        current: Option<&str>,
        uid: i32,
        user: i32,
    ) -> Result<bool, Exception> {
        if current == Some(checker.id.as_str()) {
            return Ok(true);
        }
        let (state, resolution) = self.host.live_packages()?;
        let filtered = crate::package::apps_filter::should_filter_application(
            &state,
            &resolution.apps_filter,
            state.packages.get(&checker.package),
            uid,
            user,
            true,
            true,
        )
        .map_err(|e| self.host.unimplemented_named("textservices", e.0))?;
        Ok(!filtered)
    }

    /// `getCurrentSpellCheckerSubtype`.
    fn current_subtype(
        &self,
        user: i32,
        allow_implicit: bool,
    ) -> Result<Option<Subtype>, Exception> {
        let Some((hash, checker)) = self.with_data(user, |data| {
            let hash = self.secure_int(SELECTED_SPELL_CHECKER_SUBTYPE, SUBTYPE_ID_NONE, user)?;
            Ok((hash, self.current(data, user)?))
        })?
        else {
            return Ok(None);
        };
        let Some(checker) = checker.filter(|c| !c.subtypes.is_empty()) else {
            return Ok(None);
        };
        if hash == SUBTYPE_ID_NONE && !allow_implicit {
            return Ok(None);
        }
        if hash != 0 {
            return Ok(checker
                .subtypes
                .iter()
                .find(|s| s.hash_code() == hash)
                .cloned());
        }
        // "Auto": the device's locale, else the first of its language.
        let Some(system) = self.system_locale() else {
            return Ok(None);
        };
        let mut first_language = None;
        for s in &checker.subtypes {
            let locale = s.locale_object();
            if locale.as_ref() == Some(&system) {
                return Ok(Some(s.clone()));
            }
            if first_language.is_none() && locale.is_some_and(|l| l.language == system.language) {
                first_language = Some(s.clone());
            }
        }
        Ok(first_language)
    }

    /// `getSpellCheckerService`.
    fn get_spell_checker_service(
        &self,
        call: &Call<'_>,
        a: tsm::GetSpellCheckerService<RawBundle>,
    ) -> Result<(), Exception> {
        self.verify_user(call, a.user_id)?;
        let (Some(id), Some(Binder::Handle(ts)), Some(Binder::Handle(sc))) = (
            a.sci_id.filter(|id| !id.is_empty()),
            a.ts_listener,
            a.sc_listener,
        ) else {
            eprintln!("app host: textservices: getSpellCheckerService: Invalid input.");
            return Ok(());
        };
        let uid = call.sender_euid as i32;
        let user = a.user_id;
        let process = &self.host.process;
        let request = Arc::new(Request {
            locale: a.locale,
            ts_listener: Arc::new(process.strong(ts)),
            sc_listener: Arc::new(process.strong(sc)),
            bundle: a
                .bundle
                .map(|b| KeptBundle::new(b, process))
                .transpose()
                .map_err(|_| Exception::illegal_argument("bad bundle"))?,
            supported_attributes: a.supported_attributes,
        });
        self.with_data(user, |data| {
            let Some(checker) = data.checkers.iter().find(|c| c.id == id).cloned() else {
                return Ok(());
            };
            let current = self.current(data, user)?;
            if !self.can_access(&checker, current.as_ref().map(|c| c.id.as_str()), uid, user)? {
                return Ok(());
            }
            if !data.groups.contains_key(&id) && !self.start_group(data, &checker, user)? {
                return Ok(());
            }
            let group = data.groups.get_mut(&id).unwrap();
            self.session_or_queue(group, request);
            self.clean(data, &id);
            Ok(())
        })
        .map(drop)
    }

    /// `startSpellCheckerServiceInnerLocked`: the spell checker bound as
    /// the user, and its group made.
    fn start_group(
        &self,
        data: &mut UserData,
        checker: &Checker,
        user: i32,
    ) -> Result<bool, Exception> {
        let connection = Arc::new_cyclic(|me| Connection {
            services: self.me.clone(),
            user,
            id: checker.id.clone(),
            me: me.clone(),
        });
        let bound = self.host.bind_service_from_host(
            Some(SERVICE_INTERFACE),
            &checker.package,
            &checker.class,
            0,
            BIND_AUTO_CREATE | BIND_IMPORTANT_BACKGROUND,
            user,
            connection.clone(),
        )?;
        if !bound {
            eprintln!("app host: textservices: Failed to get a spell checker service.");
            return Ok(false);
        }
        data.groups.insert(
            checker.id.clone(),
            Group {
                connection,
                unbind_called: false,
                spell_checker: None,
                connected: false,
                pending: Vec::new(),
                ongoing: Vec::new(),
                listeners: Vec::new(),
            },
        );
        Ok(true)
    }

    /// `RemoteCallbackList.register`: the listener kept until it dies,
    /// replacing one of the same binder.
    fn register(&self, group: &mut Group, listener: &Arc<Strong>, user: i32, id: &str) {
        self.unregister(group, listener.handle);
        let me = self.me.clone();
        let (handle, id) = (listener.handle, id.to_string());
        let death = self.host.process.link_to_death(
            listener,
            Box::new(move || {
                if let Some(me) = me.upgrade() {
                    me.remove_listener(user, &id, handle);
                }
            }),
        );
        group.listeners.push(Listener {
            strong: listener.clone(),
            death,
        });
    }

    fn unregister(&self, group: &mut Group, handle: u32) {
        group.listeners.retain(|l| {
            if l.strong.handle != handle {
                return true;
            }
            self.host.process.clear_death(&l.strong, l.death);
            false
        });
    }

    /// `getISpellCheckerSessionOrQueueLocked`.
    fn session_or_queue(&self, group: &mut Group, request: Arc<Request>) {
        if group.unbind_called {
            return;
        }
        let (user, id) = (group.connection.user, group.connection.id.clone());
        self.register(group, &request.sc_listener, user, &id);
        if !group.connected {
            group.pending.push(request);
            return;
        }
        if self.ask_session(group, &request) {
            group.ongoing.push(request);
        } else {
            self.remove_all(group);
        }
    }

    /// `ISpellCheckerService.getISpellCheckerSession` of `request`, with
    /// its callback: whether it was sent.
    fn ask_session(&self, group: &Group, request: &Arc<Request>) -> bool {
        let Some(spell_checker) = &group.spell_checker else {
            return false;
        };
        let callback = self.host.process.add_service(Arc::new(SessionCallback {
            services: self.me.clone(),
            user: group.connection.user,
            id: group.connection.id.clone(),
            request: Mutex::new(Some(Arc::downgrade(request))),
        }));
        let mut p = Parcel::new();
        scs::GetISpellCheckerSession {
            locale: request.locale.clone(),
            listener: Some(request.sc_listener.binder()),
            bundle: request.bundle.as_ref().map(|b| b.bundle.clone()),
            supported_attributes: request.supported_attributes,
            callback: Some(callback),
        }
        .write(&mut p);
        spell_checker
            .transact(scs::GET_I_SPELL_CHECKER_SESSION, &p, true)
            .is_ok()
    }

    /// `removeAllLocked`.
    fn remove_all(&self, group: &mut Group) {
        eprintln!("app host: textservices: Remove the spell checker bind unexpectedly.");
        for l in std::mem::take(&mut group.listeners) {
            self.host.process.clear_death(&l.strong, l.death);
        }
        group.pending.clear();
        group.ongoing.clear();
    }

    /// `cleanLocked` of the group `id`: with no client left, it is removed
    /// and unbound, once.
    fn clean(&self, data: &mut UserData, id: &str) {
        let Some(group) = data.groups.get_mut(id) else {
            return;
        };
        if group.unbind_called
            || !group.listeners.is_empty()
            || !group.pending.is_empty()
            || !group.ongoing.is_empty()
        {
            return;
        }
        group.unbind_called = true;
        let connection: Arc<dyn HostConnection> = group.connection.clone();
        data.groups.remove(id);
        self.host.unbind_service_from_host(&connection);
    }

    /// `SpellCheckerBindGroup.removeListener` of every group of the user
    /// that has `listener`, as its death or `finishSpellCheckerService`
    /// removes it.
    fn remove_listener(&self, user: i32, id: &str, handle: u32) {
        let mut users = self.users.lock().unwrap();
        let Some(data) = users.get_mut(&user) else {
            return;
        };
        if let Some(group) = data.groups.get_mut(id) {
            self.unregister(group, handle);
            group.pending.retain(|r| r.sc_listener.handle != handle);
            group.ongoing.retain(|r| r.sc_listener.handle != handle);
        }
        self.clean(data, id);
    }

    /// `onServiceConnected` and `onServiceDisconnected` of the group's
    /// connection, on the main thread.
    fn connected(&self, connection: &Arc<Connection>, service: Option<Strong>) {
        let (user, id) = (connection.user, connection.id.as_str());
        let mut users = self.users.lock().unwrap();
        let Some(data) = users.get_mut(&user) else {
            return;
        };
        let Some(group) = data
            .groups
            .get_mut(id)
            .filter(|g| Arc::ptr_eq(&g.connection, connection))
        else {
            return;
        };
        let Some(service) = service else {
            group.spell_checker = None;
            group.connected = false;
            return;
        };
        if group.unbind_called {
            return;
        }
        group.spell_checker = Some(service);
        group.connected = true;
        for request in std::mem::take(&mut group.pending) {
            if !self.ask_session(group, &request) {
                self.remove_all(group);
                break;
            }
            group.ongoing.push(request);
        }
        self.clean(data, id);
    }

    /// `SpellCheckerBindGroup.onSessionCreated`.
    fn session_created(
        &self,
        user: i32,
        id: &str,
        request: &Arc<Request>,
        session: Option<Binder>,
    ) {
        let mut users = self.users.lock().unwrap();
        let Some(data) = users.get_mut(&user) else {
            return;
        };
        let Some(group) = data.groups.get_mut(id) else {
            return;
        };
        if group.unbind_called {
            return;
        }
        if let Some(i) = group.ongoing.iter().position(|r| Arc::ptr_eq(r, request)) {
            group.ongoing.remove(i);
            let mut p = Parcel::new();
            ts_listener::OnServiceConnected {
                spell_checker_session: session,
            }
            .write(&mut p);
            // A client already dead is forgotten with its request.
            let _ = request
                .ts_listener
                .transact(ts_listener::ON_SERVICE_CONNECTED, &p, true);
        }
        self.clean(data, id);
    }
}

/// `InternalServiceConnection`.
impl PackageMonitor for TextServices {
    /// On its handler (`register(context, null, ...)`), when the whole
    /// package changed (`onPackageChanged`'s default).
    fn package_changed(&self, change: &PackageChanged<'_>) {
        if !change.some_packages_changed() {
            return;
        }
        let (me, user) = (self.me.clone(), change.user);
        self.post(move || {
            if let Some(me) = me.upgrade()
                && let Err(e) = me.packages_changed(user)
            {
                eprintln!(
                    "app host: textservices: packages of user {user}: {}",
                    e.message
                );
            }
        });
    }
}

struct Connection {
    services: Weak<TextServices>,
    user: i32,
    id: String,
    me: Weak<Connection>,
}

impl HostConnection for Connection {
    /// Called with the services locked: the call is posted to the main
    /// thread, as `ServiceDispatcher` posts it.
    fn connected(&self, service: Option<Binder>, _dead: bool) {
        let (Some(services), Some(me)) = (self.services.upgrade(), self.me.upgrade()) else {
            return;
        };
        let strong = match service {
            Some(Binder::Handle(h)) => Some(services.host.process.strong(h)),
            _ => None,
        };
        let s = services.clone();
        services.post(move || s.connected(&me, strong));
    }
}

/// `ISpellCheckerServiceCallbackBinder`: its request until the session
/// comes, once.
struct SessionCallback {
    services: Weak<TextServices>,
    user: i32,
    id: String,
    request: Mutex<Option<Weak<Request>>>,
}

impl Service for SessionCallback {
    fn descriptor(&self) -> &str {
        scs_callback::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != scs_callback::ON_SESSION_CREATED {
            return Err(BAD_VALUE);
        }
        let a = scs_callback::OnSessionCreated::read(&mut call.data).map_err(|_| BAD_VALUE)?;
        let request = self
            .request
            .lock()
            .unwrap()
            .take()
            .and_then(|r| r.upgrade());
        if let (Some(services), Some(request)) = (self.services.upgrade(), request) {
            services.session_created(self.user, &self.id, &request, a.new_session);
        }
        Ok(Parcel::new())
    }
}

impl Service for TextServices {
    fn descriptor(&self) -> &str {
        tsm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            tsm::GET_CURRENT_SPELL_CHECKER => {
                let a = tsm::GetCurrentSpellChecker::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.verify_user(call, a.user_id)
                    .and_then(|()| self.with_data(a.user_id, |data| self.current(data, a.user_id)))
                    .map(|c| {
                        tsm::write_get_current_spell_checker_reply(&mut p, c.flatten().as_deref())
                    })
            }
            tsm::GET_CURRENT_SPELL_CHECKER_SUBTYPE => {
                let a = tsm::GetCurrentSpellCheckerSubtype::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.verify_user(call, a.user_id)
                    .and_then(|()| {
                        self.current_subtype(a.user_id, a.allow_implicitly_selected_subtype)
                    })
                    .map(|s| tsm::write_get_current_spell_checker_subtype_reply(&mut p, s.as_ref()))
            }
            tsm::GET_SPELL_CHECKER_SERVICE => {
                let a = tsm::GetSpellCheckerService::<RawBundle>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.get_spell_checker_service(call, a)
            }
            tsm::FINISH_SPELL_CHECKER_SERVICE => {
                let a =
                    tsm::FinishSpellCheckerService::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.verify_user(call, a.user_id).and_then(|()| {
                    let Some(Binder::Handle(handle)) = a.listener else {
                        return Err(Exception::new(
                            aim_binder_host::parcel::EX_NULL_POINTER,
                            "Attempt to invoke interface method 'android.os.IBinder android.os.IInterface.asBinder()' on a null object reference",
                        ));
                    };
                    let ids: Vec<String> = self
                        .users
                        .lock()
                        .unwrap()
                        .get(&a.user_id)
                        .map(|d| d.groups.keys().cloned().collect())
                        .unwrap_or_default();
                    for id in ids {
                        self.remove_listener(a.user_id, &id, handle);
                    }
                    Ok(())
                })
            }
            tsm::IS_SPELL_CHECKER_ENABLED => {
                let a = tsm::IsSpellCheckerEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.verify_user(call, a.user_id)
                    .and_then(|()| self.with_data(a.user_id, |_| self.enabled(a.user_id)))
                    .map(|e| tsm::write_is_spell_checker_enabled_reply(&mut p, e.unwrap_or(false)))
            }
            tsm::GET_ENABLED_SPELL_CHECKERS => {
                let a =
                    tsm::GetEnabledSpellCheckers::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.verify_user(call, a.user_id)
                    .and_then(|()| {
                        self.with_data(a.user_id, |data| {
                            Ok((data.checkers.clone(), self.current(data, a.user_id)?))
                        })
                    })
                    .and_then(|found| {
                        let Some((checkers, current)) = found else {
                            return Ok(None);
                        };
                        let current = current.as_ref().map(|c| c.id.as_str());
                        let mut visible = Vec::new();
                        for c in checkers {
                            if self.can_access(&c, current, uid, a.user_id)? {
                                visible.push(Some((*c).clone()));
                            }
                        }
                        Ok((!visible.is_empty()).then_some(visible))
                    })
                    .map(|list| {
                        tsm::write_get_enabled_spell_checkers_reply(&mut p, list.as_deref())
                    })
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags_are_read_as_java_reads_them() {
        let l = Locale::for_language_tag("zh-Hans-CN");
        assert_eq!(
            (l.language.as_str(), l.script.as_str(), l.country.as_str()),
            ("zh", "Hans", "CN")
        );
        assert_eq!(
            Locale::for_language_tag("en-US"),
            Locale::new("en", "US", "")
        );
        assert_eq!(Locale::for_language_tag("und").language, "");
        let l = Locale::for_language_tag("en-US-u-mu-celsius");
        assert_eq!(
            (l.country.as_str(), l.extensions.as_str()),
            ("US", "u-mu-celsius")
        );
        assert_eq!(Locale::for_language_tag("de-DE-1996").variant, "1996");
    }

    #[test]
    fn subtype_locales_are_split_on_underscores() {
        assert_eq!(
            Locale::from_subtype_string("en_US"),
            Some(Locale::new("en", "US", ""))
        );
        assert_eq!(
            Locale::from_subtype_string("tl"),
            Some(Locale::new("fil", "", ""))
        );
        assert_eq!(
            Locale::from_subtype_string("sr_ZZ_Latn_x"),
            Some(Locale::new("sr", "ZZ", "Latn_x"))
        );
        assert_eq!(Locale::from_subtype_string(""), None);
    }

    #[test]
    fn suitable_locales_follow_the_originals_order() {
        let ko = Locale::new("ko", "KR", "");
        assert_eq!(
            suitable_locales(Some(&ko)),
            vec![
                Locale::new("ko", "KR", ""),
                Locale::new("ko", "", ""),
                Locale::new("en", "US", ""),
                Locale::new("en", "GB", ""),
                Locale::new("en", "", ""),
            ]
        );
        assert_eq!(
            suitable_locales(Some(&Locale::new("en", "GB", ""))),
            vec![
                Locale::new("en", "GB", ""),
                Locale::new("en", "US", ""),
                Locale::new("en", "", ""),
            ]
        );
        assert_eq!(
            suitable_locales(Some(&Locale::new("en", "", ""))),
            vec![
                Locale::new("en", "", ""),
                Locale::new("en", "US", ""),
                Locale::new("en", "GB", ""),
            ]
        );
    }

    #[test]
    fn subtype_hash_codes_are_javas() {
        let s = Subtype {
            name_res: 0,
            locale: "en_US".into(),
            language_tag: String::new(),
            extra_value: String::new(),
            id: 0,
        };
        // `Arrays.hashCode(new Object[] {"en_US", ""})`: 31 * (31 + h) + 0.
        let h = java_hash("en_US");
        assert_eq!(h, 96646644);
        assert_eq!(s.hash_code(), (31i32.wrapping_add(h)).wrapping_mul(31));
        assert_eq!(Subtype { id: 7, ..s }.hash_code(), 7);
    }

    #[test]
    fn subtypes_are_written_as_their_parcel_reads_them() {
        let s = Subtype {
            name_res: 3,
            locale: "en_US".into(),
            language_tag: "en-US".into(),
            extra_value: "a=b".into(),
            id: 0,
        };
        let mut p = Parcel::new();
        s.write_to(&mut p);
        let mut r = aim_binder_host::parcel::Reader::new(p.data(), &[]);
        assert_eq!(r.read_i32().unwrap(), 3);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("en_US"));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("en-US"));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("a=b"));
        assert_eq!(r.read_i32().unwrap(), 0);
    }
}
