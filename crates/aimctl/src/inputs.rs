//! The image, runtime and data templates selected for a resident guest.

use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub image: Option<PathBuf>,
    pub host_runtime: Option<PathBuf>,
    pub userdata: Option<PathBuf>,
    pub binder_trace: Option<PathBuf>,
    pub adb_port: Option<u16>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Inputs {
    pub image: PathBuf,
    pub host_runtime: PathBuf,
    pub userdata: PathBuf,
    pub binder_trace: Option<PathBuf>,
    pub adb_port: Option<u16>,
}

impl Default for Inputs {
    fn default() -> Self {
        Self {
            image: aim_paths::derived_image(),
            host_runtime: crate::program("guest-init").parent().unwrap().to_path_buf(),
            userdata: aim_paths::userdata(),
            binder_trace: None,
            adb_port: None,
        }
    }
}

pub(crate) fn parse_adb_port(value: &str) -> Result<u16, String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("ADB port must be a nonzero u16".into());
    }
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| "ADB port must be a nonzero u16".into())
}

impl Selection {
    pub fn resolve(self) -> Result<Inputs, String> {
        if self.adb_port == Some(0) {
            return Err("ADB port must be a nonzero u16".into());
        }
        let defaults = Inputs::default();
        let directory = |path: PathBuf| {
            let path = std::fs::canonicalize(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            if !path.is_dir() {
                return Err(format!("{}: not a directory", path.display()));
            }
            Ok(path)
        };
        Ok(Inputs {
            image: directory(self.image.unwrap_or(defaults.image))?,
            host_runtime: directory(self.host_runtime.unwrap_or(defaults.host_runtime))?,
            userdata: match self.userdata {
                Some(path) => directory(path)?,
                None => defaults.userdata,
            },
            binder_trace: self
                .binder_trace
                .map(|path| {
                    let path = if path.is_absolute() {
                        path
                    } else {
                        std::env::current_dir()
                            .map_err(|error| error.to_string())?
                            .join(path)
                    };
                    let parent = std::fs::canonicalize(path.parent().ok_or("trace has no parent")?)
                        .map_err(|error| error.to_string())?;
                    let path = parent.join(path.file_name().ok_or("trace has no filename")?);
                    let mut pending = path.as_os_str().to_os_string();
                    pending.push(".pending.tsv");
                    for candidate in [&path, &PathBuf::from(pending)] {
                        match std::fs::symlink_metadata(candidate) {
                            Ok(_) => {
                                return Err(format!(
                                    "{}: trace output already exists",
                                    candidate.display()
                                ));
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => return Err(error.to_string()),
                        }
                    }
                    Ok(path)
                })
                .transpose()?,
            adb_port: self.adb_port,
        })
    }
}

/// Where an app host image (docs/app-host.md) lists the services init
/// starts for it.
const APP_HOST_SERVICES: &str = "system/etc/aim/app-host-services";

impl Inputs {
    pub fn program(&self, name: &str) -> PathBuf {
        self.host_runtime.join(name)
    }

    /// The services an app host image starts, one per line (`#` begins a
    /// comment); None for an image that boots Android whole.
    pub fn app_host_services(&self) -> Result<Option<Vec<String>>, String> {
        let path = self.image.join(APP_HOST_SERVICES);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(
                text.lines()
                    .map(|l| l.split('#').next().unwrap().trim())
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect(),
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn arguments(&self) -> Vec<std::ffi::OsString> {
        let mut args = vec![
            "--image".into(),
            self.image.clone().into(),
            "--host-runtime".into(),
            self.host_runtime.clone().into(),
            "--userdata".into(),
            self.userdata.clone().into(),
        ];
        if let Some(path) = &self.binder_trace {
            args.extend(["--binder-trace".into(), path.clone().into()]);
        }
        if let Some(port) = self.adb_port {
            args.extend(["--adb-port".into(), port.to_string().into()]);
        }
        args
    }

    pub fn to_text(&self) -> String {
        let hex = |path: &Path| {
            path.as_os_str()
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let mut text = format!(
            "image_hex={}\nhost_runtime_hex={}\nuserdata_hex={}\n",
            hex(&self.image),
            hex(&self.host_runtime),
            hex(&self.userdata)
        );
        if let Some(path) = &self.binder_trace {
            text.push_str(&format!("binder_trace_hex={}\n", hex(path)));
        }
        if let Some(port) = self.adb_port {
            text.push_str(&format!("adb_port={port}\n"));
        }
        text
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let ports = text
            .lines()
            .filter_map(|line| line.strip_prefix("adb_port="))
            .collect::<Vec<_>>();
        let adb_port = match ports.as_slice() {
            [] => None,
            [value] => Some(parse_adb_port(value)?),
            _ => return Err("state: duplicate adb_port".into()),
        };
        let field = |name: &str| text.lines().find_map(|line| line.strip_prefix(name));
        if ["image_hex=", "host_runtime_hex=", "userdata_hex="]
            .iter()
            .all(|name| field(name).is_none())
        {
            if field("binder_trace_hex=").is_some() || adb_port.is_some() {
                return Err("state: trace without selected inputs".into());
            }
            return Ok(Self::default());
        }
        let path = |name: &str| -> Result<PathBuf, String> {
            let value = field(name).ok_or_else(|| format!("state: no {name}"))?;
            if value.is_empty() || value.len() % 2 != 0 || !value.is_ascii() {
                return Err(format!("state: bad {name}"));
            }
            let bytes = (0..value.len())
                .step_by(2)
                .map(|at| {
                    u8::from_str_radix(&value[at..at + 2], 16)
                        .map_err(|_| format!("state: bad {name}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            if bytes.contains(&0) {
                return Err(format!("state: bad {name}"));
            }
            let path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
            if !path.is_absolute() {
                return Err(format!("state: bad {name}"));
            }
            Ok(path)
        };
        Ok(Self {
            image: path("image_hex=")?,
            host_runtime: path("host_runtime_hex=")?,
            userdata: path("userdata_hex=")?,
            binder_trace: if field("binder_trace_hex=").is_some() {
                Some(path("binder_trace_hex=")?)
            } else {
                None
            },
            adb_port,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adb_port_roundtrips_resident_state_and_detached_start_arguments() {
        let mut selected = Inputs::default();
        selected.adb_port = Some(65535);
        let restored = Inputs::parse(&selected.to_text()).unwrap();
        assert_eq!(restored.adb_port, Some(65535));
        let args = restored.arguments();
        assert!(args.windows(2).any(|pair| pair
            == [
                std::ffi::OsString::from("--adb-port"),
                std::ffi::OsString::from("65535")
            ]));
        let legacy = selected
            .to_text()
            .lines()
            .filter(|line| !line.starts_with("adb_port="))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(Inputs::parse(&legacy).unwrap().adb_port, None);
        for invalid in ["0", "65536", "-1", "+1", "", "port"] {
            assert!(Inputs::parse(&(legacy.clone() + "\nadb_port=" + invalid + "\n")).is_err());
        }
        assert!(Inputs::parse(&(legacy + "\nadb_port=1\nadb_port=2\n")).is_err());
        assert!(Inputs::parse("adb_port=5637\n").is_err());
    }

    #[test]
    fn resident_selection_preserves_raw_paths_and_rejects_partial_state() {
        let selected = Inputs {
            image: "/image with space\nroot".into(),
            host_runtime: PathBuf::from(std::ffi::OsString::from_vec(b"/runtime-\xff".to_vec())),
            userdata: "/templates".into(),
            binder_trace: Some("/trace with space\noutput.tsv".into()),
            adb_port: Some(5637),
        };
        assert_eq!(Inputs::parse(&selected.to_text()).unwrap(), selected);
        assert_eq!(Inputs::parse("pid=1\n").unwrap(), Inputs::default());
        assert!(selected.arguments().windows(2).any(|pair| pair
            == [
                std::ffi::OsString::from("--binder-trace"),
                std::ffi::OsString::from("/trace with space\noutput.tsv")
            ]));
        let legacy = selected
            .to_text()
            .lines()
            .filter(|line| !line.starts_with("binder_trace_hex="))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(Inputs::parse(&legacy).unwrap().binder_trace, None);
        assert!(Inputs::parse("binder_trace_hex=2f74\n").is_err());
        assert!(Inputs::parse(&(legacy.clone() + "\nbinder_trace_hex=zz\n")).is_err());
        assert!(Inputs::parse("image_hex=2f69\n").is_err());
        assert!(Inputs::parse("image_hex=00\nhost_runtime_hex=2f72\nuserdata_hex=2f75\n").is_err());
    }
}
