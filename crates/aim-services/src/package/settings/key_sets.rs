//! Incremental KeySetManagerService reads, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{KeySetData, ReadError, Settings, identifier, required, signatures, string};
use aim_android_xml::{
    Element,
    pull::{Event, Reader},
};
use std::collections::BTreeMap;

impl Settings {
    /// Decode pool records in place. The public-key owner supplies canonical
    /// getEncoded bytes; the runtime owner finalizes references after the container.
    /// Neither native owner errors nor later XML errors roll back earlier records.
    pub fn read_key_sets(
        &mut self,
        reader: &mut Reader<'_>,
        start: &Element,
        refs: &BTreeMap<i64, i32>,
        mut public_key: impl FnMut(&[u8]) -> Result<Option<Vec<u8>>, ReadError>,
        mut finish: impl FnMut(&mut Self, &BTreeMap<i64, i32>) -> Result<(), ReadError>,
    ) -> Result<(), ReadError> {
        if string(start, "version").is_none() {
            signatures::skip(reader)?;
            for package in &mut self.packages {
                package.key_set_data = KeySetData::default();
            }
            return Ok(());
        }
        self.key_sets.versioned = true;
        let outer = reader.depth();
        loop {
            match reader.next()? {
                Event::Start(child) => match child.name.as_str() {
                    "keys" => self.read_public_keys(reader, &mut public_key)?,
                    "keysets" => self.read_key_set_list(reader)?,
                    "lastIssuedKeyId" => {
                        self.key_sets.last_issued_key_id =
                            required(&child, "value", child.long("value")?)?
                    }
                    "lastIssuedKeySetId" => {
                        self.key_sets.last_issued_key_set_id =
                            required(&child, "value", child.long("value")?)?
                    }
                    _ => {}
                },
                Event::End(_) if reader.depth() <= outer => break,
                Event::EndDocument => break,
                _ => {}
            }
        }
        finish(self, refs)
    }

    fn read_public_keys(
        &mut self,
        reader: &mut Reader<'_>,
        public_key: &mut impl FnMut(&[u8]) -> Result<Option<Vec<u8>>, ReadError>,
    ) -> Result<(), ReadError> {
        let outer = reader.depth();
        loop {
            match reader.next()? {
                Event::Start(child) if child.name == "public-key" => {
                    let id = identifier(&child)?;
                    if let Some(bytes) = child.bytes_base64("value").ok().flatten()
                        && let Some(key) = public_key(&bytes)?
                    {
                        match self
                            .key_sets
                            .public_keys
                            .iter_mut()
                            .find(|(old, _)| *old == id)
                        {
                            Some((_, value)) => *value = key,
                            None => self.key_sets.public_keys.push((id, key)),
                        }
                        self.key_sets.public_keys.sort_by_key(|(id, _)| *id);
                    }
                }
                Event::End(_) if reader.depth() <= outer => return Ok(()),
                Event::EndDocument => return Ok(()),
                _ => {}
            }
        }
    }

    fn read_key_set_list(&mut self, reader: &mut Reader<'_>) -> Result<(), ReadError> {
        let outer = reader.depth();
        let mut current = 0;
        loop {
            match reader.next()? {
                Event::Start(child) if child.name == "keyset" => {
                    current = identifier(&child)?;
                    match self
                        .key_sets
                        .key_sets
                        .iter_mut()
                        .find(|(id, _)| *id == current)
                    {
                        Some((_, keys)) => keys.clear(),
                        None => self.key_sets.key_sets.push((current, Vec::new())),
                    }
                    self.key_sets.key_sets.sort_by_key(|(id, _)| *id);
                    self.key_sets
                        .reference_counts
                        .get_or_insert_with(BTreeMap::new)
                        .insert(current, 0);
                }
                Event::Start(child) if child.name == "key-id" => {
                    let id = identifier(&child)?;
                    let (_, keys) = self
                        .key_sets
                        .key_sets
                        .iter_mut()
                        .find(|(id, _)| *id == current)
                        .ok_or_else(|| {
                            ReadError::FatalInput("key-id has no current keyset mapping".into())
                        })?;
                    if !keys.contains(&id) {
                        keys.push(id);
                        keys.sort_by_key(|id| (*id ^ (*id >> 32)) as i32);
                    }
                }
                Event::End(_) if reader.depth() <= outer => return Ok(()),
                Event::EndDocument => return Ok(()),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::settings::Package;

    fn read(
        bytes: &[u8],
        settings: &mut Settings,
        finish: impl FnMut(&mut Settings, &BTreeMap<i64, i32>) -> Result<(), ReadError>,
    ) -> Result<(), ReadError> {
        let mut reader = Reader::new(bytes)?;
        let Event::Start(start) = reader.next()? else {
            panic!()
        };
        settings.read_key_sets(
            &mut reader,
            &start,
            &BTreeMap::new(),
            |_| panic!("unexpected public key"),
            finish,
        )
    }

    #[test]
    fn records_survive_later_failure_and_reference_finalization_waits_for_container() {
        let mut settings = Settings::default();
        let mut finished = false;
        assert!(read(b"<keyset-settings version='1'><lastIssuedKeyId value='9'/><keysets><unknown><keyset identifier='2'><key-id identifier='4'/><key-id identifier='4'/></keyset></unknown><key-id identifier='3'/></keysets><lastIssuedKeySetId value='bad'/></keyset-settings>", &mut settings, |_,_| { finished = true; Ok(()) }).is_err());
        assert!(!finished);
        assert_eq!(settings.key_sets.last_issued_key_id, 9);
        assert_eq!(settings.key_sets.key_sets, [(2, vec![3, 4])]);
        read(b"<keyset-settings version='anything'><keysets><keyset identifier='2'><key-id identifier='5'/></keyset></keysets><lastIssuedKeySetId value='7'/></keyset-settings>", &mut settings, |state,_| { assert_eq!(state.key_sets.last_issued_key_set_id, 7); finished = true; Ok(()) }).unwrap();
        assert!(finished);
        assert_eq!(settings.key_sets.key_sets, [(2, vec![5])]);
    }

    #[test]
    fn unversioned_clear_follows_successful_skip_and_keeps_existing_pool() {
        let mut settings = Settings::default();
        settings.key_sets.key_sets.push((1, Vec::new()));
        let mut package = Package::default();
        package.key_set_data.proper_signing_key_set = 1;
        settings.packages.push(package);
        assert!(
            read(b"<keyset-settings><", &mut settings, |_, _| panic!(
                "unversioned finalized"
            ))
            .is_err()
        );
        assert_eq!(settings.packages[0].key_set_data.proper_signing_key_set, 1);
        read(
            b"<keyset-settings><keys/></keyset-settings>",
            &mut settings,
            |_, _| panic!("unversioned finalized"),
        )
        .unwrap();
        assert_eq!(settings.packages[0].key_set_data, KeySetData::default());
        assert_eq!(settings.key_sets.key_sets, [(1, Vec::new())]);
    }

    #[test]
    fn missing_current_mapping_is_an_uncaught_input_failure() {
        let error = read(b"<keyset-settings version='1'><keysets><key-id identifier='1'/></keysets></keyset-settings>", &mut Settings::default(), |_,_| panic!("invalid mapping finalized")).unwrap_err();
        assert!(matches!(error, ReadError::FatalInput(_)));
    }
}
