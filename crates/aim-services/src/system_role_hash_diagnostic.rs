use super::*;

/// A retained system_server generation owns this read-only diagnostic capability.
pub struct RoleHashDiagnostic {
    system: Weak<System>,
    source: Arc<NonceSource>,
    owner: Strong,
}
impl RoleHashDiagnostic {
    fn check(&self) -> std::result::Result<(), String> {
        let system = self.system.upgrade().ok_or("diagnostic system stopped")?;
        if !system.nonces.lock().unwrap().as_ref().is_some_and(|current| Arc::ptr_eq(current, &self.source)) {
            return Err("role diagnostic bridge epoch changed".into());
        }
        Ok(())
    }
    pub fn bridge_handle(&self) -> u32 { self.source.bridge.handle }
    pub fn compute(&self, user: i32) -> std::result::Result<String, String> {
        use aim_service_aidl::dev_aim_server_ipackagediagnosticinputs as diagnostic;
        self.check()?;
        let mut request = Parcel::new();
        diagnostic::ComputeRolePackageStateHash { user_id: user }.write(&mut request);
        let reply = self.owner.transact(diagnostic::COMPUTE_ROLE_PACKAGE_STATE_HASH, &request, false)
            .map_err(|error| format!("role diagnostic transport: {error}"))?;
        let mut reader = reply.reader();
        let digest = diagnostic::read_compute_role_package_state_hash_reply(&mut reader)
            .map_err(|error| format!("role diagnostic wire: {error}"))?
            .map_err(|error| format!("role diagnostic owner: {error:?}"))?
            .ok_or("role diagnostic digest absent")?;
        if reader.remaining() != 0 { return Err("role diagnostic trailing reply".into()); }
        self.check()?;
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)) {
            return Err("role diagnostic digest is not original uppercase SHA256".into());
        }
        Ok(digest)
    }
}
impl System {
    pub fn role_hash_diagnostic(self: &Arc<Self>) -> std::result::Result<RoleHashDiagnostic, String> {
        let source = self.nonces.lock().unwrap().clone().ok_or("system_server bridge absent")?;
        let mut request = Parcel::new(); request.write_interface_token(bridge::DESCRIPTOR);
        let reply = source.bridge.transact(bridge::GET_PACKAGE_DIAGNOSTIC_INPUTS, &request, false)
            .map_err(|error| format!("role diagnostic leaf transport: {error}"))?;
        let mut reader = reply.reader();
        let binder = bridge::read_get_package_diagnostic_inputs_reply(&mut reader)
            .map_err(|error| format!("role diagnostic leaf wire: {error}"))?
            .map_err(|error| format!("role diagnostic leaf owner: {error:?}"))?
            .ok_or("role diagnostic leaf absent")?;
        if reader.remaining() != 0 { return Err("role diagnostic leaf trailing reply".into()); }
        let owner = reply.retain_remote_binder(binder).map_err(|error| format!("role diagnostic lifetime: {error}"))?;
        let diagnostic = RoleHashDiagnostic { system: Arc::downgrade(self), source, owner };
        diagnostic.check()?;
        Ok(diagnostic)
    }
}
