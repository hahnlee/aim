//! `IPackageLaunchBridge` for the app host: the launch Intent
//! `ResolveIntentHelper` makes, written as `Intent.writeToParcel` writes
//! it, as system_server's `PackageLaunchBridge` serializes it, to the
//! system uid only.

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_NULL_POINTER, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_ipackagelaunchbridge as bridge;

use super::reply;
use crate::package::intent::{ComponentName, Intent};
use crate::package::intent_uri::FullIntent;

const SYSTEM_UID: u32 = 1000;
/// `Intent.FLAG_ACTIVITY_NEW_TASK`.
const FLAG_ACTIVITY_NEW_TASK: i32 = 0x1000_0000;

pub struct PackageLaunch;

/// `buildLaunchIntent`: `ACTION_MAIN` in `category`, a new task, for the
/// component, else the package.
fn launch_intent(
    category: Option<String>,
    package: Option<String>,
    component: Option<ComponentName>,
) -> Result<Vec<u8>, Exception> {
    // The native boot always names the category; a null one, which
    // `addCategory` would keep, is refused.
    let category = category.ok_or_else(|| Exception::new(EX_NULL_POINTER, "category"))?;
    let (package, component) = match component {
        Some(component) => (None, Some(component)),
        None => (package, None),
    };
    let intent = FullIntent {
        intent: Intent {
            action: Some("android.intent.action.MAIN".into()),
            flags: FLAG_ACTIVITY_NEW_TASK,
            package,
            component,
            categories: Some(vec![category]),
            ..Default::default()
        },
        source_bounds: None,
        extras: None,
    };
    let mut p = Parcel::new();
    intent.write(&mut p);
    Ok(p.data().to_vec())
}

impl Service for PackageLaunch {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != bridge::BUILD_LAUNCH_INTENT {
            return Err(UNKNOWN_TRANSACTION);
        }
        let a = bridge::BuildLaunchIntent::<ComponentName>::read(&mut call.data)
            .map_err(|_| BAD_VALUE)?;
        reply(if call.sender_euid != SYSTEM_UID {
            Err(Exception::security(
                "launch intent serialization requires system UID",
            ))
        } else {
            launch_intent(a.category, a.target_package, a.component).map(|bytes| {
                let mut p = Parcel::new();
                bridge::write_build_launch_intent_reply(&mut p, &Some(bytes));
                p
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::intent_filter::Plain;
    use aim_binder_host::parcel::Reader;

    /// The Intent reads back as the original writes it: a component's
    /// launch names no package; a package's names no component.
    #[test]
    fn launch_intents_are_the_originals() {
        let component = ComponentName {
            package: "a.b".into(),
            class: "a.b.Main".into(),
        };
        let read = |bytes: Vec<u8>| {
            let mut r = Reader::new(&bytes, &[]);
            Intent::read(&mut r, &mut Plain).unwrap()
        };
        let by_component = read(
            launch_intent(
                Some("android.intent.category.LAUNCHER".into()),
                Some("a.b".into()),
                Some(component.clone()),
            )
            .unwrap(),
        );
        assert_eq!(
            by_component.action.as_deref(),
            Some("android.intent.action.MAIN")
        );
        assert_eq!(by_component.flags, FLAG_ACTIVITY_NEW_TASK);
        assert_eq!(by_component.package, None);
        assert_eq!(by_component.component, Some(component));
        assert_eq!(
            by_component.categories,
            Some(vec!["android.intent.category.LAUNCHER".into()])
        );
        let by_package = read(
            launch_intent(
                Some("android.intent.category.INFO".into()),
                Some("a.b".into()),
                None,
            )
            .unwrap(),
        );
        assert_eq!(by_package.package.as_deref(), Some("a.b"));
        assert_eq!(by_package.component, None);
    }
}
