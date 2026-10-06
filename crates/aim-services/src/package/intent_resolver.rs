//! `com.android.server.IntentResolver`: filters indexed by MIME type,
//! scheme and action, and the cuts of them an intent is matched against,
//! in the original's order (`android-16.0.0_r1`). A result's order before
//! sorting is the cuts' order, then each cut's add order; the resolvers
//! sort stably, so equal results keep it.

use std::collections::HashMap;

use super::intent::Intent;
use super::intent_filter::IntentFilter;

pub const CATEGORY_DEFAULT: &str = "android.intent.category.DEFAULT";

/// A filter in a resolver: the filter it matches with and the package it
/// belongs to (`isPackageForFilter`).
pub trait Entry {
    fn filter(&self) -> &IntentFilter;
    fn package(&self) -> &str;
}

/// What one resolver adds to the shared matching (the subclass's
/// overrides of `isFilterStopped`, `allowFilterResult`, `newResult`).
pub trait Build<E, R> {
    /// `isFilterStopped`: skipped when the intent excludes stopped
    /// packages.
    fn stopped(&mut self, _entry: &E) -> bool {
        false
    }
    /// `allowFilterResult`: whether the entry's component is not yet in
    /// `dest`.
    fn allow(&mut self, _entry: &E, _dest: &[R]) -> bool {
        true
    }
    /// `newResult`; `None` drops the match.
    fn result(&mut self, entry: &E, matched: i32) -> Option<R>;
}

#[derive(Clone, Debug)]
pub struct IntentResolver<E> {
    entries: Vec<E>,
    /// Full MIME types, partial ones as `base/*`.
    type_to_filter: HashMap<String, Vec<usize>>,
    base_type_to_filter: HashMap<String, Vec<usize>>,
    wild_type_to_filter: HashMap<String, Vec<usize>>,
    scheme_to_filter: HashMap<String, Vec<usize>>,
    /// Filters with neither a scheme nor a type.
    action_to_filter: HashMap<String, Vec<usize>>,
    typed_action_to_filter: HashMap<String, Vec<usize>>,
}

fn register(map: &mut HashMap<String, Vec<usize>>, key: &str, index: usize) {
    map.entry(key.to_owned()).or_default().push(index);
}

impl<E> Default for IntentResolver<E> {
    fn default() -> Self {
        IntentResolver {
            entries: Vec::new(),
            type_to_filter: HashMap::new(),
            base_type_to_filter: HashMap::new(),
            wild_type_to_filter: HashMap::new(),
            scheme_to_filter: HashMap::new(),
            action_to_filter: HashMap::new(),
            typed_action_to_filter: HashMap::new(),
        }
    }
}

impl<E: Entry> IntentResolver<E> {
    pub fn entries(&self) -> &[E] {
        &self.entries
    }

    /// `addFilter`.
    pub fn add(&mut self, entry: E) {
        let index = self.entries.len();
        let f = entry.filter();
        let schemes = f.schemes.as_deref().unwrap_or_default();
        for scheme in schemes {
            register(&mut self.scheme_to_filter, scheme, index);
        }
        let types = f.types.as_deref().unwrap_or_default();
        for ty in types {
            match ty.split_once('/').filter(|(base, _)| !base.is_empty()) {
                Some((base, _)) => {
                    register(&mut self.type_to_filter, ty, index);
                    register(&mut self.base_type_to_filter, base, index);
                }
                None => {
                    register(&mut self.type_to_filter, &format!("{ty}/*"), index);
                    register(&mut self.wild_type_to_filter, ty, index);
                }
            }
        }
        if schemes.is_empty() && types.is_empty() {
            for action in &f.actions {
                register(&mut self.action_to_filter, action, index);
            }
        }
        if !types.is_empty() {
            for action in &f.actions {
                register(&mut self.typed_action_to_filter, action, index);
            }
        }
        self.entries.push(entry);
    }

    /// `queryIntent`: the cuts the intent is matched against, matched in
    /// order; the caller sorts.
    pub fn query<R>(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        default_only: bool,
        build: &mut impl Build<E, R>,
    ) -> std::result::Result<Vec<R>, super::domain_verification::uri_parcel::MatchError> {
        let scheme = intent.scheme();
        let action = intent.action.as_deref();
        fn get<'a>(map: &'a HashMap<String, Vec<usize>>, key: &str) -> &'a [usize] {
            map.get(key).map_or(&[], Vec::as_slice)
        }
        let mut cuts: [&[usize]; 4] = [&[]; 4];
        if let Some(ty) = resolved_type
            && let Some(slash) = ty.find('/').filter(|&s| s > 0)
        {
            let base = &ty[..slash];
            if base != "*" {
                if ty.len() != slash + 2 || !ty[slash + 1..].starts_with('*') {
                    cuts[0] = get(&self.type_to_filter, ty);
                } else {
                    cuts[0] = get(&self.base_type_to_filter, base);
                }
                cuts[1] = get(&self.wild_type_to_filter, base);
                cuts[2] = get(&self.wild_type_to_filter, "*");
            } else if let Some(action) = action {
                cuts[0] = get(&self.typed_action_to_filter, action);
            }
        }
        if let Some(scheme) = scheme {
            cuts[3] = get(&self.scheme_to_filter, scheme);
        }
        if resolved_type.is_none()
            && scheme.is_none()
            && let Some(action) = action
        {
            cuts[0] = get(&self.action_to_filter, action);
        }
        let mut dest = Vec::new();
        for cut in cuts {
            let entries = cut.iter().map(|&i| &self.entries[i]);
            build_resolve_list(
                entries,
                intent,
                resolved_type,
                default_only,
                &mut dest,
                build,
            )?;
        }
        Ok(dest)
    }
}

/// `queryIntentFromList`: matched against each list (one per component)
/// in order; the caller sorts.
pub fn query_from_list<'a, E: Entry + 'a, R>(
    lists: impl IntoIterator<Item = impl IntoIterator<Item = &'a E>>,
    intent: &Intent,
    resolved_type: Option<&str>,
    default_only: bool,
    build: &mut impl Build<E, R>,
) -> std::result::Result<Vec<R>, super::domain_verification::uri_parcel::MatchError> {
    let mut dest = Vec::new();
    for list in lists {
        build_resolve_list(
            list.into_iter(),
            intent,
            resolved_type,
            default_only,
            &mut dest,
            build,
        )?;
    }
    Ok(dest)
}

/// `buildResolveList`.
fn build_resolve_list<'a, E: Entry + 'a, R>(
    entries: impl Iterator<Item = &'a E>,
    intent: &Intent,
    resolved_type: Option<&str>,
    default_only: bool,
    dest: &mut Vec<R>,
    build: &mut impl Build<E, R>,
) -> std::result::Result<(), super::domain_verification::uri_parcel::MatchError> {
    let excluding_stopped = intent.is_excluding_stopped();
    let categories = intent.categories.as_deref();
    for entry in entries {
        if excluding_stopped && build.stopped(entry) {
            continue;
        }
        if intent
            .package
            .as_deref()
            .is_some_and(|p| p != entry.package())
        {
            continue;
        }
        if !build.allow(entry, dest) {
            continue;
        }
        let filter = entry.filter();
        let matched = filter.matches(
            intent.action.as_deref(),
            resolved_type,
            intent.scheme(),
            intent.data.as_ref(),
            categories,
            false,
            None,
        )?;
        if matched >= 0
            && (!default_only || filter.has_category(CATEGORY_DEFAULT))
            && let Some(result) = build.result(entry, matched)
        {
            dest.push(result);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::intent_filter::PatternMatcher;
    use super::super::uri::Uri;
    use super::*;

    struct F(&'static str, IntentFilter);

    impl Entry for F {
        fn filter(&self) -> &IntentFilter {
            &self.1
        }
        fn package(&self) -> &str {
            self.0
        }
    }

    /// Results are the entries' packages; `once` keeps a package once
    /// (`allowFilterResult` by component).
    struct Names<'a> {
        once: bool,
        stopped: &'a [&'a str],
    }

    impl Build<F, &'static str> for Names<'_> {
        fn stopped(&mut self, e: &F) -> bool {
            self.stopped.contains(&e.0)
        }
        fn allow(&mut self, e: &F, dest: &[&'static str]) -> bool {
            !self.once || !dest.contains(&e.0)
        }
        fn result(&mut self, e: &F, _: i32) -> Option<&'static str> {
            Some(e.0)
        }
    }

    fn filter(actions: &[&str], types: &[&str], schemes: &[&str], default: bool) -> IntentFilter {
        let mut f = IntentFilter::default();
        actions.iter().for_each(|a| f.add_action(a));
        types.iter().for_each(|t| f.add_data_type(t).unwrap());
        schemes.iter().for_each(|s| f.add_data_scheme(s));
        if default {
            f.add_category(CATEGORY_DEFAULT);
        }
        f
    }

    fn intent(action: &str, data: Option<&str>) -> Intent {
        Intent {
            action: Some(action.into()),
            data: data.map(Uri::parse),
            ..Intent::default()
        }
    }

    fn resolver() -> IntentResolver<F> {
        let mut r = IntentResolver::default();
        r.add(F("a", filter(&["VIEW"], &["image/png"], &[], true)));
        r.add(F("b", filter(&["VIEW"], &["image/*"], &[], true)));
        r.add(F("c", filter(&["VIEW"], &["*/*"], &[], false)));
        r.add(F("d", filter(&["VIEW"], &[], &["https"], true)));
        r.add(F("e", filter(&["VIEW"], &[], &[], true)));
        r.add(F(
            "f",
            filter(&["VIEW"], &["image/png"], &["content"], true),
        ));
        r
    }

    fn query(
        r: &IntentResolver<F>,
        i: &Intent,
        ty: Option<&str>,
        default_only: bool,
    ) -> Vec<&'static str> {
        let mut b = Names {
            once: false,
            stopped: &[],
        };
        r.query(i, ty, default_only, &mut b).unwrap()
    }

    #[test]
    fn nullable_uri_error_is_not_an_empty_resolver_result() {
        let mut filter = filter(&["VIEW"], &[], &["https"], true);
        filter.add_data_authority("x", None);
        let mut group = super::super::intent_filter::UriRelativeFilterGroup::new(0);
        group.add_nullable(0, 0, None);
        filter.add_uri_relative_filter_group(group);
        let mut resolver = IntentResolver::default(); resolver.add(F("nullable", filter));
        let mut build = Names {once: false, stopped: &[]};
        assert_eq!(resolver.query(&intent("VIEW", Some("https://x/path")), None, false, &mut build),
            Err(super::super::domain_verification::uri_parcel::MatchError::NullPattern));
    }

    #[test]
    fn cuts_in_order() {
        let r = resolver();
        // A full type: its own, then its base's wildcards, then `*`'s.
        assert_eq!(
            query(&r, &intent("VIEW", None), Some("image/png"), false),
            ["a", "b", "c"]
        );
        // A partial type matches by base.
        // (f wants content: data.)
        assert_eq!(
            query(&r, &intent("VIEW", None), Some("image/*"), false),
            ["a", "b", "c"]
        );
        // Data with a type: the types' cuts, then the scheme's.
        assert_eq!(
            query(
                &r,
                &intent("VIEW", Some("content://x/y")),
                Some("image/png"),
                false
            ),
            ["a", "f", "b", "c", "f"]
        );
        // No type and no scheme: the action's untyped filters.
        assert_eq!(query(&r, &intent("VIEW", None), None, false), ["e"]);
        assert_eq!(
            query(&r, &intent("VIEW", Some("https://h/")), None, false),
            ["d"]
        );
        // `*/*` with an action: the typed filters of the action.
        assert_eq!(
            query(&r, &intent("VIEW", None), Some("*/*"), false),
            ["a", "b", "c"]
        );
        // CATEGORY_DEFAULT only.
        assert_eq!(
            query(&r, &intent("VIEW", None), Some("image/png"), true),
            ["a", "b"]
        );
    }

    #[test]
    fn package_stopped_and_once() {
        let r = resolver();
        let mut i = intent("VIEW", None);
        i.package = Some("b".into());
        assert_eq!(query(&r, &i, Some("image/png"), false), ["b"]);
        let mut i = intent("VIEW", None);
        i.flags = super::super::intent::FLAG_EXCLUDE_STOPPED_PACKAGES;
        let mut b = Names {
            once: false,
            stopped: &["a"],
        };
        assert_eq!(r.query(&i, Some("image/png"), false, &mut b).unwrap(), ["b", "c"]);
        // A filter in two cuts is kept once.
        let mut r = IntentResolver::default();
        let mut f = filter(&["VIEW"], &["image/png"], &["content"], false);
        f.add_data_path(PatternMatcher::new("/", 1).unwrap());
        r.add(F("x", f));
        let i = intent("VIEW", Some("content://h/p"));
        let mut b = Names {
            once: true,
            stopped: &[],
        };
        assert_eq!(r.query(&i, Some("image/png"), false, &mut b).unwrap(), ["x"]);
        let mut b = Names {
            once: false,
            stopped: &[],
        };
        assert_eq!(r.query(&i, Some("image/png"), false, &mut b).unwrap(), ["x", "x"]);
    }

    #[test]
    fn from_lists() {
        let r = resolver();
        let lists = [
            vec![&r.entries()[4]],
            vec![&r.entries()[3], &r.entries()[0]],
        ];
        let mut b = Names {
            once: false,
            stopped: &[],
        };
        let got = query_from_list(lists, &intent("VIEW", None), None, false, &mut b).unwrap();
        assert_eq!(got, ["e"]);
    }
}
