//! Original staged session attributes share the existing exclusive AtomicFile owner.
use super::*;
impl Store {
    pub fn staged_states(
        &self,
    ) -> Result<
        std::collections::BTreeMap<i32, crate::package::installer::staged_owner::Status>,
        Error,
    > {
        let Some(claim) = self.claimed.iter().flatten().next() else {
            return Ok(std::collections::BTreeMap::new());
        };
        let root = aim_android_xml::read(&claim.bytes).map_err(before)?;
        let mut states = std::collections::BTreeMap::new();
        for session in root.children().filter(|node| node.name == "session") {
            if !session
                .bool("stagedSession")
                .map_err(before)?
                .unwrap_or(false)
            {
                continue;
            }
            let id = session
                .int("sessionId")
                .map_err(before)?
                .ok_or_else(|| before("staged session identity absent"))?;
            states.insert(
                id,
                crate::package::installer::staged_owner::Status {
                    ready: session.bool("isReady").map_err(before)?.unwrap_or(false),
                    applied: session.bool("isApplied").map_err(before)?.unwrap_or(false),
                    failed: session.bool("isFailed").map_err(before)?.unwrap_or(false),
                    error_code: session.int("errorCode").map_err(before)?.unwrap_or(0),
                    error_message: session
                        .string("errorMessage")
                        .map(|value| value.into_owned()),
                },
            );
        }
        Ok(states)
    }

    pub fn write_staged(
        &mut self,
        records: &[(Session, Record)],
        states: &std::collections::BTreeMap<i32, crate::package::installer::staged_owner::Status>,
    ) -> Result<(), Error> {
        if self.inspect()? != self.claimed {
            return Err(before("installer state changed outside exclusive owner"));
        }
        let mut root = element("sessions");
        for (session, record) in records {
            if session.destroyed && !session.parameters.staged {
                continue;
            }
            let mut node = self.session_document(session, record)?;
            if let Some(state) = states.get(&session.id) {
                node.attrs.retain(|(name, _)| {
                    !matches!(
                        name.as_str(),
                        "isReady" | "isApplied" | "isFailed" | "errorCode" | "errorMessage"
                    )
                });
                boolean(&mut node, "isReady", state.ready);
                boolean(&mut node, "isApplied", state.applied);
                boolean(&mut node, "isFailed", state.failed);
                int(&mut node, "errorCode", state.error_code);
                string(&mut node, "errorMessage", &state.error_message);
            }
            root.content.push(Node::Element(node));
        }
        let bytes = abx::write(&root).map_err(before)?;
        let [main, backup, new] = self.paths();
        if backup.exists() {
            fs::rename(&backup, &main).map_err(before)?;
        }
        let mut committed = false;
        let result = (|| -> Result<(), Error> {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .custom_flags(libc::O_NOFOLLOW)
                .mode(self.file_inode.mode.unwrap())
                .open(&new)
                .map_err(before)?;
            fs::set_permissions(
                &new,
                fs::Permissions::from_mode(self.file_inode.mode.unwrap()),
            )
            .map_err(before)?;
            guest_inode::record(&new, self.file_inode).map_err(before)?;
            (self.labeler)(&new, "/data/system/install_sessions.xml.new")?;
            file.write_all(&bytes).map_err(before)?;
            file.sync_all().map_err(before)?;
            fs::rename(&new, &main).map_err(before)?;
            committed = true;
            Ok(())
        })();
        if result.is_err() && new.exists() {
            fs::remove_file(&new).map_err(before)?;
        }
        self.claimed = self.inspect().map_err(|error| Error {
            legacy_status: error.legacy_status,
            committed,
            message: format!("{}; write result: {result:?}", error.message),
        })?;
        result
    }
}
