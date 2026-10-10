//! Actual Settings permission write requests, serviced by the owned timer.
impl super::State {
    pub(crate) fn import_permission_user(&mut self, source: &Self, user: i32) {
        if let Some(version) = source.versions.get(&user) { self.versions.insert(user, *version); }
        else { self.versions.remove(&user); }
        if let Some(fingerprint) = source.fingerprints.get(&user) { self.fingerprints.insert(user, fingerprint.clone()); }
        else { self.fingerprints.remove(&user); }
        if let Some(extended) = &self.extended_fingerprint {
            self.upgrade_needed.insert(user, self.fingerprint(user) != Some(extended.as_str()));
        }
        if source.writes.contains(&user) { self.request_write(user); }
    }
    pub(crate) fn request_permission_write(&mut self, user: i32) -> Result<(), String> {
        if user < 0 { return Err("negative runtime permission user".into()); }
        self.request_write(user);
        Ok(())
    }
}
