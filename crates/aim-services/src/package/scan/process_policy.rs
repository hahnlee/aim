//! ScanPackageUtils.assertProcessesAreValid, android-16.0.0_r1 (AOSP, Apache-2.0).
use crate::package::pkg::{AndroidPackage, MainComponent};

/// Pinned PackageManager.INSTALL_FAILED_PROCESS_NOT_DEFINED.
pub const STATUS: i32 = -122;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure { pub status: i32, pub message: String }

pub fn validate(pkg: &AndroidPackage) -> Result<(), Failure> {
    let Some(processes) = pkg.processes.as_ref().filter(|processes| !processes.is_empty()) else { return Ok(()); };
    let defined = |name: Option<&str>| processes.iter().any(|process| process.map_key.as_deref() == name);
    let app_process = pkg.process_name.as_deref().unwrap_or(&pkg.package_name);
    let missing = |message| Failure { status: STATUS, message };
    if !defined(Some(app_process)) {
        return Err(missing(format!("Can't install because application tag's process attribute {app_process} (in package {}) is not included in the <processes> list", pkg.package_name)));
    }
    check(pkg, &defined, pkg.activities.iter().map(|v| &v.main), "activity")?;
    check(pkg, &defined, pkg.services.iter().map(|v| &v.main), "service")?;
    check(pkg, &defined, pkg.receivers.iter().map(|v| &v.main), "receiver")?;
    check(pkg, &defined, pkg.providers.iter().map(|v| &v.main), "provider")
}

fn check<'a>(pkg: &AndroidPackage, defined: &impl Fn(Option<&str>) -> bool,
        components: impl DoubleEndedIterator<Item = &'a MainComponent>, kind: &str) -> Result<(), Failure> {
    for component in components.rev() {
        if !defined(component.process_name.as_deref()) {
            let process = component.process_name.as_deref().unwrap_or("null");
            return Err(Failure { status: STATUS, message: format!("Can't install because {kind} {}'s process attribute {process} (in package {}) is not included in the <processes> list", component.component.name, pkg.package_name) });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::pkg::{Activity, Service, Provider, Process};
    fn package() -> AndroidPackage {
        let main = MainComponent { process_name: Some("fixture".into()), ..Default::default() };
        AndroidPackage { package_name: "fixture".into(), processes: Some(vec![Process { map_key: Some("fixture".into()), name: Some("different.value.name".into()), ..Default::default() }]), activities: vec![Activity { main: main.clone(), ..Default::default() }], services: vec![Service { main: main.clone(), ..Default::default() }], receivers: vec![Activity { main: main.clone(), ..Default::default() }], providers: vec![Provider { main, ..Default::default() }], ..Default::default() }
    }
    #[test]
    fn explicit_process_map_validates_application_and_all_components_in_original_order() {
        let mut pkg = package(); assert!(validate(&pkg).is_ok());
        for kind in ["application", "activity", "service", "receiver", "provider"] {
            let mut missing = pkg.clone();
            if kind == "application" { missing.process_name = Some("absent".into()); }
            else {
                let component = match kind { "activity" => &mut missing.activities[0].main, "service" => &mut missing.services[0].main, "receiver" => &mut missing.receivers[0].main, _ => &mut missing.providers[0].main };
                component.component.name = "fixture.Missing".into(); component.process_name = Some("absent".into());
            }
            let failure = validate(&missing).unwrap_err(); assert_eq!(failure.status, STATUS);
            assert!(failure.message.starts_with(&format!("Can't install because {kind}")));
            assert!(failure.message.ends_with("(in package fixture) is not included in the <processes> list"));
        }
        pkg.receivers.push(Activity { main: MainComponent { component: crate::package::pkg::Component { name: "fixture.Last".into(), ..Default::default() }, process_name: Some("last.absent".into()), ..Default::default() }, ..Default::default() });
        pkg.receivers[0].main.process_name = Some("first.absent".into());
        assert!(validate(&pkg).unwrap_err().message.contains("receiver fixture.Last's process attribute last.absent"));
        pkg.processes = None; assert!(validate(&pkg).is_ok());
        pkg.processes = Some(Vec::new()); assert!(validate(&pkg).is_ok());
        pkg = package(); pkg.processes.as_mut().unwrap()[0].map_key = Some("different.value.name".into());
        assert!(validate(&pkg).is_err()); // Map keys, not ParsedProcess value names, are authoritative.
    }
}
