//! Exclusive ResilientAtomicFile read/failRead ownership, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{OwnedFile, Store, element, settings_paths};
use crate::package::{State, settings::Settings};
use aim_android_xml::Element;
use std::{
    fs::{self, OpenOptions},
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Main,
    Backup,
    Reserve,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Selected(Source),
    Removed(Source),
    Failed { source: Source, message: String },
    OwnerFailed { source: Source, message: String },
    FatalInput { source: Source, message: String },
    NoStartTag(Source),
    OpenFailed(Source),
    RemoveFailed(Source),
    Absent,
    CompletionFailed(String),
}

/// Exclusive boot frontend stages. Completion runs after version initialization
/// and before related user/runtime files are restored or a Store is published.
pub enum ReadStage<'a> {
    File(&'a [u8]),
    Complete,
}

/// Errors caught by original Settings recovery enter ResilientAtomicFile.failRead.
/// Native owner failures and uncaught input exceptions preserve the selected input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    File(String),
    Owner(String),
    FatalInput(String),
}

impl From<String> for ReadError {
    fn from(message: String) -> Self {
        Self::File(message)
    }
}
impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(message) | Self::Owner(message) | Self::FatalInput(message) => {
                message.fmt(f)
            }
        }
    }
}
impl std::error::Error for ReadError {}

#[derive(Debug)]
pub struct Error {
    pub events: Vec<Event>,
    pub message: String,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for Error {}

struct Input {
    file: Option<OwnedFile>,
    metadata: fs::Metadata,
    read: bool,
    source: Source,
    path: PathBuf,
}

/// Read-only claim of the canonical settings files. The exclusive boot owner
/// consumes it; changes since inspection reject before any cleanup operation.
pub struct Plan {
    data: PathBuf,
    inputs: [Option<Input>; 3],
    native_readers: Option<crate::package::settings::native_read::SharedReaders>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub first_boot: bool,
    pub events: Vec<Event>,
}

impl Plan {
    pub fn with_native_readers(
        mut self,
        readers: crate::package::settings::native_read::SharedReaders,
    ) -> Self {
        self.native_readers = Some(readers);
        self
    }
    pub fn inspect(data: &std::path::Path) -> Result<Self, Error> {
        let paths = settings_paths(data);
        let mut inputs = [None, None, None];
        for (index, source) in [Source::Main, Source::Backup, Source::Reserve]
            .into_iter()
            .enumerate()
        {
            let path = paths[index].clone();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(Error {
                        events: Vec::new(),
                        message: format!(
                            "UnsupportedOperation: settings inventory lookup requires an accessible parent (#912): {}: {error}",
                            path.display()
                        ),
                    });
                }
            };
            inputs[index] = Some(Input {
                file: None,
                metadata,
                read: false,
                source,
                path,
            });
        }
        Ok(Self {
            data: data.into(),
            inputs,
            native_readers: None,
        })
    }

    fn check(&self) -> Result<(), String> {
        for (index, path) in settings_paths(&self.data).iter().enumerate() {
            let metadata = match fs::symlink_metadata(path) {
                Ok(metadata) => Some(metadata),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.to_string()),
            };
            match (&self.inputs[index], metadata) {
                (None, None) => {}
                (Some(input), Some(metadata)) if same_metadata(&input.metadata, &metadata) => {
                    if input.read {
                        let file = input.file.as_ref().unwrap();
                        if !file.same_file(&metadata)
                            || fs::read(path)
                                .map_err(|error| error.to_string())?
                                .as_slice()
                                != file.payload.as_ref()
                        {
                            return Err("settings bytes changed outside the recovery owner".into());
                        }
                    }
                }
                _ => return Err("settings inventory changed outside the recovery owner".into()),
            }
        }
        Ok(())
    }

    // openRead catches only backup open errors; reading the selected stream
    // happens after backup cleanup, and read errors enter failRead.
    fn open(&mut self, index: usize) -> Result<(), ReadError> {
        let input = self.inputs[index].as_mut().unwrap();
        if input.metadata.is_dir() {
            return Err(ReadError::File(
                io::Error::from_raw_os_error(libc::EISDIR).to_string(),
            ));
        }
        if !input.metadata.is_file() {
            return Err(ReadError::Owner(
                "UnsupportedOperation: selected nonregular settings input requires stream/target ownership (#912)".into(),
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&input.path)
            .map_err(|error| ReadError::File(error.to_string()))?;
        let metadata = file
            .metadata()
            .map_err(|error| ReadError::Owner(error.to_string()))?;
        if !same_metadata(&input.metadata, &metadata) {
            return Err(ReadError::Owner(
                "settings inventory changed while opening recovery input".into(),
            ));
        }
        input.file = Some(OwnedFile {
            file,
            payload: std::sync::Arc::from([]),
        });
        Ok(())
    }

    fn select(&mut self, events: &mut Vec<Event>) -> Result<Option<usize>, Error> {
        for index in [1, 0, 2] {
            let Some(input) = &self.inputs[index] else {
                continue;
            };
            let source = input.source;
            match self.open(index) {
                Ok(()) => return Ok(Some(index)),
                Err(ReadError::File(message)) => {
                    events.push(Event::OpenFailed(source));
                    if index != 1 {
                        return Err(Error {
                            events: events.clone(),
                            message,
                        });
                    }
                }
                Err(error) => {
                    let message = error.to_string();
                    events.push(Event::OwnerFailed {
                        source,
                        message: message.clone(),
                    });
                    return Err(Error {
                        events: events.clone(),
                        message,
                    });
                }
            }
        }
        Ok(None)
    }

    fn read(&mut self, index: usize) -> Result<(), ReadError> {
        let input = self.inputs[index].as_mut().unwrap();
        let file = input.file.as_mut().unwrap();
        let mut bytes = Vec::new();
        file.file
            .read_to_end(&mut bytes)
            .map_err(|error| ReadError::File(error.to_string()))?;
        file.payload = bytes.into();
        input.read = true;
        Ok(())
    }

    fn remove(
        &mut self,
        index: usize,
        events: &mut Vec<Event>,
        best_effort: bool,
    ) -> Result<(), Error> {
        self.check().map_err(|message| Error {
            events: events.clone(),
            message,
        })?;
        if let Some(input) = &self.inputs[index] {
            let removed = if input.metadata.is_dir() {
                fs::remove_dir(&input.path)
            } else {
                fs::remove_file(&input.path)
            };
            if let Err(error) = removed {
                if best_effort {
                    events.push(Event::RemoveFailed(input.source));
                    return Ok(());
                }
                return Err(Error {
                    events: events.clone(),
                    message: error.to_string(),
                });
            }
            let removed = (input.metadata.dev(), input.metadata.ino());
            events.push(Event::Removed(input.source));
            self.inputs[index] = None;
            // Unlink changes every remaining hard link's ctime/link count.
            for input in self.inputs.iter_mut().flatten() {
                if (input.metadata.dev(), input.metadata.ino()) == removed {
                    let metadata = fs::symlink_metadata(&input.path).map_err(|error| Error {
                        events: events.clone(),
                        message: error.to_string(),
                    })?;
                    if !same_data(&input.metadata, &metadata)
                        || input.metadata.nlink().checked_sub(1) != Some(metadata.nlink())
                    {
                        return Err(Error {
                            events: events.clone(),
                            message: "settings hard link changed during recovery cleanup".into(),
                        });
                    }
                    input.metadata = metadata;
                }
            }
        }
        Ok(())
    }

    /// For parsers whose errors describe guest input only. Native owner callbacks
    /// use recover_with_owner. Neither entry rolls back earlier record mutations.
    pub fn recover(
        self,
        users: &[u32],
        settings: &mut Settings,
        mut parse: impl FnMut(&[u8], &mut Settings) -> Result<Option<Element>, String>,
    ) -> Result<(Store, Report), Error> {
        self.recover_with_owner(users, settings, |bytes, settings| {
            parse(bytes, settings).map_err(ReadError::File)
        })
    }

    /// Native frontends classify unavailable owners separately from corrupt
    /// input. Earlier parser/owner mutations remain observable on either error.
    pub fn recover_with_owner(
        self,
        users: &[u32],
        settings: &mut Settings,
        mut parse: impl FnMut(&[u8], &mut Settings) -> Result<Option<Element>, ReadError>,
    ) -> Result<(Store, Report), Error> {
        self.recover_inner(
            users,
            settings,
            |stage, settings| match stage {
                ReadStage::File(bytes) => parse(bytes, settings),
                ReadStage::Complete => unreachable!("completion disabled for record-only recovery"),
            },
            None,
            false,
        )
    }

    /// Native Settings boot lifecycle with values supplied by the pinned build
    /// owner. Version initialization precedes related user state restoration.
    pub fn recover_boot(
        self,
        users: &[u32],
        settings: &mut Settings,
        current: &crate::package::settings::Version,
        mut parse: impl FnMut(&[u8], &mut Settings) -> Result<Option<Element>, ReadError>,
    ) -> Result<(Store, Report), Error> {
        let result = self.recover_inner(
            users,
            settings,
            |stage, settings| match stage {
                ReadStage::File(bytes) => parse(bytes, settings),
                ReadStage::Complete => unreachable!("completion disabled for record-only recovery"),
            },
            Some(current),
            false,
        );
        settings.ensure_boot_versions(current);
        result
    }

    /// Complete pending/group owners on the original readLPw continuation path.
    /// Return None from Complete; native owner errors do not invoke failRead.
    pub fn recover_boot_frontend(
        self,
        users: &[u32],
        settings: &mut Settings,
        current: &crate::package::settings::Version,
        frontend: impl FnMut(ReadStage<'_>, &mut Settings) -> Result<Option<Element>, ReadError>,
    ) -> Result<(Store, Report), Error> {
        let result = self.recover_inner(users, settings, frontend, Some(current), true);
        settings.ensure_boot_versions(current);
        result
    }

    fn recover_inner(
        mut self,
        users: &[u32],
        settings: &mut Settings,
        mut parse: impl FnMut(ReadStage<'_>, &mut Settings) -> Result<Option<Element>, ReadError>,
        current: Option<&crate::package::settings::Version>,
        complete: bool,
    ) -> Result<(Store, Report), Error> {
        let mut events = Vec::new();
        let mut failed = false;
        loop {
            self.check().map_err(|message| Error {
                events: events.clone(),
                message,
            })?;
            let selected = self.select(&mut events)?;
            let Some(index) = selected else {
                events.push(Event::Absent);
                if let Some(current) = current {
                    settings.force_current_boot_versions(current);
                }
                if let Some(current) = current {
                    settings.ensure_boot_versions(current);
                }
                return self.finish(
                    users,
                    settings,
                    None,
                    Report {
                        first_boot: !failed,
                        events,
                    },
                    if complete { Some(&mut parse) } else { None },
                );
            };
            let source = self.inputs[index].as_ref().unwrap().source;
            events.push(Event::Selected(source));
            if index == 1 {
                // openRead discards main/reserve as soon as backup opens,
                // before the settings frontend decides whether backup parses.
                self.remove(0, &mut events, true)?;
                self.remove(2, &mut events, true)?;
            }
            let result = self.read(index).and_then(|()| {
                parse(
                    ReadStage::File(
                        &self.inputs[index]
                            .as_ref()
                            .unwrap()
                            .file
                            .as_ref()
                            .unwrap()
                            .payload,
                    ),
                    settings,
                )
            });
            match result {
                Ok(Some(document)) => {
                    if let Some(current) = current {
                        settings.ensure_boot_versions(current);
                    }
                    return self.finish(
                        users,
                        settings,
                        Some(document),
                        Report {
                            first_boot: false,
                            events,
                        },
                        if complete { Some(&mut parse) } else { None },
                    );
                }
                Ok(None) => {
                    events.push(Event::NoStartTag(source));
                    if let Some(current) = current {
                        settings.ensure_boot_versions(current);
                    }
                    return self.finish(
                        users,
                        settings,
                        None,
                        Report {
                            first_boot: !failed,
                            events,
                        },
                        if complete { Some(&mut parse) } else { None },
                    );
                }
                Err(ReadError::Owner(message)) => {
                    events.push(Event::OwnerFailed {
                        source,
                        message: message.clone(),
                    });
                    return Err(Error { events, message });
                }
                Err(ReadError::FatalInput(message)) => {
                    events.push(Event::FatalInput {
                        source,
                        message: message.clone(),
                    });
                    return Err(Error { events, message });
                }
                Err(ReadError::File(message)) => {
                    failed = true;
                    events.push(Event::Failed { source, message });
                    self.remove(index, &mut events, false)?;
                }
            }
        }
    }

    fn finish(
        mut self,
        users: &[u32],
        settings: &mut Settings,
        document: Option<Element>,
        mut report: Report,
        frontend: Option<
            &mut dyn FnMut(ReadStage<'_>, &mut Settings) -> Result<Option<Element>, ReadError>,
        >,
    ) -> Result<(Store, Report), Error> {
        self.check().map_err(|message| Error {
            events: report.events.clone(),
            message,
        })?;
        if !report.first_boot {
            if let Some(frontend) = frontend {
                let result = frontend(ReadStage::Complete, settings).and_then(|root| {
                    if root.is_some() {
                        Err(ReadError::Owner(
                            "completion returned a persistence document".into(),
                        ))
                    } else {
                        Ok(())
                    }
                });
                if let Err(error) = result {
                    let message = error.to_string();
                    report.events.push(Event::CompletionFailed(message.clone()));
                    return Err(Error {
                        events: report.events,
                        message,
                    });
                }
                self.check().map_err(|message| Error {
                    events: report.events.clone(),
                    message,
                })?;
            }
        }
        let state = if let Some(readers) = &self.native_readers {
            let readers = readers.0.lock().map_err(|_| Error {
                events: report.events.clone(),
                message: "native Settings owner poisoned".into(),
            })?;
            State::read_related_seeded(
                &self.data,
                users,
                settings.clone(),
                !report.first_boot,
                &readers.remaining,
            )
        } else {
            State::read_related_mode(&self.data, users, settings.clone(), !report.first_boot)
        }
        .map_err(|message| Error {
            events: report.events.clone(),
            message,
        })?;
        let present = document.is_some();
        let mut store = Store::from_state_mode(
            &self.data,
            users,
            state,
            document.unwrap_or_else(|| element("packages")),
            present,
            !report.first_boot,
        )
        .map_err(|message| Error {
            events: report.events.clone(),
            message,
        })?;
        if let Some(readers) = &self.native_readers {
            let readers = readers.0.lock().map_err(|_| Error {
                events: report.events.clone(),
                message: "native Settings owner poisoned".into(),
            })?;
            store
                .seed_native_readers(&readers.remaining)
                .map_err(|message| Error {
                    events: report.events.clone(),
                    message,
                })?;
        }
        self.check().map_err(|message| Error {
            events: report.events.clone(),
            message,
        })?;
        if !present {
            for input in self.inputs.iter_mut().filter_map(Option::take) {
                if input.read {
                    store.first_write_files.push(input.file.unwrap());
                } else {
                    store.first_write_inventory.push(input.metadata);
                }
            }
        }
        Ok((store, report))
    }
}

// Metadata claims cover unopened roles without requiring read access. Reads do
// not change these fields; chmod, replacement and in-place writes do.
fn same_data(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.mode() == b.mode()
        && a.uid() == b.uid()
        && a.gid() == b.gid()
        && a.len() == b.len()
        && a.rdev() == b.rdev()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
}

pub(super) fn same_contents(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    same_data(a, b) && a.nlink() == b.nlink()
}

pub(super) fn same_metadata(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    same_contents(a, b) && a.ctime() == b.ctime() && a.ctime_nsec() == b.ctime_nsec()
}
