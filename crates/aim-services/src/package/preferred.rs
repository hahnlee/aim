//! A user's preferred and persistent preferred activities
//! (`PreferredActivity`, `PreferredComponent`, `PersistentPreferredActivity`
//! and their `IntentResolver`s at `android-16.0.0_r1`), read from the XML
//! the original writes them in: `getPreferredActivityBackup`'s
//! `<preferred-activities>` and package-restrictions.xml's
//! `<persistent-preferred-activities>` and `<crossProfile-intent-filters>`.
//! Owner mutations and typed XML writers keep resolver registration order
//! distinct from the original identity-hashed filter iteration order.

pub mod actions;
pub mod defaults;
mod double;
pub mod registry;
pub mod records;
mod serialization;

use aim_android_xml::{Element, Node, Value};

use super::intent::{ComponentName, Intent};
use super::intent_filter::IntentFilter;
use super::intent_resolver::{Build, Entry, IntentResolver};

/// `ComponentName.unflattenFromString`: `package/class`, a class starting
/// with `.` relative to the package.
pub fn unflatten(s: &str) -> Option<ComponentName> {
    let (package, class) = s.split_once('/')?;
    if class.is_empty() {
        return None;
    }
    let class = if class.starts_with('.') {
        format!("{package}{class}")
    } else {
        class.to_owned()
    };
    Some(ComponentName {
        package: package.to_owned(),
        class,
    })
}

/// `PreferredActivity`: a filter and the component chosen for it.
#[derive(Clone, Debug, PartialEq)]
pub struct PreferredActivity {
    pub filter: IntentFilter,
    pub component: ComponentName,
    /// The match quality of the choice (`MATCH_CATEGORY_*`).
    pub match_: i32,
    pub always: bool,
    /// The components offered when the choice was made.
    pub set: Option<Vec<ComponentName>>,
}

impl Entry for PreferredActivity {
    fn filter(&self) -> &IntentFilter {
        &self.filter
    }
    fn package(&self) -> &str {
        &self.component.package
    }
}

/// `PersistentPreferredActivity`.
#[derive(Clone, Debug, PartialEq)]
pub struct PersistentPreferredActivity {
    pub filter: IntentFilter,
    pub component: ComponentName,
    pub set_by_dpm: bool,
}

impl Entry for PersistentPreferredActivity {
    fn filter(&self) -> &IntentFilter {
        &self.filter
    }
    fn package(&self) -> &str {
        &self.component.package
    }
}

/// Deferred removal effects; native disk/registry publication has already
/// completed. Execute the independent home/AM leaves after the install lock.
#[derive(Clone, Debug, Default)]
pub struct RemovalEffects {
    pub changed_users: Vec<i32>,
    pub broadcast_user: i32,
}

/// A user's preferred activities in resolver registration order.
/// Identity-based filter iteration is supplied separately for listings/XML.
#[derive(Clone, Debug, Default)]
pub struct Preferred {
    pub preferred_resolver_present: bool,
    pub persistent_resolver_present: bool,
    pub cross_profile_resolver_present: bool,
    pub preferred: IntentResolver<PreferredActivity>,
    pub persistent: IntentResolver<PersistentPreferredActivity>,
    pub cross_profile: IntentResolver<CrossProfileIntentFilter>,
}

/// `PreferredComponent(parser)`; `None` for an entry with a parse error,
/// which the original drops.
fn preferred_activity(item: &Element) -> Option<PreferredActivity> {
    let component = unflatten(&item.string("name")?)?;
    let match_ = item.int_hex("match").ok().flatten().unwrap_or(0);
    let count = item.int("set").ok().flatten().unwrap_or(0);
    let always = item.bool("always").ok().flatten().unwrap_or(true);
    let mut set = Vec::new();
    let mut filter = IntentFilter::default();
    for c in item.children() {
        match c.name.as_str() {
            "set" => set.push(unflatten(&c.string("name")?)?),
            "filter" => filter = parse_filter(c)?,
            _ => {}
        }
    }
    if set.len() != count.max(0) as usize {
        return None;
    }
    Some(PreferredActivity {
        filter,
        component,
        match_,
        always,
        set: (count > 0).then_some(set),
    })
}

fn parse_filter(element: &Element) -> Option<IntentFilter> {
    let mut effective = element.clone();
    let mut skip_after_extras = false;
    effective.content.retain(|node| {
        let Node::Element(child) = node else {
            return true;
        };
        if skip_after_extras {
            skip_after_extras = false;
            return false;
        }
        if child.name == "extras" {
            skip_after_extras = true;
        }
        true
    });
    let mut filter = IntentFilter::parse(&effective);
    if let Some(extras) = effective
        .children()
        .filter(|child| child.name == "extras")
        .last()
    {
        filter.extras = Some(
            super::restrictions::persistable::Bundle::restore(extras)
                .ok()?
                .parcel()
                .ok()?
                .data()
                .to_vec(),
        );
    }
    Some(filter)
}

// readFromXml calls skipCurrentTag after restoreFromXml has already consumed
// ENDextras. With no following filter child it consumes ENDfilter, and the
// enclosing constructor then consumes the rest of its containing item list.
fn consumes_item_list(item: &Element) -> bool {
    let Some(filter) = item.children().find(|child| child.name == "filter") else {
        return false;
    };
    let mut last = None;
    let mut skip = false;
    for child in filter.children() {
        if skip {
            skip = false;
            continue;
        }
        last = Some(child.name.as_str());
        skip = child.name == "extras";
    }
    last == Some("extras") && skip
}

/// Reading a valid preferred activity creates a resolver, even if it is later
/// emptied. Empty or invalid-only documents do not create one.
pub(crate) fn has_preferred_resolver(root: &Element) -> bool {
    items(root, "preferred-activities")
        .into_iter()
        .any(|item| preferred_activity(item).is_some())
}

/// Settings.clearPackagePreferredActivities for one user's saved resolver.
/// A named package clears only always choices; null clears every choice.
/// Persistent choices, candidate sets and unrelated XML remain untouched.
pub(crate) fn clear_package_document(root: &mut Element, package: Option<&str>) -> bool {
    let mut changed = false;
    for node in &mut root.content {
        let Node::Element(list) = node else { continue };
        if list.name != "preferred-activities" {
            continue;
        }
        list.content.retain(|node| {
            let Node::Element(item) = node else {
                return true;
            };
            let remove = item.name == "item"
                && preferred_activity(item).is_some_and(|activity| {
                    package.is_none_or(|name| activity.always && activity.component.package == name)
                });
            changed |= remove;
            !remove
        });
    }
    changed
}

fn persistent_preferred_activity(item: &Element) -> Option<PersistentPreferredActivity> {
    Some(PersistentPreferredActivity {
        component: unflatten(&item.string("name")?)?,
        set_by_dpm: item.bool("set-by-dpm").ok().flatten().unwrap_or(false),
        filter: item
            .children()
            .find(|c| c.name == "filter")
            .map(parse_filter)
            .unwrap_or_else(|| Some(IntentFilter::default()))?,
    })
}

/// Settings and backup readers enter only their direct named sections;
/// unknown nested sections are skipped rather than searched recursively.
fn items<'a>(element: &'a Element, name: &str) -> Vec<&'a Element> {
    if element.name == name {
        return element
            .children()
            .filter(|child| child.name == "item")
            .collect();
    }
    element
        .children()
        .filter(|section| section.name == name)
        .flat_map(|section| section.children().filter(|child| child.name == "item"))
        .collect()
}

impl Preferred {
    pub fn preferred_dump_xml(&self, order: &[usize], full: bool) -> Result<Vec<u8>, String> {
        serialization::fast_xml(&self.preferred_document(order, full)?)
    }
    /// From the backup of the preferred activities and package
    /// restrictions; either may be missing or unreadable (then none of
    /// its activities).
    pub fn parse(backup: Option<&[u8]>, restrictions: Option<&[u8]>) -> Preferred {
        let mut p = Preferred::default();
        let read = |b: Option<&[u8]>| b.and_then(|b| aim_android_xml::read(b).ok());
        if let Some(root) = read(backup) {
            for item in items(&root, "preferred-activities") {
                if let Some(pa) = preferred_activity(item) {
                    p.preferred_resolver_present = true;
                    if p.should_add_restored_preferred(&pa) {
                        p.preferred.add(pa);
                    }
                }
                if consumes_item_list(item) {
                    break;
                }
            }
        }
        if let Some(root) = read(restrictions) {
            for item in items(&root, "persistent-preferred-activities") {
                if let Some(ppa) = persistent_preferred_activity(item) {
                    p.persistent_resolver_present = true;
                    p.persistent.add(ppa);
                }
                if consumes_item_list(item) {
                    break;
                }
            }
        }
        if let Some(root) = read(restrictions) {
            for item in items(&root, "crossProfile-intent-filters") {
                if let Some(filter) = CrossProfileIntentFilter::parse(item) {
                    p.cross_profile_resolver_present = true;
                    p.cross_profile.add(filter);
                }
                if consumes_item_list(item) {
                    break;
                }
            }
        }
        p
    }
}

/// The entries an `IntentResolver` of the default kind returns (the
/// filters themselves), sorted by priority as its `sortResults`.
struct Itself;

impl<E: Entry + Clone> Build<E, E> for Itself {
    fn result(&mut self, e: &E, _: i32) -> Option<E> {
        Some(e.clone())
    }
}

/// `queryIntent` of a resolver whose results are its filters.
pub fn query<E: Entry + Clone>(
    r: &IntentResolver<E>,
    intent: &Intent,
    resolved_type: Option<&str>,
    default_only: bool,
) -> std::result::Result<Vec<E>, super::domain_verification::uri_parcel::MatchError> {
    let mut list = r.query(intent, resolved_type, default_only, &mut Itself)?;
    list.sort_by(|a, b| b.filter().priority.cmp(&a.filter().priority));
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BACKUP: &[u8] = br#"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>
<preferred-backup>
<preferred-activities>
<item name="com.example.browser/.Main" match="200000" always="true" set="2">
<set name="com.example.browser/.Main" />
<set name="com.other/com.other.View" />
<filter>
<action name="android.intent.action.VIEW" />
<cat name="android.intent.category.DEFAULT" />
<scheme name="https" />
</filter>
</item>
<item name="com.broken/.X" set="1">
<filter />
</item>
</preferred-activities>
</preferred-backup>"#;

    const RESTRICTIONS: &[u8] = br#"<package-restrictions>
<persistent-preferred-activities>
<item name="com.example.camera/com.example.camera.Shoot" set-by-dpm="true">
<filter>
<action name="android.media.action.IMAGE_CAPTURE" />
<cat name="android.intent.category.DEFAULT" />
</filter>
</item>
</persistent-preferred-activities>
</package-restrictions>"#;

    #[test]
    fn reads_and_queries() {
        let p = Preferred::parse(Some(BACKUP), Some(RESTRICTIONS));
        // The broken item (one set entry declared, none given) is dropped.
        assert_eq!(p.preferred.entries().len(), 1);
        let pa = &p.preferred.entries()[0];
        assert_eq!(pa.component.class, "com.example.browser.Main");
        assert_eq!((pa.match_, pa.always), (0x200000, true));
        assert_eq!(pa.set.as_ref().unwrap()[1].class, "com.other.View");
        let view = Intent {
            action: Some("android.intent.action.VIEW".into()),
            data: Some(super::super::uri::Uri::parse("https://example.com/")),
            ..Intent::default()
        };
        assert_eq!(query(&p.preferred, &view, None, true).unwrap().len(), 1);
        let ppa = &p.persistent.entries()[0];
        assert!(ppa.set_by_dpm);
        let capture = Intent {
            action: Some("android.media.action.IMAGE_CAPTURE".into()),
            ..Intent::default()
        };
        assert_eq!(query(&p.persistent, &capture, None, true).unwrap().len(), 1);
        assert!(
            Preferred::parse(None, Some(b"not xml"))
                .persistent
                .entries()
                .is_empty()
        );
    }

    #[test]
    fn unflattens() {
        assert_eq!(unflatten("a/.B").unwrap().class, "a.B");
        assert_eq!(unflatten("a/b.C").unwrap().class, "b.C");
        assert!(unflatten("a/").is_none() && unflatten("a").is_none());
    }
}

/// Persisted cross-profile routing filter. Permission and access-control
/// enforcement belongs to the calling owner, before it edits this state.
#[derive(Clone, Debug, PartialEq)]
pub struct CrossProfileIntentFilter {
    pub filter: IntentFilter,
    pub target_user_id: i32,
    pub owner_package: String,
    pub flags: i32,
    pub access_control: i32,
}

impl Entry for CrossProfileIntentFilter {
    fn filter(&self) -> &IntentFilter {
        &self.filter
    }
    fn package(&self) -> &str {
        &self.owner_package
    }
}

impl CrossProfileIntentFilter {
    pub fn parse(item: &Element) -> Option<Self> {
        Some(Self {
            filter: item
                .children()
                .find(|c| c.name == "filter")
                .map(parse_filter)
                .unwrap_or_else(|| Some(IntentFilter::default()))?,
            target_user_id: item.int("targetUserId").ok().flatten().unwrap_or(-10000),
            owner_package: item.string("ownerPackage").unwrap_or_default().into_owned(),
            flags: item.int("flags").ok().flatten().unwrap_or(0),
            access_control: item.int("accessControl").ok().flatten().unwrap_or(0),
        })
    }
    pub fn equals_ignore_filter(&self, other: &Self) -> bool {
        self.target_user_id == other.target_user_id
            && self.owner_package == other.owner_package
            && self.flags == other.flags
            && self.access_control == other.access_control
    }
}

/// `IntentFilter.filterEquals` ignores priority, extras, URI-relative groups,
/// autoVerify and MIME groups. Null and empty collections compare equally.
pub fn filter_equals(a: &IntentFilter, b: &IntentFilter) -> bool {
    fn same<T: PartialEq>(a: &[T], b: &[T]) -> bool {
        a.len() == b.len() && a.iter().all(|v| b.contains(v))
    }
    fn list<T: PartialEq>(a: &Option<Vec<T>>, b: &Option<Vec<T>>) -> bool {
        same(
            a.as_deref().unwrap_or_default(),
            b.as_deref().unwrap_or_default(),
        )
    }
    same(&a.actions, &b.actions)
        && list(&a.categories, &b.categories)
        && list(&a.types, &b.types)
        && list(&a.schemes, &b.schemes)
        && {
            let a = a.authorities.as_deref().unwrap_or_default();
            let b = b.authorities.as_deref().unwrap_or_default();
            a.len() == b.len()
                && a.iter().all(|a| {
                    b.iter()
                        .any(|b| a.host == b.host && a.wild == b.wild && a.port == b.port)
                })
        }
        && list(&a.paths, &b.paths)
        && list(&a.ssps, &b.ssps)
}

impl PreferredActivity {
    /// PreferredComponent's constructor masks match adjustments.
    pub fn new(
        filter: IntentFilter,
        match_: i32,
        set: Option<Vec<ComponentName>>,
        component: ComponentName,
        always: bool,
    ) -> Self {
        Self {
            filter,
            match_: match_ & super::intent_filter::MATCH_CATEGORY_MASK,
            set,
            component,
            always,
        }
    }

    /// PreferredComponent.sameSet(ComponentName[]), including duplicate entries.
    pub fn same_set(&self, components: &[ComponentName]) -> bool {
        self.set.as_ref().is_some_and(|set| {
            components.len() == set.len()
                && components.iter().all(|component| set.contains(component))
        })
    }

    /// PreferredComponent.isSuperset, with the caller's resolved setup wizard.
    pub fn is_superset(
        &self,
        components: Option<&[ComponentName]>,
        excluded_setup_wizard: Option<&str>,
    ) -> bool {
        let Some(set) = &self.set else {
            return components.is_none();
        };
        let Some(components) = components else {
            return true;
        };
        if excluded_setup_wizard.is_none() && set.len() < components.len() {
            return false;
        }
        components.iter().all(|component| {
            excluded_setup_wizard == Some(component.package.as_str()) || set.contains(component)
        })
    }

    pub fn discard_obsolete_components(
        &self,
        components: Option<&[ComponentName]>,
    ) -> Vec<ComponentName> {
        let Some(set) = &self.set else {
            return Vec::new();
        };
        components
            .unwrap_or_default()
            .iter()
            .filter(|component| set.contains(component))
            .cloned()
            .collect()
    }
}

fn retain<E: Entry + Clone>(
    resolver: &mut IntentResolver<E>,
    mut keep: impl FnMut(&E) -> bool,
) -> bool {
    let entries = resolver.entries().to_vec();
    let mut changed = false;
    *resolver = IntentResolver::default();
    for entry in entries {
        if keep(&entry) {
            resolver.add(entry);
        } else {
            changed = true;
        }
    }
    changed
}

impl Preferred {
    /// Settings.readPreferredActivitiesLPw uses this load-only rule. Public
    /// addPreferredActivity deliberately retains distinct live registrations.
    fn should_add_restored_preferred(&self, activity: &PreferredActivity) -> bool {
        let matching = self.preferred.entries().iter()
            .filter(|old| filter_equals(&old.filter, &activity.filter)).collect::<Vec<_>>();
        if matching.is_empty() { return true; }
        if !activity.always { return false; }
        !matching.into_iter().any(|old| old.always
            && old.match_ == (activity.match_ & super::intent_filter::MATCH_CATEGORY_MASK)
            && old.component == activity.component
            && old.set.as_ref().zip(activity.set.as_ref()).is_some_and(|(old, new)| old == new))
    }

    /// Adds a choice after permission checks. Empty-action filters are ignored
    /// exactly as PreferredActivityHelper.addPreferredActivity does.
    pub fn add_preferred(&mut self, activity: PreferredActivity, remove_existing: bool) -> bool {
        if activity.filter.actions.is_empty() {
            return false;
        }
        if remove_existing {
            retain(&mut self.preferred, |old| {
                !filter_equals(&old.filter, &activity.filter)
            });
        }
        self.preferred_resolver_present = true;
        self.preferred.add(activity);
        true
    }

    /// Validates the replacement shape before changing existing state.
    pub fn replace_preferred(&mut self, activity: PreferredActivity) -> Result<bool, &'static str> {
        let filter = &activity.filter;
        if filter.actions.len() != 1 {
            return Err("replacePreferredActivity expects filter to have only 1 action.");
        }
        if !filter.authorities.as_deref().unwrap_or_default().is_empty()
            || !filter.paths.as_deref().unwrap_or_default().is_empty()
            || !filter.types.as_deref().unwrap_or_default().is_empty()
            || filter.schemes.as_deref().unwrap_or_default().len() > 1
        {
            return Err(
                "replacePreferredActivity expects filter to have no data authorities, paths, or types; and at most one scheme.",
            );
        }
        let existing: Vec<_> = self
            .preferred
            .entries()
            .iter()
            .filter(|old| filter_equals(&old.filter, filter))
            .collect();
        if existing.len() == 1 {
            let old = existing[0];
            if old.always
                && old.component == activity.component
                && old.match_ == activity.match_
                && activity.set.as_ref().is_some_and(|set| old.same_set(set))
            {
                return Ok(false);
            }
        }
        retain(&mut self.preferred, |old| {
            !filter_equals(&old.filter, filter)
        });
        Ok(self.add_preferred(activity, false))
    }

    pub fn clear_preferred(&mut self, package: Option<&str>) -> bool {
        retain(&mut self.preferred, |old| {
            !package.is_none_or(|package| old.always && old.component.package == package)
        })
    }

    pub fn add_persistent(&mut self, activity: PersistentPreferredActivity) -> bool {
        if activity.filter.actions.is_empty() {
            return false;
        }
        self.persistent_resolver_present = true;
        self.persistent.add(activity);
        true
    }

    pub fn clear_persistent(&mut self, package: &str) -> bool {
        retain(&mut self.persistent, |old| old.component.package != package)
    }

    /// Caller has already enforced owner rights, cross-user permission and
    /// UserManager access policy; the supplied access_control is that owner's.
    pub fn add_cross_profile(&mut self, activity: CrossProfileIntentFilter) -> bool {
        if activity.filter.actions.is_empty() {
            return false;
        }
        if self.cross_profile.entries().iter().any(|old| {
            filter_equals(&old.filter, &activity.filter) && old.equals_ignore_filter(&activity)
        }) {
            return false;
        }
        self.cross_profile_resolver_present = true;
        self.cross_profile.add(activity);
        true
    }

    /// Removes the first matching object in the original ArraySet order.
    /// Global permissions and target access must be checked before this call.
    pub fn remove_cross_profile(
        &mut self,
        filter: &IntentFilter,
        owner_package: &str,
        target_user_id: i32,
        flags: i32,
        order: &[usize],
    ) -> Result<bool, &'static str> {
        validate_order(order, self.cross_profile.entries().len())?;
        let index = order.iter().copied().find(|&index| {
            let old = &self.cross_profile.entries()[index];
            filter_equals(&old.filter, filter)
                && old.owner_package == owner_package
                && old.target_user_id == target_user_id
                && old.flags == flags
        });
        let Some(remove) = index else {
            return Ok(false);
        };
        let mut index = 0;
        Ok(retain(&mut self.cross_profile, |_| {
            let keep = index != remove;
            index += 1;
            keep
        }))
    }

    /// Settings.removeCrossProfileIntentFiltersLPw internal user cleanup.
    /// The caller separately drops the removed user's source resolver.
    pub fn remove_cross_profile_target(&mut self, user: i32) -> bool {
        retain(&mut self.cross_profile, |old| old.target_user_id != user)
    }

    /// Clear uses UserManager's current access policy for each target; denied
    /// targets remain installed, as CrossProfileIntentFilterHelper does.
    pub fn clear_cross_profile(
        &mut self,
        owner_package: &str,
        target_user_id: Option<i32>,
        mut accessible: impl FnMut(i32) -> bool,
    ) -> bool {
        retain(&mut self.cross_profile, |old| {
            !(old.owner_package == owner_package
                && target_user_id.is_none_or(|target| old.target_user_id == target)
                && accessible(old.target_user_id))
        })
    }

    /// Native callers supply their actual resolver filterIterator order.
    /// An imported original ArraySet order must be carried explicitly: object
    /// identity hashes cannot be reconstructed from component names or XML.
    pub fn preferred_activities(
        &self,
        iteration_order: &[usize],
        package: Option<&str>,
        mut visible: impl FnMut(&str) -> bool,
    ) -> Result<Vec<&PreferredActivity>, &'static str> {
        let entries = self.preferred.entries();
        validate_order(iteration_order, entries.len())?;
        Ok(iteration_order
            .iter()
            .map(|&index| &entries[index])
            .filter(|activity| {
                package.is_none_or(|name| activity.always && activity.component.package == name)
                    && visible(&activity.component.package)
            })
            .collect())
    }
}

fn validate_order(order: &[usize], count: usize) -> Result<(), &'static str> {
    if order.len() != count {
        return Err("resolver identity iteration order is unavailable");
    }
    let mut seen = vec![false; count];
    for &index in order {
        let Some(slot) = seen.get_mut(index) else {
            return Err("invalid resolver iteration index");
        };
        if std::mem::replace(slot, true) {
            return Err("duplicate resolver iteration index");
        }
    }
    Ok(())
}

fn element(name: &str) -> Element {
    Element {
        name: name.into(),
        attrs: Vec::new(),
        content: Vec::new(),
    }
}
fn attr(element: &mut Element, name: &str, value: Value) {
    element.attrs.push((name.into(), value));
}
fn child(parent: &mut Element, child: Element) {
    parent.content.push(Node::Element(child));
}
fn named(parent: &mut Element, tag: &str, name: &str) {
    let mut entry = element(tag);
    attr(&mut entry, "name", Value::String(name.into()));
    child(parent, entry);
}
fn short_component(component: &ComponentName) -> String {
    let class = component
        .class
        .strip_prefix(&component.package)
        .filter(|class| class.starts_with('.'))
        .unwrap_or(&component.class);
    format!("{}/{class}", component.package)
}

/// The typed document emitted by IntentFilter.writeToXml. Extras require the
/// actual PersistableBundle owner; parcel bytes cannot stand in for XML.
pub fn filter_document(filter: &IntentFilter) -> Result<Element, String> {
    let mut document = element("filter");
    if filter.auto_verify {
        attr(&mut document, "autoVerify", Value::String("true".into()));
    }
    for action in &filter.actions {
        named(&mut document, "action", action);
    }
    for category in filter.categories.as_deref().unwrap_or_default() {
        named(&mut document, "cat", category);
    }
    if let Some(static_types) = &filter.static_types {
        let types = filter.types.as_deref().unwrap_or_default();
        let mut index = 0;
        for static_type in static_types {
            while types.get(index).is_some_and(|ty| ty != static_type) {
                named(&mut document, "type", &xml_type(&types[index]));
                index += 1;
            }
            if types.get(index) != Some(static_type) {
                return Err("static MIME types are not a subsequence of data types".into());
            }
            named(&mut document, "staticType", &xml_type(static_type));
            index += 1;
        }
        for ty in &types[index..] {
            named(&mut document, "type", &xml_type(ty));
        }
    }
    for group in filter.mime_groups.as_deref().unwrap_or_default() {
        named(&mut document, "group", group);
    }
    for scheme in filter.schemes.as_deref().unwrap_or_default() {
        named(&mut document, "scheme", scheme);
    }
    for (tag, patterns) in [("ssp", &filter.ssps), ("path", &filter.paths)] {
        // Authorities precede paths in the original's writer.
        if tag == "path" {
            for authority in filter.authorities.as_deref().unwrap_or_default() {
                let mut entry = element("auth");
                attr(
                    &mut entry,
                    "host",
                    Value::String(authority.orig_host.clone()),
                );
                if authority.port >= 0 {
                    attr(
                        &mut entry,
                        "port",
                        Value::String(authority.port.to_string()),
                    );
                }
                child(&mut document, entry);
            }
        }
        for pattern in patterns.as_deref().unwrap_or_default() {
            let mut entry = element(tag);
            if let Some(attribute) =
                ["literal", "prefix", "sglob", "aglob", "suffix"].get(pattern.kind as usize)
            {
                attr(
                    &mut entry,
                    attribute,
                    Value::String(pattern.pattern.clone()),
                );
            }
            child(&mut document, entry);
        }
    }
    if let Some(extras) = &filter.extras {
        child(&mut document, serialization::extras(extras)?);
    }
    for group in filter
        .uri_relative_filter_groups
        .as_deref()
        .unwrap_or_default()
    {
        let mut entry = element("uriRelativeFilterGroup");
        attr(&mut entry, "allow", Value::String(group.action.to_string()));
        for relative in &group.filters {
            let mut item = element("uriRelativeFilter");
            attr(
                &mut item,
                "part",
                Value::String(relative.uri_part.to_string()),
            );
            attr(
                &mut item,
                "pattern",
                Value::String(relative.pattern_type.to_string()),
            );
            attr(
                &mut item,
                "filter",
                relative
                    .filter
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
            child(&mut entry, item);
        }
        child(&mut document, entry);
    }
    Ok(document)
}
fn xml_type(ty: &str) -> String {
    if ty.contains('/') {
        ty.into()
    } else {
        format!("{ty}/*")
    }
}

impl Preferred {
    /// Original backup wrapper and FastXmlSerializer byte layout. A caller
    /// must enforce the original exact SYSTEM_UID check before invoking it.
    pub fn preferred_backup(&self, order: &[usize]) -> Result<Vec<u8>, String> {
        let mut root = element("pa");
        child(&mut root, self.preferred_document(order, true)?);
        serialization::fast_xml(&root)
    }

    pub fn preferred_document(&self, order: &[usize], full: bool) -> Result<Element, String> {
        validate_order(order, self.preferred.entries().len()).map_err(str::to_owned)?;
        let mut document = element("preferred-activities");
        for &index in order {
            let activity = &self.preferred.entries()[index];
            let mut item = element("item");
            attr(
                &mut item,
                "name",
                Value::String(short_component(&activity.component)),
            );
            if full {
                if activity.match_ != 0 {
                    attr(&mut item, "match", Value::IntHex(activity.match_));
                }
                attr(&mut item, "always", Value::Bool(activity.always));
                let set = activity.set.as_deref().unwrap_or_default();
                let count = i32::try_from(set.len())
                    .map_err(|_| "preferred component set exceeds XML range")?;
                attr(&mut item, "set", Value::Int(count));
                for component in set {
                    named(&mut item, "set", &short_component(component));
                }
            }
            child(&mut item, filter_document(&activity.filter)?);
            child(&mut document, item);
        }
        Ok(document)
    }

    pub fn persistent_document(&self, order: &[usize]) -> Result<Element, String> {
        validate_order(order, self.persistent.entries().len()).map_err(str::to_owned)?;
        let mut document = element("persistent-preferred-activities");
        for &index in order {
            let activity = &self.persistent.entries()[index];
            let mut item = element("item");
            attr(
                &mut item,
                "name",
                Value::String(short_component(&activity.component)),
            );
            attr(&mut item, "set-by-dpm", Value::Bool(activity.set_by_dpm));
            child(&mut item, filter_document(&activity.filter)?);
            child(&mut document, item);
        }
        Ok(document)
    }

    pub fn cross_profile_document(&self, order: &[usize]) -> Result<Element, String> {
        validate_order(order, self.cross_profile.entries().len()).map_err(str::to_owned)?;
        let mut document = element("crossProfile-intent-filters");
        for &index in order {
            let activity = &self.cross_profile.entries()[index];
            let mut item = element("item");
            attr(
                &mut item,
                "targetUserId",
                Value::Int(activity.target_user_id),
            );
            attr(&mut item, "flags", Value::Int(activity.flags));
            attr(
                &mut item,
                "ownerPackage",
                Value::String(activity.owner_package.clone()),
            );
            attr(
                &mut item,
                "accessControl",
                Value::Int(activity.access_control),
            );
            child(&mut item, filter_document(&activity.filter)?);
            child(&mut document, item);
        }
        Ok(document)
    }
}

#[cfg(test)]
mod owner_tests {
    use super::super::intent_filter::{PatternMatcher, UriRelativeFilterGroup};
    use super::*;

    fn filter() -> IntentFilter {
        let mut filter = IntentFilter::default();
        filter.add_action("view");
        filter.add_category("android.intent.category.DEFAULT");
        filter
    }
    fn choice(package: &str, always: bool) -> PreferredActivity {
        let component = unflatten(&format!("{package}/.Main")).unwrap();
        PreferredActivity::new(
            filter(),
            0x108001,
            Some(vec![component.clone()]),
            component,
            always,
        )
    }

    #[test]
    fn settings_reload_drops_duplicate_live_choices_using_original_candidate_order() {
        let mut state = Preferred::default();
        let mut first = choice("one", true);
        first.set = Some(vec![unflatten("one/.Main").unwrap(), unflatten("two/.Main").unwrap()]);
        state.add_preferred(first.clone(), false);
        state.add_preferred(first.clone(), false);
        assert_eq!(state.preferred.entries().len(), 2);
        let document = state.preferred_document(&[1, 0], true).unwrap();
        let bytes = aim_android_xml::abx::write(&document).unwrap();
        let restored = Preferred::parse(Some(&bytes), None);
        assert_eq!(restored.preferred.entries().len(), 1);
        assert_eq!(restored.preferred.entries()[0], first);
        let mut reordered = first.clone(); reordered.set.as_mut().unwrap().reverse();
        assert!(restored.should_add_restored_preferred(&reordered));
        let mut other_choice = first.clone(); other_choice.component = unflatten("two/.Main").unwrap();
        assert!(restored.should_add_restored_preferred(&other_choice));
        let mut different_match = first.clone(); different_match.match_ = 0x200000;
        assert!(restored.should_add_restored_preferred(&different_match));
        let mut no_candidates = first.clone(); no_candidates.set = None;
        assert!(restored.should_add_restored_preferred(&no_candidates));
        let mut last_chosen = first.clone(); last_chosen.always = false;
        assert!(!restored.should_add_restored_preferred(&last_chosen));
        let mut distinct_filter = last_chosen; distinct_filter.filter.add_action("different");
        assert!(restored.should_add_restored_preferred(&distinct_filter));
    }

    #[test]
    fn replacement_is_atomic_and_suppresses_identical_choice() {
        let mut state = Preferred::default();
        assert!(state.add_preferred(choice("one", true), false));
        assert!(!state.replace_preferred(choice("one", true)).unwrap());
        let mut invalid = choice("two", true);
        invalid.filter.add_action("second");
        assert!(state.replace_preferred(invalid).is_err());
        assert_eq!(state.preferred.entries()[0].component.package, "one");
        assert!(state.replace_preferred(choice("two", true)).unwrap());
        assert_eq!(state.preferred.entries().len(), 1);
        assert_eq!(state.preferred.entries()[0].match_, 0x100000);
        assert_eq!(state.preferred.entries()[0].component.package, "two");
    }

    #[test]
    fn preferred_query_and_clear_distinguish_last_chosen() {
        let mut state = Preferred::default();
        state.add_preferred(choice("one", true), false);
        state.add_preferred(choice("one", false), false);
        state.add_preferred(choice("two", true), false);
        let output = state
            .preferred_activities(&[2, 1, 0], None, |name| name != "two")
            .unwrap();
        assert_eq!(output.len(), 2);
        assert!(!output[0].always);
        assert_eq!(
            state
                .preferred_activities(&[2, 1, 0], Some("one"), |_| true)
                .unwrap()
                .len(),
            1
        );
        assert!(
            state
                .preferred_activities(&[0, 0, 2], None, |_| true)
                .is_err()
        );
        assert!(state.clear_preferred(Some("one")));
        assert_eq!(state.preferred.entries().len(), 2);
        assert!(!state.preferred.entries()[0].always);
        assert!(state.clear_preferred(None));
        assert!(state.preferred.entries().is_empty());
    }

    #[test]
    fn component_sets_keep_original_duplicate_and_query_order_semantics() {
        let mut activity = choice("one", true);
        let one = activity.component.clone();
        let two = unflatten("two/.Main").unwrap();
        activity.set = Some(vec![one.clone(), two.clone()]);
        assert!(activity.same_set(&[one.clone(), one.clone()]));
        assert!(!activity.same_set(std::slice::from_ref(&one)));
        assert!(activity.is_superset(Some(std::slice::from_ref(&two)), None));
        assert!(!activity.is_superset(Some(&[unflatten("setup/.Main").unwrap()]), None));
        assert!(activity.is_superset(Some(&[unflatten("setup/.Main").unwrap()]), Some("setup")));
        assert_eq!(
            activity.discard_obsolete_components(Some(&[two.clone(), one.clone(), two.clone()])),
            vec![two.clone(), one, two]
        );
    }

    #[test]
    fn equality_ignores_non_matching_fields_and_collection_order() {
        let mut a = filter();
        a.add_action("edit");
        let mut b = a.clone();
        b.actions.reverse();
        b.priority = 999;
        b.auto_verify = true;
        b.mime_groups = Some(vec!["different".into()]);
        b.uri_relative_filter_groups = Some(vec![UriRelativeFilterGroup::new(1)]);
        assert!(filter_equals(&a, &b));
        b.add_data_path(PatternMatcher::new("/foo", 0).unwrap());
        assert!(!filter_equals(&a, &b));
    }

    #[test]
    fn documents_round_trip_filter_semantics_and_explicit_identity_order() {
        let mut state = Preferred::default();
        let mut first = choice("one", true);
        first.filter.add_data_scheme("https");
        first
            .filter
            .add_data_authority("*.example.com", Some("443"));
        first
            .filter
            .add_data_path(PatternMatcher::new("/foo", 1).unwrap());
        first.filter.add_data_type("text/*").unwrap();
        let mut group = UriRelativeFilterGroup::new(1);
        group.add(1, 0, "q=yes");
        first.filter.add_uri_relative_filter_group(group);
        state.add_preferred(first.clone(), false);
        state.add_preferred(choice("two", false), false);
        let document = state.preferred_document(&[1, 0], true).unwrap();
        let bytes = aim_android_xml::abx::write(&document).unwrap();
        let reloaded = Preferred::parse(Some(&bytes), None);
        assert_eq!(reloaded.preferred.entries()[0].component.package, "two");
        assert_eq!(reloaded.preferred.entries()[1], first);
        assert!(state.preferred_document(&[], true).is_err());
        let mut extras = filter();
        extras.extras = Some(vec![0]);
        assert!(filter_document(&extras).is_err());
    }

    #[test]
    fn cross_profile_mutations_keep_access_and_remove_first_identity_match() {
        let mut state = Preferred::default();
        let mut activity = CrossProfileIntentFilter {
            filter: filter(),
            target_user_id: 10,
            owner_package: "policy".into(),
            flags: 8,
            access_control: 0,
        };
        assert!(state.add_cross_profile(activity.clone()));
        assert!(!state.add_cross_profile(activity.clone()));
        activity.access_control = 20;
        assert!(state.add_cross_profile(activity.clone()));
        assert!(
            state
                .remove_cross_profile(&activity.filter, "policy", 10, 8, &[1, 0])
                .unwrap()
        );
        assert_eq!(state.cross_profile.entries()[0].access_control, 0);
        activity.target_user_id = 11;
        assert!(state.add_cross_profile(activity.clone()));
        assert!(state.clear_cross_profile("policy", None, |target| target == 10));
        assert_eq!(state.cross_profile.entries()[0].target_user_id, 11);
        assert!(!state.clear_cross_profile("policy", Some(10), |_| true));
        assert!(state.clear_cross_profile("policy", None, |_| true));
    }

    #[test]
    fn persistent_choices_survive_normal_clear_and_canonical_roundtrip() {
        let mut state = Preferred::default();
        let mut empty = filter();
        empty.actions.clear();
        let component = unflatten("policy/.Choice").unwrap();
        assert!(!state.add_persistent(PersistentPreferredActivity {
            filter: empty,
            component: component.clone(),
            set_by_dpm: true
        }));
        assert!(state.add_persistent(PersistentPreferredActivity {
            filter: filter(),
            component,
            set_by_dpm: true
        }));
        assert!(!state.clear_preferred(None));
        let mut root = element("package-restrictions");
        child(&mut root, state.persistent_document(&[0]).unwrap());
        let bytes = aim_android_xml::abx::write(&root).unwrap();
        let reloaded = Preferred::parse(None, Some(&bytes));
        assert_eq!(reloaded.persistent.entries(), state.persistent.entries());
        assert!(!state.clear_persistent("other"));
        assert!(state.clear_persistent("policy"));
    }

    #[test]
    fn cross_profile_defaults_and_typed_roundtrip() {
        let state = Preferred::parse(None, Some(br#"<package-restrictions><crossProfile-intent-filters>
            <item><filter><action name="view" /></filter></item>
            <item targetUserId="10" ownerPackage="policy" flags="24" accessControl="20"><filter /></item>
            </crossProfile-intent-filters></package-restrictions>"#));
        let entries = state.cross_profile.entries();
        assert_eq!(
            (entries[0].target_user_id, entries[0].access_control),
            (-10000, 0)
        );
        assert_eq!((entries[1].target_user_id, entries[1].flags), (10, 24));
        assert!(!entries[0].equals_ignore_filter(&entries[1]));
        let mut root = element("package-restrictions");
        child(&mut root, state.cross_profile_document(&[1, 0]).unwrap());
        let reloaded = Preferred::parse(None, Some(&aim_android_xml::abx::write(&root).unwrap()));
        assert_eq!(reloaded.cross_profile.entries()[0], entries[1]);
        assert_eq!(reloaded.cross_profile.entries()[1], entries[0]);
    }
}

/// A resolver result and its original match quality, before preferred selection.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub component: ComponentName,
    pub match_: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    Remove {
        registration: usize,
    },
    Replace {
        registration: usize,
        activity: PreferredActivity,
    },
}

/// A snapshot-derived mutation plan. The disk owner must commit these edits
/// against the same snapshot generation before returning the selected result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub chosen: Option<usize>,
    pub edits: Vec<Edit>,
}

impl Preferred {
    /// Settings.systemReady removes every dangling chosen activity, including
    /// non-always records. The candidate set is not the selected component.
    pub(crate) fn dangling_selection(
        &self,
        mut contains_activity: impl FnMut(&ComponentName) -> bool,
    ) -> Selection {
        Selection {
            chosen: None,
            edits: self.preferred.entries().iter().enumerate()
                .filter(|(_, activity)| !contains_activity(&activity.component))
                .map(|(registration, _)| Edit::Remove { registration })
                .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SelectionPolicy<'a> {
    pub always: bool,
    pub remove_matches: bool,
    pub allow_set_mutation: bool,
    pub improve_home_behavior: bool,
    pub home_intent: bool,
    pub excluded_setup_wizard: Option<&'a str>,
}

impl Preferred {
    /// ComputerEngine.findPreferredActivityBody after intent/flag updates and
    /// persistent selection. Matching registration indices come from the
    /// actual resolver; same_set additionally applies install-reason policy.
    pub fn select(
        &self,
        matching: &[usize],
        candidates: &[Candidate],
        policy: SelectionPolicy<'_>,
        mut activity_exists: impl FnMut(&ComponentName) -> Result<bool, String>,
        mut same_set: impl FnMut(&PreferredActivity) -> Result<bool, String>,
    ) -> Result<Selection, String> {
        let best = candidates
            .iter()
            .map(|candidate| candidate.match_)
            .fold(0, i32::max)
            & super::intent_filter::MATCH_CATEGORY_MASK;
        let mut result = Selection::default();
        let components: Vec<_> = candidates
            .iter()
            .map(|candidate| candidate.component.clone())
            .collect();
        for &registration in matching {
            let activity = self
                .preferred
                .entries()
                .get(registration)
                .ok_or("invalid preferred resolver registration")?;
            if activity.match_ != best || policy.always && !activity.always {
                continue;
            }
            if !activity_exists(&activity.component)? {
                if policy.allow_set_mutation {
                    result.edits.push(Edit::Remove { registration });
                }
                continue;
            }
            let Some(chosen) = candidates
                .iter()
                .position(|candidate| candidate.component == activity.component)
            else {
                continue;
            };
            if policy.remove_matches && policy.allow_set_mutation {
                result.edits.push(Edit::Remove { registration });
                continue;
            }
            if policy.always && !same_set(activity)? {
                if activity.is_superset(Some(&components), policy.excluded_setup_wizard) {
                    if policy.allow_set_mutation {
                        let mut fresh = activity.clone();
                        fresh.set = Some(activity.discard_obsolete_components(Some(&components)));
                        result.edits.push(Edit::Replace {
                            registration,
                            activity: fresh,
                        });
                    }
                } else if !policy.improve_home_behavior || !policy.home_intent {
                    if policy.allow_set_mutation {
                        let mut last = activity.clone();
                        last.always = false;
                        last.set = None;
                        result.edits.push(Edit::Replace {
                            registration,
                            activity: last,
                        });
                    }
                    return Ok(result);
                }
            }
            result.chosen = Some(chosen);
            return Ok(result);
        }
        Ok(result)
    }

    /// Apply a validated snapshot plan. Replacements register as fresh objects
    /// at the end, matching removeFilter/addFilter; identity ordering is owned
    /// separately and must assign a fresh identity to each replacement.
    pub fn apply_selection(&mut self, selection: &Selection) -> Result<bool, &'static str> {
        if selection.edits.is_empty() {
            return Ok(false);
        }
        let count = self.preferred.entries().len();
        for edit in &selection.edits {
            let index = match edit {
                Edit::Remove { registration } | Edit::Replace { registration, .. } => *registration,
            };
            if index >= count {
                return Err("invalid preferred mutation registration");
            }
        }
        let old = self.preferred.entries().to_vec();
        let mut resolver = IntentResolver::default();
        for (index, activity) in old.into_iter().enumerate() {
            if !selection.edits.iter().any(|edit| match edit {
                Edit::Remove { registration } | Edit::Replace { registration, .. } => {
                    *registration == index
                }
            }) {
                resolver.add(activity);
            }
        }
        for edit in &selection.edits {
            if let Edit::Replace { activity, .. } = edit {
                resolver.add(activity.clone());
            }
        }
        self.preferred = resolver;
        Ok(true)
    }
}

impl Preferred {
    /// Query matching native registration objects without reducing identity to
    /// ComponentName. Several objects may hold equal filters and components.
    pub fn matching_registrations(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        default_only: bool,
    ) -> Result<Vec<usize>, super::domain_verification::uri_parcel::MatchError> {
        struct Indexed<'a> {
            index: usize,
            activity: &'a PreferredActivity,
        }
        impl Entry for Indexed<'_> {
            fn filter(&self) -> &IntentFilter {
                &self.activity.filter
            }
            fn package(&self) -> &str {
                &self.activity.component.package
            }
        }
        struct Indices;
        impl Build<Indexed<'_>, usize> for Indices {
            fn result(&mut self, entry: &Indexed<'_>, _: i32) -> Option<usize> {
                Some(entry.index)
            }
        }
        let mut resolver = IntentResolver::default();
        for (index, activity) in self.preferred.entries().iter().enumerate() {
            resolver.add(Indexed { index, activity });
        }
        let mut matched = resolver.query(intent, resolved_type, default_only, &mut Indices)?;
        matched.sort_by(|&a, &b| {
            self.preferred.entries()[b]
                .filter
                .priority
                .cmp(&self.preferred.entries()[a].filter.priority)
        });
        Ok(matched)
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    fn activity(package: &str, always: bool) -> PreferredActivity {
        let mut filter = IntentFilter::default();
        filter.add_action("view");
        let component = unflatten(&format!("{package}/.Main")).unwrap();
        PreferredActivity::new(
            filter,
            0x100000,
            Some(vec![component.clone()]),
            component,
            always,
        )
    }
    fn policy(always: bool) -> SelectionPolicy<'static> {
        SelectionPolicy {
            always,
            remove_matches: false,
            allow_set_mutation: true,
            improve_home_behavior: false,
            home_intent: false,
            excluded_setup_wizard: None,
        }
    }
    #[test]
    fn system_ready_sweeps_all_dangling_choices_without_using_candidate_sets() {
        let mut preferred = Preferred::default();
        let mut missing = activity("missing-always", true);
        missing.set = Some(vec![unflatten("live/.Main").unwrap()]);
        preferred.add_preferred(missing, false);
        let mut live = activity("live", false);
        live.set = Some(vec![unflatten("removed-candidate/.Main").unwrap()]);
        preferred.add_preferred(live, false);
        preferred.add_preferred(activity("missing-last", false), false);
        let selection = preferred.dangling_selection(|component| component.package == "live");
        assert_eq!(selection.edits, [Edit::Remove { registration: 0 }, Edit::Remove { registration: 2 }]);
        preferred.apply_selection(&selection).unwrap();
        assert_eq!(preferred.preferred.entries().len(), 1);
        assert_eq!(preferred.preferred.entries()[0].component.package, "live");
        assert!(!preferred.preferred.entries()[0].always);
        assert!(preferred.dangling_selection(|_| true).edits.is_empty());
    }

    #[test]
    fn last_chosen_cleans_dangling_entries_and_honors_mutation_gate() {
        let mut preferred = Preferred::default();
        preferred.add_preferred(activity("gone", true), false);
        preferred.add_preferred(activity("live", false), false);
        let candidate = Candidate {
            component: unflatten("live/.Main").unwrap(),
            match_: 0x108000,
        };
        let matched = preferred
            .matching_registrations(
                &Intent {
                    action: Some("view".into()),
                    ..Intent::default()
                },
                None,
                false,
            )
            .unwrap();
        assert_eq!(matched, [0, 1]);
        let result = preferred
            .select(
                &matched,
                std::slice::from_ref(&candidate),
                policy(false),
                |component| Ok(component.package != "gone"),
                |_| panic!("last chosen never checks the set"),
            )
            .unwrap();
        assert_eq!(result.chosen, Some(0));
        assert_eq!(result.edits, [Edit::Remove { registration: 0 }]);
        let mut frozen = policy(false);
        frozen.allow_set_mutation = false;
        let gated = preferred
            .select(
                &matched,
                &[candidate],
                frozen,
                |component| Ok(component.package != "gone"),
                |_| panic!(),
            )
            .unwrap();
        assert!(gated.edits.is_empty());
        assert!(preferred.apply_selection(&result).unwrap());
        assert_eq!(preferred.preferred.entries()[0].component.package, "live");
    }
    #[test]
    fn changed_candidates_downgrade_always_choice_before_answer() {
        let mut preferred = Preferred::default();
        preferred.add_preferred(activity("live", true), false);
        let candidates = vec![
            Candidate {
                component: unflatten("live/.Main").unwrap(),
                match_: 0x100000,
            },
            Candidate {
                component: unflatten("added/.Main").unwrap(),
                match_: 0x100000,
            },
        ];
        let result = preferred
            .select(&[0], &candidates, policy(true), |_| Ok(true), |_| Ok(false))
            .unwrap();
        assert_eq!(result.chosen, None);
        assert_eq!(result.edits.len(), 1);
        preferred.apply_selection(&result).unwrap();
        assert!(!preferred.preferred.entries()[0].always);
        assert!(preferred.preferred.entries()[0].set.is_none());
        let mut remove = policy(false);
        remove.remove_matches = true;
        let result = preferred
            .select(&[0], &candidates, remove, |_| Ok(true), |_| panic!())
            .unwrap();
        assert_eq!(result.chosen, None);
        assert_eq!(result.edits, [Edit::Remove { registration: 0 }]);
    }
}

#[derive(Clone, Debug)]
pub enum Mutation {
    Add {
        activity: PreferredActivity,
        remove_existing: bool,
    },
    Replace {
        activity: PreferredActivity,
    },
    SetLast {
        selection: Selection,
        activity: PreferredActivity,
    },
    Clear {
        package: Option<String>,
    },
    AddPersistent(PersistentPreferredActivity),
    ClearPersistentPackage(String),
    ClearPersistentFilter(IntentFilter),
    CrossAdd(CrossProfileIntentFilter),
    CrossRemove {
        filter: IntentFilter,
        owner_package: String,
        target_user: i32,
        flags: i32,
    },
    CrossClear {
        owner_package: String,
        accessible_targets: Vec<i32>,
    },
    Restore(Preferred),
    Reset {
        defaults: Vec<PreferredActivity>,
    },
    ApplyDefaults { defaults: Vec<PreferredActivity> },
}
#[derive(Clone, Debug, Default)]
pub struct MutationEffects {
    pub preferred_changed_broadcast: bool,
    pub broadcast_if_home_unsent: bool,
    pub broadcast_before_reset: bool,
    pub update_home: bool,
    pub reset_domain_user: bool,
    pub reset_runtime_permissions: bool,
    pub reset_network_policies: bool,
}
impl Preferred {
    pub fn clear_persistent_filter(&mut self, filter: &IntentFilter) -> bool {
        retain(&mut self.persistent, |old| {
            !filter_equals(&old.filter, filter)
        })
    }
}

pub fn default_apps_backup(browser: Option<&str>) -> Result<Vec<u8>, String> {
    let mut root = element("da");
    let mut defaults = element("default-apps");
    if let Some(browser) = browser.filter(|browser| !browser.is_empty()) {
        let mut item = element("default-browser");
        attr(&mut item, "packageName", Value::String(browser.to_owned()));
        child(&mut defaults, item);
    }
    child(&mut root, defaults);
    serialization::fast_xml(&root)
}

pub fn restored_default_browser(backup: &[u8]) -> Result<Option<String>, String> {
    let root = aim_android_xml::read(backup)?;
    if root.name != "da" {
        return Ok(None);
    }
    let Some(defaults) = root.children().next() else {
        return Ok(None);
    };
    Ok(defaults
        .children()
        .filter(|item| item.name == "default-browser")
        .map(|item| item.string("packageName").map(|name| name.into_owned()))
        .last()
        .flatten())
}

pub fn restored_preferred_activities(backup: &[u8]) -> Result<Option<Preferred>, String> {
    let root = aim_android_xml::read(backup)?;
    if root.name != "pa" {
        return Ok(None);
    }
    let Some(first) = root.children().next() else {
        return Ok(Some(Preferred::default()));
    };
    // readPreferredActivitiesLPw consumes the aligned section body; it does
    // not validate the section's name after restoreFromXml aligns the parser.
    let mut section = first.clone();
    section.name = "preferred-activities".into();
    let bytes = aim_android_xml::abx::write(&section)?;
    Ok(Some(Preferred::parse(Some(&bytes), None)))
}
