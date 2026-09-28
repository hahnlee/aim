//! BoringSSL FIPS module integrity hash.
//!
//! A FIPS build of BoringSSL checks at startup that the HMAC-SHA256 (fixed
//! all-zero key) of its module equals `BORINGSSL_bcm_text_hash`. The module
//! is the bytes between the `BORINGSSL_bcm_text_start/end` symbols and, in
//! shared builds, `BORINGSSL_bcm_rodata_start/end`; each part is then
//! preceded by its length as a little-endian u64. BoringSSL's build writes
//! the value after linking (`util/fipstools/inject_hash/inject_hash.go`;
//! the check is `BORINGSSL_integrity_test` in `crypto/fipsmodule/bcm.cc`,
//! both pinned in `upstream/android16-boringssl-fips.lock`).
//!
//! Rewriting sites inside the module changes those bytes, so the translator
//! repeats that build step on its output. The markers are found by symbol
//! name, which is BoringSSL's build contract (Android's libcrypto exports
//! them). The stored hash is found as the one occurrence of the original
//! module's HMAC in the file, so a file whose hash does not verify is left
//! alone.

use sha2::{Digest, Sha256};

use super::elf::Elf;

pub type Hash = [u8; 32];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    /// Module text, as `[start, end)` virtual addresses.
    pub text: (u64, u64),
    /// Module read-only data (shared builds only).
    pub rodata: Option<(u64, u64)>,
    pub text_offset: u64,
    pub rodata_offset: u64,
    /// Where the hash is stored.
    pub hash_vaddr: u64,
    pub hash_offset: u64,
    /// The hash of the original file.
    pub original: Hash,
}

/// HMAC-SHA256 with BoringSSL's all-zero 64-byte key.
fn hmac(parts: &[&[u8]]) -> Hash {
    let mut inner = Sha256::new();
    inner.update([0x36u8; 64]);
    for p in parts {
        inner.update(p);
    }
    let mut outer = Sha256::new();
    outer.update([0x5cu8; 64]);
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// The module hash over its text and (shared builds) read-only data.
pub fn digest(text: &[u8], rodata: Option<&[u8]>) -> Hash {
    match rodata {
        Some(r) => hmac(&[
            &(text.len() as u64).to_le_bytes(),
            text,
            &(r.len() as u64).to_le_bytes(),
            r,
        ]),
        None => hmac(&[text]),
    }
}

impl Module {
    fn range(bytes: &[u8], off: u64, (lo, hi): (u64, u64)) -> &[u8] {
        &bytes[off as usize..(off + hi - lo) as usize]
    }

    /// The module hash of a file laid out like the original (same offsets).
    pub fn digest_of(&self, bytes: &[u8]) -> Hash {
        digest(
            Self::range(bytes, self.text_offset, self.text),
            self.rodata
                .map(|r| Self::range(bytes, self.rodata_offset, r)),
        )
    }

    /// Store the hash of `patched` (the original with rewritten words).
    pub fn reinject(&self, patched: &mut [u8]) {
        let h = self.digest_of(patched);
        patched[self.hash_offset as usize..self.hash_offset as usize + 32].copy_from_slice(&h);
    }

    /// Whether `[lo, hi)` (virtual addresses) overlaps the hashed bytes.
    pub fn covers(&self, lo: u64, hi: u64) -> bool {
        let hit = |(a, b): (u64, u64)| lo < b && a < hi;
        hit(self.text) || self.rodata.is_some_and(hit)
    }
}

/// Find a FIPS module whose stored hash verifies against the file.
pub fn find(elf: &Elf) -> Option<Module> {
    let mut marks: [Option<u64>; 4] = [None; 4];
    const NAMES: [&[u8]; 4] = [
        b"BORINGSSL_bcm_text_start",
        b"BORINGSSL_bcm_text_end",
        b"BORINGSSL_bcm_rodata_start",
        b"BORINGSSL_bcm_rodata_end",
    ];
    for s in elf.symbols() {
        if let Some(i) = NAMES.iter().position(|n| *n == s.name) {
            match marks[i] {
                // .symtab and .dynsym both list it.
                Some(v) if v != s.value => return None,
                _ => marks[i] = Some(s.value),
            }
        }
    }
    let text = (marks[0]?, marks[1]?);
    let rodata = match (marks[2], marks[3]) {
        (Some(a), Some(b)) => Some((a, b)),
        (None, None) => None,
        _ => return None,
    };
    // Each part must lie inside one segment's file image.
    let offset = |(lo, hi): (u64, u64)| -> Option<u64> {
        let p = elf
            .loads()
            .find(|p| lo >= p.p_vaddr && hi <= p.p_vaddr.saturating_add(p.p_filesz))?;
        (lo <= hi).then_some(lo - p.p_vaddr + p.p_offset)
    };
    let text_offset = offset(text)?;
    let rodata_offset = match rodata {
        Some(r) => offset(r)?,
        None => 0,
    };
    let mut m = Module {
        text,
        rodata,
        text_offset,
        rodata_offset,
        hash_vaddr: 0,
        hash_offset: 0,
        original: [0; 32],
    };
    m.original = m.digest_of(elf.bytes);
    let mut hits = elf
        .bytes
        .windows(32)
        .enumerate()
        .filter(|(_, w)| *w == m.original);
    let (at, _) = hits.next()?;
    if hits.next().is_some() {
        return None;
    }
    m.hash_offset = at as u64;
    m.hash_vaddr = elf
        .loads()
        .find(|p| m.hash_offset >= p.p_offset && m.hash_offset + 32 <= p.p_offset + p.p_filesz)
        .map(|p| m.hash_offset - p.p_offset + p.p_vaddr)?;
    // A hash inside what it covers cannot be re-injected.
    (!m.covers(m.hash_vaddr, m.hash_vaddr + 32)).then_some(m)
}
