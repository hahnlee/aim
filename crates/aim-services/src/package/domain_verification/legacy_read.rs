//! DomainVerificationLegacySettings/SettingsXml cursor semantics, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{State, number, put};
use crate::package::string;
use aim_android_xml::{
    Element,
    pull::{Event, Reader},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionError {
    pub depth: i32,
    pub message: String,
}

pub(super) struct Sections<'r, 'a> {
    reader: &'r mut Reader<'a>,
    depths: Vec<i32>,
    pub(super) errors: Vec<SectionError>,
}

impl<'r, 'a> Sections<'r, 'a> {
    pub(super) fn new(reader: &'r mut Reader<'a>) -> Self {
        Sections {
            reader,
            depths: Vec::new(),
            errors: Vec::new(),
        }
    }
    pub(super) fn children(&mut self) {
        self.depths.push(self.reader.depth());
    }
    pub(super) fn next_named(&mut self, expected: Option<&str>) -> Option<Element> {
        let Some(&depth) = self.depths.last() else {
            return None;
        };
        loop {
            match self.reader.next() {
                Ok(Event::Start(element)) if expected.is_none_or(|name| name == element.name) => {
                    return Some(element);
                }
                Ok(Event::End(_)) if self.reader.depth() <= depth => {
                    self.depths.pop();
                    return None;
                }
                Ok(Event::EndDocument) => {
                    self.depths.pop();
                    return None;
                }
                Ok(_) => {}
                Err(message) => {
                    // Original moveToNext catches the exception without popping
                    // its shared depth stack. Retain a diagnostic before resuming.
                    self.errors.push(SectionError {
                        depth: self.reader.depth(),
                        message,
                    });
                    self.reader.resume_document_after_section_error();
                    return None;
                }
            }
        }
    }
}

impl State {
    /// Apply legacy user states at completed starts. SettingsXml sections share
    /// one cursor/stack and do not skip unknown subtrees. Diagnostics report the
    /// original section's caught errors; outer parsing still decides file recovery.
    pub fn read_legacy_events(&mut self, reader: &mut Reader<'_>) -> Vec<SectionError> {
        let mut section = Sections::new(reader);
        section.children();
        while let Some(start) = section.next_named(None) {
            if start.name != "user-states" {
                continue;
            }
            let name = string(&start, "packageName");
            let index = match self.legacy.iter().position(|(key, _)| *key == name) {
                Some(index) => index,
                None => {
                    self.legacy.push((name, Vec::new()));
                    self.legacy.len() - 1
                }
            };
            section.children();
            while let Some(start) = section.next_named(None) {
                if start.name == "user-state" {
                    let id = number(&start, "userId", -1);
                    let state = number(&start, "state", -1);
                    put(&mut self.legacy[index].1, id, (id, state), |(id, _)| id);
                    self.legacy[index].1.sort_by_key(|(id, _)| *id);
                }
            }
        }
        section.errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_section_error_keeps_outer_document_capture_available() {
        let mut settings = crate::package::settings::Settings::default();
        let root = settings.read_document(b"<packages><domain-verifications-legacy><user-states packageName='p'><user-state userId='1' state='3'/><", |_,_,_| Ok(false)).unwrap().unwrap();
        assert_eq!(root.name, "packages");
        assert_eq!(
            settings.domain_verification.legacy,
            [(Some("p".into()), vec![(1, 3)])]
        );
    }

    #[test]
    fn shared_sections_expose_nested_unknown_records_and_retain_error_effects() {
        let mut state = State::default();
        let mut reader = Reader::new(b"<domain-verifications-legacy><unknown><user-states packageName='p'><user-state userId='3' state='2'/><unknown><user-state userId='1' state='4'/></unknown><user-state userId='3' state='5'/><").unwrap();
        reader.next().unwrap();
        let errors = state.read_legacy_events(&mut reader);
        assert_eq!(state.legacy, [(Some("p".into()), vec![(1, 4), (3, 5)])]);
        assert!(!errors.is_empty());
    }
}
