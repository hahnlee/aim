//! Credentials authenticated by the profile daemon for one SCM transfer.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Credentials {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

impl Credentials {
    pub(super) fn valid(self) -> bool {
        self.pid > 0
    }
}
