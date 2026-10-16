//! The settings SettingsProvider last persisted for user 0
//! (`/data/system/users/0/settings_{system,secure,global,config}.xml`,
//! `SettingsState`'s files), read-only: what an app's start reads of them.

use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Table {
    System,
    Secure,
    Global,
    /// DeviceConfig's flags, named `namespace/flag`.
    Config,
}

impl Table {
    fn file(self) -> &'static str {
        match self {
            Table::System => "settings_system.xml",
            Table::Secure => "settings_secure.xml",
            Table::Global => "settings_global.xml",
            Table::Config => "settings_config.xml",
        }
    }
}

#[derive(Debug, Default)]
pub struct Settings {
    values: HashMap<(Table, String), Option<String>>,
}

impl Settings {
    /// User 0's settings below the guest's `/data` at `data`; a table
    /// never written is empty.
    pub fn load(data: &Path) -> Result<Settings, String> {
        let mut settings = Settings::default();
        for table in [Table::System, Table::Secure, Table::Global, Table::Config] {
            let path = data.join("system/users/0").join(table.file());
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("{}: {e}", path.display())),
            };
            settings
                .read(table, &bytes)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(settings)
    }

    fn read(&mut self, table: Table, bytes: &[u8]) -> Result<(), String> {
        // SettingsState writes its namespace hashes after the settings.
        let roots = aim_android_xml::read_sequence(bytes)?;
        let root = roots
            .iter()
            .find(|r| r.name == "settings")
            .ok_or("no <settings>")?;
        for setting in root.children().filter(|e| e.name == "setting") {
            let name = setting.string("name").ok_or("setting without a name")?;
            let value = setting.string("value").map(|v| v.into_owned());
            self.values.insert((table, name.into_owned()), value);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn read_for_test(&mut self, table: Table, bytes: &[u8]) {
        self.read(table, bytes).unwrap();
    }

    /// `Settings.*.getStringForUser`: `None` for a setting that is absent
    /// or null.
    pub fn get(&self, table: Table, name: &str) -> Option<&str> {
        self.values.get(&(table, name.to_string()))?.as_deref()
    }

    /// `DeviceConfig.getProperty(namespace, name)`.
    pub fn device_config(&self, namespace: &str, name: &str) -> Option<&str> {
        self.get(Table::Config, &format!("{namespace}/{name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_values_and_nulls() {
        let mut s = Settings::default();
        s.read(
            Table::Secure,
            br#"<settings version="226">
            <setting id="1" name="long_press_timeout" value="400" package="android" />
            <setting id="2" name="nothing" package="android" />
        </settings>"#,
        )
        .unwrap();
        s.read(Table::Config, br#"<settings><setting id="3" name="widget/line_slop_ratio" value="0.25" /></settings>"#).unwrap();
        assert_eq!(s.get(Table::Secure, "long_press_timeout"), Some("400"));
        assert_eq!(s.get(Table::Secure, "nothing"), None);
        assert_eq!(s.device_config("widget", "line_slop_ratio"), Some("0.25"));
    }
}
