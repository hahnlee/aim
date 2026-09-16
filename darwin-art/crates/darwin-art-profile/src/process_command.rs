//! Darwin process launch mechanics; no Android service or reuse policy.
use crate::ProfileError;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(crate) fn prepare_command(
    arguments: &[OsString],
    environment: &[(OsString, OsString)],
    working_directory: &Path,
) -> Result<Command, ProfileError> {
    let program = arguments
        .first()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProfileError::Daemon("process requires an executable".into()))?;
    let mut command = Command::new(program);
    // A daemon can outlive the bundle that started it. Neither its cwd nor its
    // environment may leak into a new runtime process after an update.
    command
        .args(&arguments[1..])
        .env_clear()
        .envs(environment.iter().cloned())
        .current_dir(working_directory)
        .stdin(Stdio::null());
    if let Some(path) = environment
        .iter()
        .find(|(key, _)| key == "DARWIN_ART_DAEMONIZED_LOG")
        .map(|(_, value)| PathBuf::from(value))
    {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)?;
        command
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_command_environment_and_cwd_are_preserved() {
        let command = prepare_command(
            &["/bin/cat".into(), "input".into()],
            &[("LANG".into(), "C".into())],
            Path::new("/tmp"),
        )
        .unwrap();
        assert_eq!(command.get_program(), "/bin/cat");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["input"]);
        assert_eq!(command.get_current_dir(), Some(Path::new("/tmp")));
        assert_eq!(command.get_envs().count(), 1);
        assert!(prepare_command(&[], &[], Path::new("/tmp")).is_err());
    }
}
