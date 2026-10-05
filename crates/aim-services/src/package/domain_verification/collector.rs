//! DomainVerificationCollector at android-16.0.0_r1 (#957).
use crate::package::{info::array_order, intent_filter::IntentFilter, pkg::AndroidPackage};

const MAX_BYTES: usize = 1024 * 1024;
const CATEGORY_DEFAULT: &str = "android.intent.category.DEFAULT";

/// RESTRICT_DOMAINS must come from the compatibility owner, including overrides.
#[derive(Clone, Copy)]
pub struct Policy {
    pub restrict_domains: bool,
    pub linked_app: bool,
}
#[derive(Clone, Copy)]
pub enum Kind {
    Web,
    ValidAutoVerify,
    InvalidAutoVerify,
}

/// The collector's ArraySet, including its signed Java hash order.
pub fn collect(package: &AndroidPackage, policy: Policy, kind: Kind) -> Vec<String> {
    collect_filters(
        package
            .activities
            .iter()
            .flat_map(|a| a.main.component.intents.iter().map(|i| &i.filter)),
        policy,
        kind,
        MAX_BYTES,
    )
}

fn collect_filters<'a>(
    filters: impl Iterator<Item = &'a IntentFilter> + Clone,
    policy: Policy,
    kind: Kind,
    limit: usize,
) -> Vec<String> {
    let auto = !matches!(kind, Kind::Web);
    let valid = !matches!(kind, Kind::InvalidAutoVerify);
    let legacy = auto && !policy.restrict_domains;
    if legacy
        && !policy.linked_app
        && !filters
            .clone()
            .any(|f| f.auto_verify && f.handles_web_uris(true))
    {
        return vec![];
    }
    let mut hosts = Vec::new();
    let mut size = 0;
    for filter in filters {
        if size >= limit {
            break;
        }
        if legacy {
            if !filter.handles_web_uris(false) {
                continue;
            }
        } else if (auto && !filter.auto_verify)
            || !filter.has_category(CATEGORY_DEFAULT)
            || !filter.handles_web_uris(auto)
        {
            continue;
        }
        for authority in filter.authorities.iter().flatten() {
            // Legacy checks the bound between filters, not between their authorities.
            if !legacy && size >= limit {
                break;
            }
            let host = &authority.orig_host;
            if valid_host(host) != valid {
                continue;
            }
            // Java String.length, including duplicates and the host crossing the bound.
            size += host.encode_utf16().count() * 2 + 12;
            if !hosts.contains(host) {
                hosts.push(host.clone());
            }
        }
    }
    array_order(hosts, |s| s)
}

fn valid_host(host: &str) -> bool {
    crate::package::resolve::is_domain_name(host.strip_prefix("*.").unwrap_or(host))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::intent_filter::{ACTION_VIEW, CATEGORY_BROWSABLE};

    fn filter(auto: bool, default: bool, schemes: &[&str], hosts: &[&str]) -> IntentFilter {
        let mut f = IntentFilter {
            auto_verify: auto,
            ..Default::default()
        };
        f.add_action(ACTION_VIEW);
        f.add_category(CATEGORY_BROWSABLE);
        if default {
            f.add_category(CATEGORY_DEFAULT);
        }
        for scheme in schemes {
            f.add_data_scheme(scheme);
        }
        for host in hosts {
            f.add_data_authority(host, None);
        }
        f
    }
    fn run(
        filters: &[IntentFilter],
        restrict: bool,
        linked: bool,
        kind: Kind,
        limit: usize,
    ) -> Vec<String> {
        collect_filters(
            filters.iter(),
            Policy {
                restrict_domains: restrict,
                linked_app: linked,
            },
            kind,
            limit,
        )
    }
    #[test]
    fn compatibility_selects_modern_and_legacy_filter_rules() {
        let filters = [
            filter(true, false, &["https"], &["seed.example"]),
            filter(false, false, &["https", "custom"], &["legacy.example"]),
            filter(
                true,
                true,
                &["https"],
                &["*.modern.example", "invalid", "modern.example"],
            ),
        ];
        let sorted =
            |hosts: &[&str]| array_order(hosts.iter().map(|s| s.to_string()).collect(), |s| s);
        assert_eq!(
            run(&filters, true, false, Kind::ValidAutoVerify, MAX_BYTES),
            sorted(&["*.modern.example", "modern.example"])
        );
        assert_eq!(
            run(&filters, false, false, Kind::ValidAutoVerify, MAX_BYTES),
            sorted(&[
                "seed.example",
                "legacy.example",
                "*.modern.example",
                "modern.example"
            ])
        );
        assert_eq!(
            run(&filters, false, false, Kind::Web, MAX_BYTES),
            sorted(&["*.modern.example", "modern.example"])
        );
        assert_eq!(
            run(&filters, true, false, Kind::InvalidAutoVerify, MAX_BYTES),
            vec!["invalid"]
        );
        let unmarked = [filter(false, false, &["https"], &["linked.example"])];
        assert!(run(&unmarked, false, false, Kind::ValidAutoVerify, MAX_BYTES).is_empty());
        assert_eq!(
            run(&unmarked, false, true, Kind::ValidAutoVerify, MAX_BYTES),
            vec!["linked.example"]
        );
        assert!(run(&unmarked, true, true, Kind::ValidAutoVerify, MAX_BYTES).is_empty());
        let mixed_seed = [filter(true, true, &["https", "custom"], &["mixed.example"])];
        assert!(run(&mixed_seed, false, false, Kind::ValidAutoVerify, MAX_BYTES).is_empty());
    }
    #[test]
    fn byte_bound_preserves_duplicate_accounting_and_legacy_inner_loop() {
        let filters = [
            filter(
                true,
                true,
                &["https"],
                &["a.example", "a.example", "b.example"],
            ),
            filter(true, true, &["https"], &["c.example"]),
        ];
        let one = "a.example".encode_utf16().count() * 2 + 12;
        // AuthorityEntry add suppresses the duplicate, so retain it as the original
        // parcel can: counting occurs before the collector's ArraySet deduplication.
        let mut filters = filters;
        let duplicate = filters[0].authorities.as_ref().unwrap()[0].clone();
        filters[0]
            .authorities
            .as_mut()
            .unwrap()
            .insert(1, duplicate);
        assert_eq!(
            run(&filters, true, false, Kind::ValidAutoVerify, 2 * one),
            vec!["a.example"]
        );
        assert_eq!(
            run(&filters, false, false, Kind::ValidAutoVerify, 2 * one),
            vec!["a.example", "b.example"]
        );
        assert_eq!(
            run(&filters, true, false, Kind::ValidAutoVerify, 1),
            vec!["a.example"]
        );
        let non_bmp = [filter(true, true, &["https"], &["𐀀.example", "b.example"])];
        assert_eq!(
            run(&non_bmp, true, false, Kind::ValidAutoVerify, 32),
            vec!["𐀀.example"]
        );
    }
}
