//! `game`: GameManagerService for the app host (`android-16.0.0_r1`): the
//! game mode of an app that is no game (`GAME_MODE_UNSUPPORTED`), and the
//! graphics environment notice of such an app, which does nothing. A
//! game's modes, their settings and configurations, and the listeners are
//! not implemented (#1672).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::android_app_igamemanagerservice as gm;

use super::{AppHost, reply};

/// `GameManager.GAME_MODE_UNSUPPORTED`; `ApplicationInfo.CATEGORY_GAME`;
/// `PackageManager.MATCH_ALL`.
const GAME_MODE_UNSUPPORTED: i32 = 0;
const CATEGORY_GAME: i32 = 0;
const MATCH_ALL: i64 = 0x0002_0000;

pub struct GameManager {
    host: Arc<AppHost>,
}

impl GameManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        GameManager { host }
    }

    /// `isPackageGame`: its category, with any package.
    fn is_game(&self, package: Option<&str>) -> Result<bool, Exception> {
        let Some(package) = package else {
            return Ok(false);
        };
        Ok(self
            .host
            .inputs()?
            .packages
            .application_info(package, MATCH_ALL)
            .is_some_and(|ai| ai.category == CATEGORY_GAME))
    }

    /// `getGameMode`'s answer for a package that is no game; a game's is
    /// not implemented.
    fn game_mode(
        &self,
        uid: i32,
        package: Option<&str>,
        user: i32,
        name: &str,
    ) -> Result<i32, Exception> {
        self.host
            .handle_incoming_user(uid, user, false, true, Some(name))?;
        if !self.is_game(package)? {
            return Ok(GAME_MODE_UNSUPPORTED);
        }
        Err(self
            .host
            .unimplemented_named("game", "a game's mode (#1672)"))
    }
}

impl Service for GameManager {
    fn descriptor(&self) -> &str {
        gm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        match call.code {
            gm::GET_GAME_MODE => {
                let a = gm::GetGameMode::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.game_mode(uid, a.package_name.as_deref(), a.user_id, "getGameMode") {
                    Ok(mode) => gm::write_get_game_mode_reply(&mut p, mode),
                    Err(e) => return reply(Err(e)),
                }
            }
            // Nothing for another package than the caller's
            // (`isValidPackageName`) or one that is no game.
            gm::NOTIFY_GRAPHICS_ENVIRONMENT_SETUP => {
                let a = gm::NotifyGraphicsEnvironmentSetup::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let result = (|| {
                    self.host.handle_incoming_user(
                        uid,
                        a.user_id,
                        false,
                        true,
                        Some("notifyGraphicsEnvironmentSetup"),
                    )?;
                    let package = a.package_name.as_deref().unwrap_or_default();
                    if !self.host.owns_package(uid, package)? {
                        return Ok(());
                    }
                    match self.game_mode(
                        uid,
                        a.package_name.as_deref(),
                        a.user_id,
                        "notifyGraphicsEnvironmentSetup",
                    )? {
                        GAME_MODE_UNSUPPORTED => Ok(()),
                        _ => Err(self
                            .host
                            .unimplemented_named("game", "a game's loading boost (#1672)")),
                    }
                })();
                if let Err(e) = result {
                    return reply(Err(e));
                }
                gm::write_notify_graphics_environment_setup_reply(&mut p);
            }
            code => {
                let name = gm::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                return reply(Err(self
                    .host
                    .unimplemented_named("game", &format!("{name} (#1672)"))));
            }
        }
        Ok(p)
    }
}
