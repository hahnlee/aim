//! Web links with several handlers (`android-16.0.0_r1`):
//! `filterCandidatesWithDomainPreferredActivitiesLPrBody` of one profile,
//! with domain verification's approval (`DomainVerificationService`'s
//! `filterToApprovedApp` and `approvalLevelForDomain`) and the default
//! browser. The approval reads what the feed gives of domain
//! verification: each user's selection, whose hosts are the package's web
//! domains marked verified or selected, and the URI relative filter
//! groups; and the legacy per-user states package-restrictions.xml keeps.

use std::collections::HashMap;

use super::super::intent::Intent;
use super::super::intent_filter::{CATEGORY_BROWSABLE, UriRelativeFilterGroup};
use super::super::model::{PackageState, User};
use super::super::pkg::booleans;
use super::super::restrictions::Restrictions;
use super::*;

/// `DomainVerificationManagerInternal.APPROVAL_LEVEL_*`.
const APPROVAL_LEVEL_NONE: i32 = 0;
const APPROVAL_LEVEL_LEGACY_ASK: i32 = 1;
const APPROVAL_LEVEL_LEGACY_ALWAYS: i32 = 2;
const APPROVAL_LEVEL_SELECTION: i32 = 3;
const APPROVAL_LEVEL_VERIFIED: i32 = 4;
/// `DomainVerificationUserState.DOMAIN_STATE_*`.
const DOMAIN_STATE_SELECTED: i32 = 1;
const DOMAIN_STATE_VERIFIED: i32 = 2;
/// `PackageManager.INTENT_FILTER_DOMAIN_VERIFICATION_STATUS_*`.
const STATUS_ASK: i32 = 1;
const STATUS_ALWAYS: i32 = 2;
const STATUS_NEVER: i32 = 3;
const STATUS_ALWAYS_ASK: i32 = 4;
/// `PackageManager.COMPONENT_ENABLED_STATE_*`.
const ENABLED: i32 = 1;
const DISABLED: i32 = 2;
const DISABLED_USER: i32 = 3;
const DISABLED_UNTIL_USED: i32 = 4;
/// `PackageManager.MATCH_ALL`.
const MATCH_ALL: i64 = 0x0002_0000;
/// `DomainVerificationManager.SETTINGS_API_V2`: on after Android 11 (R).
const SETTINGS_API_V2_AFTER_SDK: i32 = 30;
const CATEGORY_DEFAULT: &str = "android.intent.category.DEFAULT";

/// `Patterns.DOMAIN_NAME` matched whole: a host name of IRI labels and a
/// top-level domain, or an IPv4 address.
pub fn is_domain_name(host: &str) -> bool {
    is_host_name(host) || is_ip_address(host)
}

/// `UCS_CHAR` of RFC 3987, without the space characters.
fn ucs(c: char) -> bool {
    let c = c as u32;
    (0xA0..=0xD7FF).contains(&c)
        || (0xF900..=0xFDCF).contains(&c)
        || (0xFDF0..=0xFFEF).contains(&c)
        || ((0x10000..=0xEFFFD).contains(&c) && c & 0xFFFE != 0xFFFE)
}

fn label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || ucs(c)
}

/// `IRI_LABEL`: 1-63 characters, `_` and `-` only inside.
fn is_label(l: &str) -> bool {
    let n = l.chars().count();
    let (first, last) = (l.chars().next(), l.chars().last());
    (1..=63).contains(&n)
        && first.is_some_and(label_char)
        && last.is_some_and(label_char)
        && l.chars().all(|c| label_char(c) || c == '_' || c == '-')
}

/// `TLD`: punycode (`xn--` and word characters) or 2-63 letters.
fn is_tld(t: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    if let Some(rest) = t.strip_prefix("xn--") {
        return (1..=59).contains(&rest.len())
            && rest.chars().last().is_some_and(word)
            && rest.chars().all(|c| word(c) || c == '-');
    }
    (2..=63).contains(&t.chars().count()) && t.chars().all(|c| c.is_ascii_alphabetic() || ucs(c))
}

fn is_host_name(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2
        && labels[..labels.len() - 1].iter().all(|l| is_label(l))
        && is_tld(labels[labels.len() - 1])
}

/// `IP_ADDRESS_STRING`: the first octet not 0, the last a single digit
/// or more.
fn is_ip_address(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    let octet = |p: &str, zero: bool| {
        let b = p.as_bytes();
        let digits = !b.is_empty() && b.iter().all(u8::is_ascii_digit);
        digits
            && match b.len() {
                1 => b[0] != b'0' || zero,
                2 => b[0] != b'0',
                3 => {
                    matches!(b[0], b'0' | b'1')
                        || (b[0] == b'2' && (b[1] < b'5' || (b[1] == b'5' && b[2] <= b'5')))
                }
                _ => false,
            }
    };
    octet(parts[0], false)
        && octet(parts[1], true)
        && octet(parts[2], true)
        && octet(parts[3], true)
}

/// `DomainVerificationUtils.isDomainVerificationIntent`.
pub fn is_domain_verification_intent(intent: &Intent, flags: i64) -> bool {
    if !intent.is_web_intent() {
        return false;
    }
    let host = intent
        .data
        .as_ref()
        .and_then(|d| d.host())
        .unwrap_or_default();
    if host.is_empty() || !is_domain_name(&host) {
        return false;
    }
    let categories = intent.categories.as_deref().unwrap_or_default();
    let has = |c: &str| categories.iter().any(|x| x == c);
    let default_by_flags = flags & MATCH_DEFAULT_ONLY != 0;
    match categories.len() {
        n if n > 2 => false,
        2 => has(CATEGORY_DEFAULT) && has(CATEGORY_BROWSABLE),
        0 => default_by_flags,
        _ if has(CATEGORY_BROWSABLE) => default_by_flags,
        _ => has(CATEGORY_DEFAULT),
    }
}

/// A verified or selected host of the package matching `host`, exactly
/// or through a `*.` wildcard (`host.endsWith`, as the original checks).
fn host_in(states: &[(String, i32)], host: &str, state: i32) -> bool {
    states.iter().any(|(d, s)| *s == state && d == host)
        || states.iter().any(|(d, s)| {
            *s == state
                && d.strip_prefix("*.")
                    .is_some_and(|base| host.ends_with(base))
        })
}

/// The legacy domain verification states of a user's packages, from
/// package-restrictions.xml.
pub(super) fn legacy_domain_states(user: &User) -> HashMap<String, i32> {
    let Some(bytes) = user.restrictions.as_deref() else {
        return HashMap::new();
    };
    let Some(r) = aim_android_xml::read(bytes)
        .ok()
        .and_then(|e| Restrictions::parse(&e).ok())
    else {
        return HashMap::new();
    };
    r.legacy_domain_states.into_iter().collect()
}

impl Resolution {
    /// `approvalLevelForDomainInternal`, not including negative levels.
    pub(super) fn approval_level(&self, ps: &PackageState, host: &str, user: i32) -> Result<i32> {
        let Some(us) = ps.users.get(&user) else {
            return Ok(APPROVAL_LEVEL_NONE);
        };
        let pkg = ps.pkg.as_deref();
        let enabled = match us.enabled {
            ENABLED => true,
            DISABLED | DISABLED_USER | DISABLED_UNTIL_USED => false,
            _ => pkg.is_some_and(|p| p.booleans & booleans::ENABLED != 0),
        };
        if !us.installed || !enabled || !us.suspended_by.is_empty() {
            return Ok(APPROVAL_LEVEL_NONE);
        }
        if pkg.is_some() && ps.target_sdk_version <= SETTINGS_API_V2_AFTER_SDK {
            match self.legacy.get(&user).and_then(|m| m.get(&ps.name)) {
                Some(&STATUS_NEVER) => return Ok(APPROVAL_LEVEL_NONE),
                Some(&(STATUS_ASK | STATUS_ALWAYS_ASK)) => return Ok(APPROVAL_LEVEL_LEGACY_ASK),
                Some(&STATUS_ALWAYS) => return Ok(APPROVAL_LEVEL_LEGACY_ALWAYS),
                _ => {}
            }
        }
        let Some((allowed, hosts)) = &us.domain_selection else {
            return Ok(APPROVAL_LEVEL_NONE);
        };
        if !allowed {
            return Ok(APPROVAL_LEVEL_NONE);
        }
        if us.instant_app {
            return Err(NotModelled("an instant app's domain approval").into());
        }
        Ok(if host_in(hosts, host, DOMAIN_STATE_VERIFIED) {
            APPROVAL_LEVEL_VERIFIED
        } else if host_in(hosts, host, DOMAIN_STATE_SELECTED) {
            APPROVAL_LEVEL_SELECTION
        } else {
            APPROVAL_LEVEL_NONE
        })
    }

    /// `filterToApprovedApp`: the results of the packages with the highest
    /// approval of the intent's host, then of the last installed of
    /// them, each package's last declared activity.
    fn filter_to_approved_app(
        &self,
        intent: &Intent,
        infos: &[ResolveInfo],
        user: i32,
    ) -> Result<(Vec<ResolveInfo>, i32)> {
        let data = intent
            .data
            .as_ref()
            .expect("a domain verification intent has data");
        let host = data.host().unwrap_or_default();
        let mut levels: HashMap<&str, i32> = HashMap::new();
        let mut highest = APPROVAL_LEVEL_NONE;
        for ri in infos.iter().filter(|ri| ri.auto_resolution_allowed) {
            let package = ri.component().0;
            if levels.contains_key(package) {
                continue;
            }
            let ps = self.state.packages.get(package);
            let groups = ps
                .and_then(|ps| {
                    ps.uri_relative_filter_groups
                        .iter()
                        .find(|(d, _)| *d == host)
                })
                .map_or(&[][..], |(_, g)| g.as_slice());
            let level = match ps {
                Some(ps)
                    if groups.is_empty() || UriRelativeFilterGroup::match_groups(groups, data).map_err(ResolutionError::UriMatching)? =>
                {
                    self.approval_level(ps, &host, user)?
                }
                _ => APPROVAL_LEVEL_NONE,
            };
            highest = highest.max(level);
            levels.insert(package, level);
        }
        if highest <= APPROVAL_LEVEL_NONE {
            return Ok((Vec::new(), highest));
        }
        let mut approved: Vec<&ResolveInfo> = infos
            .iter()
            .filter(|ri| {
                ri.auto_resolution_allowed && levels.get(ri.component().0) == Some(&highest)
            })
            .collect();
        if highest != APPROVAL_LEVEL_LEGACY_ASK {
            // filterToLastFirstInstalled: the original keeps the first of
            // equal times in an identity-hashed order.
            let installed = |ri: &ResolveInfo| {
                self.state.packages.get(ri.component().0).map(|ps| {
                    let times = ps
                        .users
                        .values()
                        .map(|u| u.first_install_time)
                        .filter(|&t| t != 0);
                    times.min().unwrap_or(0)
                })
            };
            let latest = approved.iter().filter_map(|ri| installed(ri)).max();
            let mut packages: Vec<&str> = approved
                .iter()
                .filter(|ri| installed(ri) == latest)
                .map(|ri| ri.component().0)
                .collect();
            packages.dedup();
            if packages.len() > 1 {
                return Err(NotModelled(
                    "approved web handlers installed at the same time",
                ).into());
            }
            approved.retain(|ri| Some(ri.component().0) == packages.first().copied());
            // filterToLastDeclared: of one package, its last declared
            // activity.
            if let Some(last) = approved
                .iter()
                .copied()
                .max_by_key(|ri| self.activity_index(ri))
            {
                approved = vec![last];
            }
        }
        Ok((approved.into_iter().cloned().collect(), highest))
    }

    /// `indexOfIntentFilterEntry`.
    fn activity_index(&self, ri: &ResolveInfo) -> i64 {
        let (package, class) = ri.component();
        let pkg = self
            .state
            .packages
            .get(package)
            .and_then(|ps| ps.pkg.as_deref());
        pkg.and_then(|p| {
            p.activities
                .iter()
                .position(|a| a.main.component.name == class)
        })
        .map_or(-1, |i| i as i64)
    }

    /// `filterCandidatesWithDomainPreferredActivitiesLPrBody` of one
    /// profile.
    pub(super) fn filter_web_candidates(
        &self,
        intent: &Intent,
        flags: i64,
        candidates: Vec<ResolveInfo>,
        user: i32,
    ) -> Result<Vec<ResolveInfo>> {
        if candidates.iter().any(|ri| ri.is_instant_app_available) {
            return Err(NotModelled("web instant apps' setting").into());
        }
        let (all, undefined): (Vec<ResolveInfo>, Vec<ResolveInfo>) = candidates
            .iter()
            .cloned()
            .partition(|ri| ri.handle_all_web_data_uri);
        let mut result;
        let include_browser;
        if !is_domain_verification_intent(intent, flags) {
            result = undefined;
            include_browser = true;
        } else {
            let (approved, _) = self.filter_to_approved_app(intent, &undefined, user)?;
            include_browser = approved.is_empty();
            result = approved;
        }
        if include_browser {
            if flags & MATCH_ALL != 0 {
                result.extend(all);
            } else {
                let browser = self
                    .state
                    .users
                    .get(&user)
                    .and_then(|u| u.default_browser.as_deref());
                let max = all.iter().map(|ri| ri.priority).fold(0, i32::max);
                let mut chosen: Option<&ResolveInfo> = None;
                for ri in &all {
                    if Some(ri.component().0) == browser
                        && chosen.is_none_or(|c| c.priority < ri.priority)
                    {
                        chosen = Some(ri);
                    }
                }
                match chosen {
                    Some(c) if c.priority >= max && browser.is_some_and(|b| !b.is_empty()) => {
                        result.push(c.clone())
                    }
                    _ => result.extend(all.iter().cloned()),
                }
            }
            if result.is_empty() {
                result = candidates;
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_names() {
        for yes in [
            "example.com",
            "a.b.co",
            "xn--p1ai.xn--p1ai",
            "1.2.3.4",
            "10.0.0.0",
            "ex-ample.com",
            "例え.テスト",
        ] {
            assert!(is_domain_name(yes), "{yes}");
        }
        for no in [
            "localhost",
            "example.",
            ".com",
            "-a.com",
            "a.c",
            "0.1.2.3",
            "1.2.3",
            "256.1.1.1",
            "a_.com",
            "a..com",
        ] {
            assert!(!is_domain_name(no), "{no}");
        }
    }

    #[test]
    fn verification_intents() {
        let web = |cats: &[&str]| Intent {
            action: Some("android.intent.action.VIEW".into()),
            data: Some(super::super::super::uri::Uri::parse(
                "https://example.com/x",
            )),
            categories: (!cats.is_empty()).then(|| cats.iter().map(|c| c.to_string()).collect()),
            ..Intent::default()
        };
        assert!(is_domain_verification_intent(&web(&[]), MATCH_DEFAULT_ONLY));
        assert!(!is_domain_verification_intent(&web(&[]), 0));
        assert!(is_domain_verification_intent(
            &web(&[CATEGORY_BROWSABLE, CATEGORY_DEFAULT]),
            0
        ));
        assert!(!is_domain_verification_intent(
            &web(&[CATEGORY_BROWSABLE]),
            0
        ));
        assert!(is_domain_verification_intent(&web(&[CATEGORY_DEFAULT]), 0));
        assert!(!is_domain_verification_intent(
            &web(&["a", "b", "c"]),
            MATCH_DEFAULT_ONLY
        ));
        let states = [("*.example.com".to_owned(), DOMAIN_STATE_VERIFIED)];
        assert!(host_in(&states, "www.example.com", DOMAIN_STATE_VERIFIED));
        assert!(!host_in(&states, "www.example.com", DOMAIN_STATE_SELECTED));
    }
}
