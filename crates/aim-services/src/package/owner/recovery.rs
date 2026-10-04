//! Exclusive ResilientAtomicFile read/failRead ownership, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{OwnedFile, Store, element, settings_paths};
use crate::package::{State, settings::Settings};
use aim_android_xml::Element;
use std::{
    fs::{self, OpenOptions},
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
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
    NoStartTag(Source),
    Absent,
}

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
    file: OwnedFile,
    source: Source,
    path: PathBuf,
}

/// Read-only claim of the canonical settings files. The exclusive boot owner
/// consumes it; changes since inspection reject before any cleanup operation.
pub struct Plan {
    data: PathBuf,
    inputs: [Option<Input>; 3],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub first_boot: bool,
    pub events: Vec<Event>,
}

impl Plan {
    pub fn inspect(data: &std::path::Path) -> Result<Self, Error> {
        let paths = settings_paths(data);
        let mut inputs = [None, None, None];
        for (index, source) in [Source::Main, Source::Backup, Source::Reserve]
            .into_iter()
            .enumerate()
        {
            let path = paths[index].clone();
            let mut file = match OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
            {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(Error {
                        events: Vec::new(),
                        message: format!("{}: {error}", path.display()),
                    });
                }
            };
            let metadata = file.metadata().map_err(|error| Error {
                events: Vec::new(),
                message: error.to_string(),
            })?;
            if !metadata.is_file() {
                return Err(Error {
                    events: Vec::new(),
                    message: "settings input is not a regular file".into(),
                });
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|error| Error {
                events: Vec::new(),
                message: error.to_string(),
            })?;
            inputs[index] = Some(Input {
                file: OwnedFile {
                    file,
                    payload: bytes.into(),
                },
                source,
                path,
            });
        }
        Ok(Self {
            data: data.into(),
            inputs,
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
                (Some(input), Some(metadata))
                    if metadata.is_file() && input.file.same_file(&metadata) =>
                {
                    if fs::read(path)
                        .map_err(|error| error.to_string())?
                        .as_slice()
                        != input.file.payload.as_ref()
                    {
                        return Err("settings bytes changed outside the recovery owner".into());
                    }
                }
                _ => return Err("settings inventory changed outside the recovery owner".into()),
            }
        }
        Ok(())
    }

    fn remove(&mut self, index: usize, events: &mut Vec<Event>) -> Result<(), Error> {
        self.check().map_err(|message| Error {
            events: events.clone(),
            message,
        })?;
        if let Some(input) = &self.inputs[index] {
            fs::remove_file(&input.path).map_err(|error| Error {
                events: events.clone(),
                message: error.to_string(),
            })?;
            events.push(Event::Removed(input.source));
            self.inputs[index] = None;
        }
        Ok(())
    }

    /// The settings frontend owns incremental record mutations, including
    /// effects before a failed read. This file owner never rolls them back.
    pub fn recover(
        mut self,
        users: &[u32],
        settings: &mut Settings,
        mut parse: impl FnMut(&[u8], &mut Settings) -> Result<Option<Element>, String>,
    ) -> Result<(Store, Report), Error> {
        let mut events = Vec::new();
        let mut failed = false;
        loop {
            self.check().map_err(|message| Error {
                events: events.clone(),
                message,
            })?;
            let selected = [1, 0, 2]
                .into_iter()
                .find(|index| self.inputs[*index].is_some());
            let Some(index) = selected else {
                events.push(Event::Absent);
                return self.finish(
                    users,
                    settings,
                    None,
                    Report {
                        first_boot: !failed,
                        events,
                    },
                );
            };
            let source = self.inputs[index].as_ref().unwrap().source;
            events.push(Event::Selected(source));
            if index == 1 {
                // openRead discards main/reserve as soon as backup opens,
                // before the settings frontend decides whether backup parses.
                self.remove(0, &mut events)?;
                self.remove(2, &mut events)?;
            }
            match parse(&self.inputs[index].as_ref().unwrap().file.payload, settings) {
                Ok(Some(document)) => {
                    return self.finish(
                        users,
                        settings,
                        Some(document),
                        Report {
                            first_boot: false,
                            events,
                        },
                    );
                }
                Ok(None) => {
                    events.push(Event::NoStartTag(source));
                    return self.finish(
                        users,
                        settings,
                        None,
                        Report {
                            first_boot: !failed,
                            events,
                        },
                    );
                }
                Err(message) => {
                    failed = true;
                    events.push(Event::Failed { source, message });
                    self.remove(index, &mut events)?;
                }
            }
        }
    }

    fn finish(
        mut self,
        users: &[u32],
        settings: &Settings,
        document: Option<Element>,
        report: Report,
    ) -> Result<(Store, Report), Error> {
        self.check().map_err(|message| Error {
            events: report.events.clone(),
            message,
        })?;
        let state =
            State::read_related(&self.data, users, settings.clone()).map_err(|message| Error {
                events: report.events.clone(),
                message,
            })?;
        let present = document.is_some();
        let mut store = Store::from_state(
            &self.data,
            users,
            state,
            document.unwrap_or_else(|| element("packages")),
            present,
        )
        .map_err(|message| Error {
            events: report.events.clone(),
            message,
        })?;
        self.check().map_err(|message| Error {
            events: report.events.clone(),
            message,
        })?;
        if !present {
            store.first_write_files.extend(
                self.inputs
                    .iter_mut()
                    .filter_map(|input| input.take().map(|input| input.file)),
            );
        }
        Ok((store, report))
    }
}
