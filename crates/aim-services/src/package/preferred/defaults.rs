//! Settings.applyDefaultPreferredAppsLPw expands original package and image
//! declarations into concrete native choices, queried as SYSTEM_UID after reset.
use super::*;
use crate::package::{
    intent_filter::{AuthorityEntry, PatternMatcher},
    model::State,
    resolve::Resolution,
    uri::Uri,
};
use std::path::Path;

pub fn collect(
    state: &State,
    resolution: &Resolution,
    image: &Path,
    user: i32,
    package_order: &[String],
) -> Result<Vec<PreferredActivity>, String> {
    let mut declarations = Vec::new();
    for name in package_order {
        let package = state
            .packages
            .get(name)
            .ok_or("default preferred package registration missing")?;
        let Some(code) = package.pkg.as_ref().filter(|_| package.is.system) else {
            continue;
        };
        for (class, intent) in &code.preferred_activity_filters {
            declarations.push((
                ComponentName {
                    package: code.package_name.clone(),
                    class: class
                        .clone()
                        .ok_or("null default preferred activity class")?,
                },
                intent.filter.clone(),
            ));
        }
    }
    // PackagePartitions.getOrderedPartitions at the pinned version. Preserve
    // the actual filesystem iterator order within each original directory.
    for partition in ["system", "vendor", "odm", "oem", "product", "system_ext"] {
        let path = image.join(partition).join("etc/preferred-apps");
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error.to_string()),
        };
        for file in entries {
            let file = file.map_err(|error| error.to_string())?.path();
            if file.extension().is_none_or(|extension| extension != "xml") {
                continue;
            }
            let bytes = match std::fs::read(&file) {
                Ok(bytes) => bytes,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error.to_string()),
            };
            let root = match aim_android_xml::read(&bytes) {
                Ok(root) if root.name == "preferred-activities" => root,
                Ok(_) | Err(_) => continue,
            };
            let encoded = aim_android_xml::abx::write(&root)?;
            for activity in Preferred::parse(Some(&encoded), None).preferred.entries() {
                declarations.push((activity.component.clone(), activity.filter.clone()));
            }
        }
    }
    let mut preferred = Preferred::default();
    for (component, filter) in declarations {
        expand(&mut preferred, resolution, &filter, &component, user)?;
    }
    Ok(preferred.preferred.entries().to_vec())
}

fn expand(
    preferred: &mut Preferred,
    resolution: &Resolution,
    filter: &IntentFilter,
    component: &ComponentName,
    user: i32,
) -> Result<(), String> {
    let mut intent = Intent {
        action: Some(
            filter
                .actions
                .first()
                .ok_or("default preferred filter has no actions")?
                .clone(),
        ),
        ..Intent::default()
    };
    let mut flags = crate::package::info::flags::MATCH_DIRECT_BOOT_AWARE
        | crate::package::info::flags::MATCH_DIRECT_BOOT_UNAWARE;
    for category in filter.categories.iter().flatten() {
        if category == "android.intent.category.DEFAULT" {
            flags |= crate::package::component_resolver::MATCH_DEFAULT_ONLY;
        } else {
            intent
                .categories
                .get_or_insert_with(Vec::new)
                .push(category.clone());
        }
    }
    let mut non_data = true;
    let mut has_schemes = false;
    for scheme in filter.schemes.iter().flatten() {
        let mut bare_scheme = true;
        has_schemes |= !scheme.is_empty();
        for ssp in filter.ssps.iter().flatten() {
            let mut query = intent.clone();
            query.data = Some(Uri::preferred_opaque(scheme, &ssp.pattern));
            add(
                preferred,
                resolution,
                &query,
                None,
                flags,
                component,
                Some(scheme),
                Some(ssp),
                None,
                None,
                user,
            )?;
            bare_scheme = false;
        }
        for authority in filter.authorities.iter().flatten() {
            let mut bare_authority = true;
            for path in filter.paths.iter().flatten() {
                let mut query = intent.clone();
                query.data = Some(Uri::preferred_hierarchical(
                    scheme,
                    Some(&authority.orig_host),
                    Some(&path.pattern),
                ));
                add(
                    preferred,
                    resolution,
                    &query,
                    None,
                    flags,
                    component,
                    Some(scheme),
                    None,
                    Some(authority),
                    Some(path),
                    user,
                )?;
                bare_authority = false;
                bare_scheme = false;
            }
            if bare_authority {
                let mut query = intent.clone();
                query.data = Some(Uri::preferred_hierarchical(
                    scheme,
                    Some(&authority.orig_host),
                    None,
                ));
                add(
                    preferred,
                    resolution,
                    &query,
                    None,
                    flags,
                    component,
                    Some(scheme),
                    None,
                    Some(authority),
                    None,
                    user,
                )?;
                bare_scheme = false;
            }
        }
        if bare_scheme {
            let mut query = intent.clone();
            query.data = Some(Uri::preferred_hierarchical(scheme, None, None));
            add(
                preferred,
                resolution,
                &query,
                None,
                flags,
                component,
                Some(scheme),
                None,
                None,
                None,
                user,
            )?;
        }
        non_data = false;
    }
    for mime in filter.types.iter().flatten() {
        if has_schemes {
            for scheme in filter
                .schemes
                .iter()
                .flatten()
                .filter(|scheme| !scheme.is_empty())
            {
                let mut query = intent.clone();
                query.data = Some(Uri::preferred_hierarchical(scheme, None, None));
                add(
                    preferred,
                    resolution,
                    &query,
                    Some(mime),
                    flags,
                    component,
                    Some(scheme),
                    None,
                    None,
                    None,
                    user,
                )?;
            }
        } else {
            add(
                preferred,
                resolution,
                &intent,
                Some(mime),
                flags,
                component,
                None,
                None,
                None,
                None,
                user,
            )?;
        }
        non_data = false;
    }
    if non_data {
        add(
            preferred, resolution, &intent, None, flags, component, None, None, None, None, user,
        )?;
    }
    Ok(())
}

fn add(
    preferred: &mut Preferred,
    resolution: &Resolution,
    intent: &Intent,
    mime: Option<&str>,
    flags: i64,
    component: &ComponentName,
    scheme: Option<&str>,
    ssp: Option<&PatternMatcher>,
    authority: Option<&AuthorityEntry>,
    path: Option<&PatternMatcher>,
    user: i32,
) -> Result<(), String> {
    let matches = resolution
        .query_intent_activities(intent, mime, flags, user, 1000)
        .map_err(|error| format!("{error:?}"))?;
    if matches.is_empty() {
        return Ok(());
    }
    let mut have_activity = false;
    let mut non_system = false;
    let mut system_match = 0;
    let mut set = vec![None; matches.len()];
    for (index, result) in matches.iter().enumerate() {
        let (package, class) = result.component();
        set[index] = Some(ComponentName {
            package: package.to_owned(),
            class: class.to_owned(),
        });
        let system = match &result.info {
            crate::package::component_resolver::Info::Activity(activity) => {
                activity.info.application_info.flags & crate::package::info::FLAG_SYSTEM != 0
            }
            _ => false,
        };
        if !system {
            non_system = true;
            break;
        }
        if package == component.package && class == component.class {
            have_activity = true;
            system_match = result.match_;
        }
    }
    // The pinned implementation initializes thirdPartyMatch to zero and never
    // updates it before breaking on the first third-party result.
    if non_system && system_match > 0 {
        non_system = false;
    }
    if !have_activity || non_system {
        return Ok(());
    }
    let mut filter = IntentFilter::default();
    if let Some(action) = &intent.action {
        filter.add_action(action);
    }
    for category in intent.categories.iter().flatten() {
        filter.add_category(category);
    }
    if flags & crate::package::component_resolver::MATCH_DEFAULT_ONLY != 0 {
        filter.add_category("android.intent.category.DEFAULT");
    }
    if let Some(scheme) = scheme {
        filter.add_data_scheme(scheme);
    }
    if let Some(ssp) = ssp {
        filter.add_data_scheme_specific_part(ssp.clone());
    }
    if let Some(authority) = authority {
        filter.authorities = Some(vec![authority.clone()]);
    }
    if let Some(path) = path {
        filter.add_data_path(path.clone());
    }
    if let Some(mime) = mime {
        filter
            .add_data_type(mime)
            .map_err(|_| "malformed default preferred MIME type")?;
    }
    let set = set.into_iter().collect::<Option<Vec<_>>>();
    preferred.add_preferred(
        PreferredActivity::new(filter, system_match, set, component.clone(), true),
        true,
    );
    Ok(())
}
