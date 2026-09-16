//! Caller-supplied Android identity; never inferred from host IDs or environment.
use darwin_art_engine_sys::ProcessCredentialsConfig;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessCredentialsInputs {
    pub uid: u32,
    pub euid: u32,
    pub suid: u32,
    pub gid: u32,
    pub egid: u32,
    pub sgid: u32,
    pub groups: Vec<u32>,
    pub permitted: u64,
    pub effective: u64,
    pub inheritable: u64,
}

impl ProcessCredentialsInputs {
    // Only used while this owner remains borrowed through synchronous copying.
    pub(super) fn wire(&self) -> ProcessCredentialsConfig {
        ProcessCredentialsConfig {
            uid: self.uid,
            euid: self.euid,
            suid: self.suid,
            gid: self.gid,
            egid: self.egid,
            sgid: self.sgid,
            groups: self.groups.as_ptr(),
            group_count: self.groups.len(),
            permitted: self.permitted,
            effective: self.effective,
            inheritable: self.inheritable,
        }
    }
}
