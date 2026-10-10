//! Package launch IntentSender planning. ActivityManager owns the returned sender.
//! android-16.0.0_r1, Copyright AOSP, Apache License 2.0.
use super::{apps_filter::NotModelled, intent::ComponentName, query::Query, resolve::Resolution};
use aim_binder_host::parcel::{Binder, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use std::sync::Arc;
pub struct Request {
    pub caller_uid: i32,
    pub caller_package: Option<String>,
    pub feature: Option<String>,
    pub user: i32,
    pub category: String,
    pub target_package: Option<String>,
    pub component: Option<ComponentName>,
}
pub struct Sender {
    pub target: Option<Binder>,
    retained: Option<Arc<aim_binder_host::local::Strong>>,
}
impl Sender {
    pub fn retained(node: aim_binder_host::local::Strong) -> Self {
        let target = Some(node.binder());
        Self {
            target,
            retained: Some(Arc::new(node)),
        }
    }
    pub fn local(target: Binder) -> Self {
        Self {
            target: Some(target),
            retained: None,
        }
    }
}
impl WriteParcelable for Sender {
    fn write_to(&self, parcel: &mut Parcel) {
        if let Some(retained) = &self.retained {
            parcel.keep_alive(retained.clone());
        }
        parcel.write_binder(self.target);
    }
}
pub struct Owner {
    create: Box<dyn Fn(Request) -> Result<Sender, Exception> + Send + Sync>,
}
impl Owner {
    pub fn new(create: Box<dyn Fn(Request) -> Result<Sender, Exception> + Send + Sync>) -> Self {
        Self { create }
    }
    pub fn create(&self, request: Request) -> Result<Sender, Exception> {
        (self.create)(request)
    }
}
impl std::fmt::Debug for Owner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackageLaunchSender")
            .finish_non_exhaustive()
    }
}
impl PartialEq for Owner {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}
impl Query<'_> {
    pub(crate) fn launch_sender(
        &self,
        package: Option<&str>,
        caller_package: Option<&str>,
        feature: Option<&str>,
        user: i32,
    ) -> Result<Result<Sender, Exception>, NotModelled> {
        let Some(package) = package else {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "packageName is null",
            )));
        };
        if let Err(exception) = self.internal_enforce_cross_user(
            self.calling_uid,
            user,
            false,
            false,
            "get launch intent sender for package",
        )? {
            return Ok(Err(exception));
        }
        let uid = match self.package_uid(caller_package.unwrap_or_default(), 0, user)? {
            Ok(uid) => uid,
            Err(exception) => return Ok(Err(exception)),
        };
        if super::apps_filter::app_id(uid) != super::apps_filter::app_id(self.calling_uid) {
            return Ok(Err(Exception::security(format!(
                "getLaunchIntentSenderForPackage() from calling uid: {} does not own package: {}",
                self.calling_uid,
                caller_package.unwrap_or("null")
            ))));
        }
        let resolution = Resolution::new(
            Arc::new(self.state.clone()),
            &super::apps_filter::Config {
                force_system_packages_queryable: self.state.system.force_system_packages_queryable,
                force_queryable_packages: self.state.system.force_queryable_packages.clone(),
            },
        )
        .map_err(|_| NotModelled("native launch component registry unavailable"))?;
        let mut category = "android.intent.category.INFO";
        let mut candidates =
            resolution.launch_candidates(package, category, user, self.calling_uid)?;
        if candidates.is_empty() {
            category = "android.intent.category.LAUNCHER";
            candidates = resolution.launch_candidates(package, category, user, self.calling_uid)?;
        }
        let component = candidates.first().map(|candidate| {
            let (package, class) = candidate.component();
            ComponentName {
                package: package.into(),
                class: class.into(),
            }
        });
        let owner = self.state.system.launch_sender.as_ref().ok_or(NotModelled(
            "actual ActivityManager IntentSender owner unavailable",
        ))?;
        Ok(owner.create(Request {
            caller_uid: self.calling_uid,
            caller_package: caller_package.map(str::to_owned),
            feature: feature.map(str::to_owned),
            user,
            category: category.into(),
            target_package: component.is_none().then(|| package.into()),
            component,
        }))
    }
}
