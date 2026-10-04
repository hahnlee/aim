//! Incremental PackageSignatures.readXml, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{Certificates, Signatures, certificate, defaulted};
use aim_android_xml::{
    Element,
    pull::{Event, Reader},
};

/// One settings read attempt shares this table between package/group owners.
/// Table mutations survive an XML failure; the target signing state does not
/// change until the original builder would publish its completed result.
#[derive(Default)]
pub struct SignatureReader {
    certificates: Certificates,
}

impl SignatureReader {
    pub fn certificates(&self) -> &[Option<(Vec<u8>, i32)>] {
        &self.certificates
    }

    /// The XML reader is at the completed signature-container start event.
    /// Returns current-certificate flags on publication, or None when a missing
    /// count skips the container without changing the existing signing owner.
    pub fn read(
        &mut self,
        reader: &mut Reader<'_>,
        start: &Element,
        target: &mut Option<Signatures>,
    ) -> Result<Option<Vec<i32>>, String> {
        let count = defaulted(start.int("count"), -1);
        if count == -1 {
            skip(reader)?;
            return Ok(None);
        }
        let mut past = None;
        let current = self.list(reader, count, false, &mut past)?;
        let flags = current.iter().map(|(_, flags)| *flags).collect();
        let signatures: Vec<_> = current.into_iter().map(|(key, _)| key).collect();
        // SigningDetails.Builder derives public keys only after reading the
        // complete list. Invalid DER clears SigningDetails, not its read table.
        *target = if super::super::sign::saved_certificates_valid(&signatures) {
            Some(Signatures {
                scheme_version: defaulted(start.int("schemeVersion"), 0),
                signatures,
                past_signatures: past,
                ..Default::default()
            })
        } else {
            None
        };
        Ok(Some(flags))
    }

    fn list(
        &mut self,
        reader: &mut Reader<'_>,
        count: i32,
        past_list: bool,
        past: &mut Option<Vec<(Vec<u8>, i32)>>,
    ) -> Result<Vec<(Vec<u8>, i32)>, String> {
        let outer = reader.depth();
        let mut position = 0i32;
        let mut out = Vec::new();
        loop {
            match reader.next()? {
                Event::Start(element) if element.name == "cert" => {
                    if position < count
                        && let Some((key, inherited, added)) =
                            certificate(&element, &mut self.certificates)?
                    {
                        let flags = if past_list {
                            match defaulted(element.int("flags"), -1) {
                                -1 => inherited,
                                flags => flags,
                            }
                        } else {
                            inherited
                        };
                        if past_list && let Some(index) = added {
                            self.certificates[index].as_mut().unwrap().1 = flags;
                        }
                        out.push((key, flags));
                    }
                    position = position.wrapping_add(1);
                    skip(reader)?;
                }
                Event::Start(element) if element.name == "pastSigs" && !past_list => {
                    let count = defaulted(element.int("count"), -1);
                    if count == -1 {
                        skip(reader)?;
                    } else {
                        *past = Some(self.list(reader, count, true, &mut None)?);
                    }
                }
                Event::Start(_) => skip(reader)?,
                Event::End(_) if reader.depth() <= outer => return Ok(out),
                Event::EndDocument => return Ok(out),
                _ => {}
            }
        }
    }
}

fn skip(reader: &mut Reader<'_>) -> Result<(), String> {
    let outer = reader.depth();
    loop {
        match reader.next()? {
            Event::End(_) if reader.depth() <= outer => return Ok(()),
            Event::EndDocument => return Ok(()),
            _ => {}
        }
    }
}
