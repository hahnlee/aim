//! The Intent an application launch asks its first Activity to receive.
//!
//! `am start -a ACTION -d URI PACKAGE` names the Intent for the launched
//! Activity. A host launch carries the same request in its environment
//! (`DARWIN_ART_APK_APP_INTENT_ACTION` / `DARWIN_ART_APK_APP_INTENT_URI`);
//! the daemon records it with the process so system_server can build that
//! Intent when the process attaches, instead of the launcher Intent.
use crate::ProfileError;
use std::ffi::OsString;

pub const LAUNCH_INTENT_ACTION_ENV: &str = "DARWIN_ART_APK_APP_INTENT_ACTION";
pub const LAUNCH_INTENT_URI_ENV: &str = "DARWIN_ART_APK_APP_INTENT_URI";

const MAX_ACTION: usize = 256;
const MAX_DATA: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchIntent {
    pub action: String,
    pub data: Option<String>,
}

impl LaunchIntent {
    /// The launch request's Intent, when its environment names an action.
    pub(crate) fn from_environment(
        environment: &[(OsString, OsString)],
    ) -> Result<Option<Self>, ProfileError> {
        let value = |name: &str| {
            environment
                .iter()
                .rev()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let Some(action) = value(LAUNCH_INTENT_ACTION_ENV) else {
            if value(LAUNCH_INTENT_URI_ENV).is_some() {
                return Err(invalid("a launch Intent URI requires an action"));
            }
            return Ok(None);
        };
        let action = action
            .into_string()
            .map_err(|_| invalid("launch Intent action is not UTF-8"))?;
        let data = value(LAUNCH_INTENT_URI_ENV)
            .map(|data| {
                data.into_string()
                    .map_err(|_| invalid("launch Intent URI is not UTF-8"))
            })
            .transpose()?;
        let intent = Self { action, data };
        intent.validate()?;
        Ok(Some(intent))
    }

    fn validate(&self) -> Result<(), ProfileError> {
        if self.action.is_empty()
            || self.action.len() > MAX_ACTION
            || !self
                .action
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'_')
        {
            return Err(invalid("invalid launch Intent action"));
        }
        if let Some(data) = &self.data
            && (data.is_empty() || data.len() > MAX_DATA || data.bytes().any(|byte| byte < 0x20))
        {
            return Err(invalid("invalid launch Intent URI"));
        }
        Ok(())
    }

    /// `action` NUL `data`; an empty payload is "no launch Intent".
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = self.action.as_bytes().to_vec();
        bytes.push(0);
        if let Some(data) = &self.data {
            bytes.extend_from_slice(data.as_bytes());
        }
        bytes
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Option<Self>, ProfileError> {
        if bytes.is_empty() {
            return Ok(None);
        }
        let separator = bytes
            .iter()
            .position(|&byte| byte == 0)
            .ok_or_else(|| invalid("invalid launch Intent response"))?;
        let text = |part: &[u8]| {
            std::str::from_utf8(part)
                .map(str::to_owned)
                .map_err(|_| invalid("invalid launch Intent response"))
        };
        let action = text(&bytes[..separator])?;
        let data = &bytes[separator + 1..];
        let intent = Self {
            action,
            data: if data.is_empty() {
                None
            } else {
                Some(text(data)?)
            },
        };
        intent.validate()?;
        Ok(Some(intent))
    }
}

fn invalid(message: &str) -> ProfileError {
    ProfileError::Daemon(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect()
    }

    #[test]
    fn environment_selects_the_requested_intent() {
        assert_eq!(LaunchIntent::from_environment(&[]).unwrap(), None);
        let intent = LaunchIntent::from_environment(&environment(&[
            (LAUNCH_INTENT_ACTION_ENV, "android.intent.action.VIEW"),
            (LAUNCH_INTENT_URI_ENV, "https://127.0.0.1:8443/index.html"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(intent.action, "android.intent.action.VIEW");
        assert_eq!(
            intent.data.as_deref(),
            Some("https://127.0.0.1:8443/index.html")
        );
        assert_eq!(
            LaunchIntent::decode(&intent.encode()).unwrap(),
            Some(intent)
        );
        let bare = LaunchIntent::from_environment(&environment(&[(
            LAUNCH_INTENT_ACTION_ENV,
            "android.intent.action.MAIN",
        )]))
        .unwrap()
        .unwrap();
        assert_eq!(LaunchIntent::decode(&bare.encode()).unwrap(), Some(bare));
        assert_eq!(LaunchIntent::decode(b"").unwrap(), None);
    }

    #[test]
    fn malformed_requests_are_rejected() {
        for pairs in [
            vec![(LAUNCH_INTENT_URI_ENV, "https://example.com")],
            vec![(LAUNCH_INTENT_ACTION_ENV, "")],
            vec![(LAUNCH_INTENT_ACTION_ENV, "bad action")],
            vec![
                (LAUNCH_INTENT_ACTION_ENV, "android.intent.action.VIEW"),
                (LAUNCH_INTENT_URI_ENV, "line\nbreak"),
            ],
        ] {
            assert!(LaunchIntent::from_environment(&environment(&pairs)).is_err());
        }
        assert!(LaunchIntent::decode(b"no-separator").is_err());
    }
}
