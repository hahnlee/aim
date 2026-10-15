use super::{Bridge, OwnerError, bridge};
use aim_binder_host::parcel::{BAD_VALUE, Parcel};
impl Bridge {
    pub fn invalidate_packages_for_uid_cache(&self) -> Result<(), OwnerError> {
        let mut request = Parcel::new();
        bridge::InvalidatePackagesForUidCache {}.write(&mut request);
        let reply = self
            .owner
            .transact(bridge::INVALIDATE_PACKAGES_FOR_UID_CACHE, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        bridge::read_invalidate_packages_for_uid_cache_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(())
    }
}
impl Bridge {
    pub fn is_auto_revoke_whitelisted(
        &self,
        calling_uid: i32,
        package_name: Option<String>,
    ) -> Result<bool, OwnerError> {
        let mut request = Parcel::new();
        bridge::IsAutoRevokeWhitelisted {
            calling_uid,
            package_name,
        }
        .write(&mut request);
        let reply = self
            .owner
            .transact(bridge::IS_AUTO_REVOKE_WHITELISTED, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let result = bridge::read_is_auto_revoke_whitelisted_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(result)
    }
}
impl Bridge {
    pub fn package_monitor_user(
        &self,
        calling_pid: i32,
        calling_uid: i32,
        user_id: i32,
    ) -> Result<i32, OwnerError> {
        let mut request = Parcel::new();
        bridge::PackageMonitorUser {
            calling_pid,
            calling_uid,
            user_id,
        }
        .write(&mut request);
        let reply = self
            .owner
            .transact(bridge::PACKAGE_MONITOR_USER, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let result = bridge::read_package_monitor_user_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(result)
    }
}
impl Bridge {
    pub fn log_package_process_start(
        &self,
        package_name: Option<String>,
        process_name: Option<String>,
        uid: i32,
        seinfo: Option<String>,
        apk_file: Option<String>,
        pid: i32,
    ) -> Result<(), OwnerError> {
        let mut request = Parcel::new();
        bridge::LogPackageProcessStart {
            package_name,
            process_name,
            uid,
            seinfo,
            apk_file,
            pid,
        }
        .write(&mut request);
        let reply = self
            .owner
            .transact(bridge::LOG_PACKAGE_PROCESS_START, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        bridge::read_log_package_process_start_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(())
    }
}
impl Bridge {
    pub fn wait_package_background_handler(&self, timeout_millis: i64) -> Result<bool, OwnerError> {
        let mut request = Parcel::new();
        bridge::WaitPackageBackgroundHandler { timeout_millis }.write(&mut request);
        let reply = self
            .owner
            .transact(bridge::WAIT_PACKAGE_BACKGROUND_HANDLER, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let result = bridge::read_wait_package_background_handler_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(result)
    }
}
