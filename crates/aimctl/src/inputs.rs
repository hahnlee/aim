//! The image, runtime and data templates selected for a resident guest.

use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub image: Option<PathBuf>,
    pub host_runtime: Option<PathBuf>,
    pub userdata: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Inputs {
    pub image: PathBuf,
    pub host_runtime: PathBuf,
    pub userdata: PathBuf,
}

impl Default for Inputs {
    fn default() -> Self {
        Self {
            image: aim_paths::derived_image(),
            host_runtime: crate::program("guest-init").parent().unwrap().to_path_buf(),
            userdata: aim_paths::userdata(),
        }
    }
}

impl Selection {
    pub fn resolve(self) -> Result<Inputs, String> {
        let defaults = Inputs::default();
        let directory = |path: PathBuf| {
            let path = std::fs::canonicalize(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            if !path.is_dir() { return Err(format!("{}: not a directory", path.display())); }
            Ok(path)
        };
        Ok(Inputs {
            image: directory(self.image.unwrap_or(defaults.image))?,
            host_runtime: directory(self.host_runtime.unwrap_or(defaults.host_runtime))?,
            userdata: match self.userdata { Some(path) => directory(path)?, None => defaults.userdata },
        })
    }
}

impl Inputs {
    pub fn program(&self, name: &str) -> PathBuf { self.host_runtime.join(name) }

    pub fn arguments(&self) -> Vec<std::ffi::OsString> {
        vec!["--image".into(), self.image.clone().into(),
            "--host-runtime".into(), self.host_runtime.clone().into(),
            "--userdata".into(), self.userdata.clone().into()]
    }

    pub fn to_text(&self) -> String {
        let hex = |path: &Path| path.as_os_str().as_bytes().iter()
            .map(|byte| format!("{byte:02x}")).collect::<String>();
        format!("image_hex={}\nhost_runtime_hex={}\nuserdata_hex={}\n",
            hex(&self.image), hex(&self.host_runtime), hex(&self.userdata))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let field = |name: &str| text.lines().find_map(|line| line.strip_prefix(name));
        if ["image_hex=", "host_runtime_hex=", "userdata_hex="].iter()
            .all(|name| field(name).is_none()) { return Ok(Self::default()); }
        let path = |name: &str| -> Result<PathBuf, String> {
            let value = field(name).ok_or_else(|| format!("state: no {name}"))?;
            if value.is_empty() || value.len() % 2 != 0 || !value.is_ascii() {
                return Err(format!("state: bad {name}"));
            }
            let bytes = (0..value.len()).step_by(2)
                .map(|at| u8::from_str_radix(&value[at..at + 2], 16)
                    .map_err(|_| format!("state: bad {name}")))
                .collect::<Result<Vec<_>, _>>()?;
            if bytes.contains(&0) { return Err(format!("state: bad {name}")); }
            let path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
            if !path.is_absolute() { return Err(format!("state: bad {name}")); }
            Ok(path)
        };
        Ok(Self { image: path("image_hex=")?, host_runtime: path("host_runtime_hex=")?,
            userdata: path("userdata_hex=")? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resident_selection_preserves_raw_paths_and_rejects_partial_state() {
        let selected = Inputs { image: "/image with space\nroot".into(),
            host_runtime: PathBuf::from(std::ffi::OsString::from_vec(b"/runtime-\xff".to_vec())),
            userdata: "/templates".into() };
        assert_eq!(Inputs::parse(&selected.to_text()).unwrap(), selected);
        assert_eq!(Inputs::parse("pid=1\n").unwrap(), Inputs::default());
        assert!(Inputs::parse("image_hex=2f69\n").is_err());
        assert!(Inputs::parse("image_hex=00\nhost_runtime_hex=2f72\nuserdata_hex=2f75\n").is_err());
    }
}
