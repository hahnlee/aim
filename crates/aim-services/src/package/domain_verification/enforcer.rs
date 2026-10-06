//! DomainVerificationEnforcer at android-16.0.0_r1 (AOSP, Apache License 2.0).
//! Missing permission/verifier/user/visibility owners remain explicit failures.
pub const DUMP: &str = "android.permission.DUMP";
pub const QUERY_ALL: &str = "android.permission.QUERY_ALL_PACKAGES";
pub const AGENT: &str = "android.permission.DOMAIN_VERIFICATION_AGENT";
pub const LEGACY_AGENT: &str = "android.permission.INTENT_FILTER_VERIFICATION_AGENT";
pub const CROSS_USER: &str = "android.permission.INTERACT_ACROSS_USERS";
pub const CROSS_USER_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
pub const UPDATE: &str = "android.permission.UPDATE_DOMAIN_VERIFICATION_USER_SELECTION";
pub const PREFERRED: &str = "android.permission.SET_PREFERRED_APPLICATIONS";

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Security,
    Owner(String),
}
pub trait Owners {
    fn permission(&self, uid: i32, permission: &str) -> Result<bool, String>;
    fn verifier(&self, uid: i32) -> Result<bool, String>;
    fn user_exists(&self, user: i32) -> Result<bool, String>;
    fn filtered(&self, package: Option<&str>, uid: i32, user: i32) -> Result<bool, String>;
}
#[derive(Clone, Copy, Debug)]
pub enum Operation<'a> {
    Internal,
    Info,
    Verifier,
    UriAgent,
    UserQuery(Option<&'a str>, i32),
    UserSelect(Option<&'a str>, i32),
    UserSelectionVisibility(&'a str, i32),
    Owners(i32),
    LegacySelect(&'a str, i32),
    LegacyQuery(&'a str, i32),
}
pub fn authorize(
    owners: &impl Owners,
    uid: i32,
    caller_user: i32,
    operation: Operation<'_>,
) -> Result<bool, Error> {
    let check = |permission| owners.permission(uid, permission).map_err(Error::Owner);
    let require = |permission| {
        if check(permission)? {
            Ok(())
        } else {
            Err(Error::Security)
        }
    };
    let users = |target| {
        if owners.user_exists(caller_user).map_err(Error::Owner)?
            && owners.user_exists(target).map_err(Error::Owner)?
        {
            Ok(())
        } else {
            Err(Error::Security)
        }
    };
    let visible = |package, target| {
        owners
            .filtered(package, uid, target)
            .map(|v| !v)
            .map_err(Error::Owner)
    };
    match operation {
        Operation::Internal => {
            if matches!(uid, 0 | 1000 | 2000) {
                Ok(true)
            } else {
                Err(Error::Security)
            }
        }
        Operation::Info => {
            if !matches!(uid, 0 | 1000 | 2000) {
                require(if owners.verifier(uid).map_err(Error::Owner)? {
                    QUERY_ALL
                } else {
                    DUMP
                })?;
            }
            Ok(true)
        }
        Operation::UriAgent => { require(AGENT)?; Ok(true) }
        Operation::Verifier => {
            if !matches!(uid, 0 | 1000 | 2000) {
                if check(AGENT)? {
                    require(QUERY_ALL)?;
                } else {
                    require(LEGACY_AGENT)?;
                }
                if !owners.verifier(uid).map_err(Error::Owner)? {
                    return Err(Error::Security);
                }
            }
            Ok(true)
        }
        Operation::UserQuery(package, target) => {
            if caller_user != target {
                require(CROSS_USER)?;
            }
            users(target)?;
            visible(package, target)
        }
        Operation::UserSelect(package, target) => {
            if caller_user != target {
                require(CROSS_USER)?;
            }
            require(UPDATE)?;
            users(target)?;
            package.map_or(Ok(true), |package| visible(Some(package), target))
        }
        Operation::UserSelectionVisibility(package, target) => visible(Some(package), target),
        Operation::Owners(target) => {
            if caller_user != target {
                require(CROSS_USER)?;
            }
            require(QUERY_ALL)?;
            require(UPDATE)?;
            users(target)?;
            Ok(true)
        }
        Operation::LegacySelect(package, target) => {
            require(PREFERRED)?;
            if caller_user != target && !check(CROSS_USER)? {
                return Ok(false);
            }
            users(target)?;
            visible(Some(package), target)
        }
        Operation::LegacyQuery(package, target) => {
            if caller_user != target {
                require(CROSS_USER_FULL)?;
            }
            users(target)?;
            visible(Some(package), target)
        }
    }
}

/// Permission checks use the current original front end; users/visibility use
/// the same captured native query graph. Verifier selection must be explicit.
pub struct Captured<'a> {
    pub query: &'a crate::package::query::Query<'a>,
    pub permissions: &'a dyn Fn(i32, &str) -> Result<bool, String>,
    pub verifier: &'a dyn Fn(i32) -> Result<bool, String>,
}
impl Owners for Captured<'_> {
    fn permission(&self, uid: i32, permission: &str) -> Result<bool, String> {
        (self.permissions)(uid, permission)
    }
    fn verifier(&self, uid: i32) -> Result<bool, String> {
        (self.verifier)(uid)
    }
    fn user_exists(&self, user: i32) -> Result<bool, String> {
        Ok(self.query.state.users.contains_key(&user))
    }
    fn filtered(&self, package: Option<&str>, uid: i32, user: i32) -> Result<bool, String> {
        crate::package::apps_filter::should_filter_application(
            self.query.state,
            self.query.filter,
            package.and_then(|name| self.query.state.packages.get(name)),
            uid,
            user,
            true,
            true,
        )
        .map_err(|e| e.0.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Missing;
    impl Owners for Missing {
        fn permission(&self, _: i32, _: &str) -> Result<bool, String> {
            Err("permission owner".into())
        }
        fn verifier(&self, _: i32) -> Result<bool, String> {
            Err("verifier owner".into())
        }
        fn user_exists(&self, _: i32) -> Result<bool, String> {
            Err("user owner".into())
        }
        fn filtered(&self, _: Option<&str>, _: i32, _: i32) -> Result<bool, String> {
            Err("visibility owner".into())
        }
    }
    #[test]
    fn uncaptured_owners_are_not_permission_denials_or_grants() {
        for op in [
            Operation::Info,
            Operation::Verifier,
            Operation::UserQuery(Some("p"), 0),
            Operation::UserSelect(Some("p"), 10),
            Operation::Owners(0),
        ] {
            assert!(matches!(
                authorize(&Missing, 10001, 0, op),
                Err(Error::Owner(_))
            ));
        }
        assert_eq!(authorize(&Missing, 1000, 0, Operation::Internal), Ok(true));
        assert_eq!(
            authorize(&Missing, 10001, 0, Operation::Internal),
            Err(Error::Security)
        );
    }
}
