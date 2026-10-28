//! Connection-wide dentries/attributes, following Linux v6.6 fs/fuse/dir.c:
//! fuse_lookup, fuse_getattr and fuse_dentry_revalidate. Timeout zero disables
//! reuse; a cached dentry owns its daemon lookup until eviction and last user.
use super::*;
use std::time::Instant;

struct Timed {
    until: Instant,
    bytes: Vec<u8>,
}
#[derive(Default)]
struct References {
    real: u64,
    users: u64,
}
#[derive(Default)]
pub(super) struct Cache {
    pub epoch: u64,
    entries: BTreeMap<(u64, Vec<u8>), Timed>,
    attrs: BTreeMap<u64, Timed>,
    refs: BTreeMap<u64, References>,
}
fn deadline(seconds: u64, nanos: u32) -> Instant {
    let now = Instant::now();
    now.checked_add(
        Duration::from_secs(seconds)
            .saturating_add(Duration::from_nanos(nanos.min(999_999_999) as u64)),
    )
    .or_else(|| now.checked_add(Duration::from_secs(u32::MAX as u64)))
    .unwrap_or(now)
}
fn mutates(op: u32, data: &[u8]) -> Result<bool, Errno> {
    Ok(
        matches!(op, 4 | 6 | 8..=13 | 16 | 21 | 24 | 35 | 43 | 45 | 47 | 51)
            || op == 14 && u32_at(data, 0)? & 0o1000 != 0,
    )
}
impl Cache {
    pub fn notify(&mut self, code: i32, data: &[u8]) -> Result<(), Errno> {
        self.epoch = self.epoch.wrapping_add(1);
        let node = u64_at(data, 0)?;
        if code == 2 {
            self.attrs.remove(&node);
        } else {
            let start = if code == 6 { 24 } else { 16 };
            let mut name = data[start..].to_vec();
            name.push(0);
            self.entries.remove(&(node, name));
            if code == 6 {
                self.attrs.remove(&u64_at(data, 8)?);
            }
        }
        Ok(())
    }
    pub fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.entries.clear();
        self.attrs.clear();
    }
    fn collect(&mut self) -> Vec<(u64, u64)> {
        let now = Instant::now();
        self.entries.retain(|_, e| e.until > now);
        self.attrs.retain(|_, e| e.until > now);
        let held: std::collections::BTreeSet<_> = self
            .entries
            .values()
            .filter_map(|e| u64_at(&e.bytes, 0).ok())
            .collect();
        let mut forgets = Vec::new();
        self.refs.retain(|node, refs| {
            let needed = refs
                .users
                .min(refs.real)
                .max(u64::from(held.contains(node)));
            if refs.real > needed {
                forgets.push((*node, refs.real - needed));
                refs.real = needed;
            }
            if refs.users == 0 && !held.contains(node) {
                self.attrs.remove(node);
                false
            } else {
                true
            }
        });
        forgets
    }
    pub fn prepare(
        &mut self,
        op: u32,
        node: u64,
        data: &[u8],
    ) -> Result<(Option<Vec<u8>>, Vec<(u64, u64)>), Errno> {
        // Namespace, data and metadata mutations invalidate before dispatch,
        // so a concurrent older reply cannot repopulate the cache afterwards.
        if mutates(op, data)? {
            self.invalidate();
        }
        let reply = if op == FORGET {
            let count = u64_at(data, 0)?;
            if let Some(refs) = self.refs.get_mut(&node) {
                refs.users = refs.users.checked_sub(count).ok_or(EINVAL)?;
                Some(Vec::new())
            } else {
                None
            }
        } else {
            None
        };
        let forgets = self.collect();
        if op == FORGET {
            return Ok((reply, forgets));
        }
        let cached = match op {
            1 => self.entries.get(&(node, data.to_vec())),
            3 => self.attrs.get(&node),
            _ => None,
        }
        .map(|entry| entry.bytes.clone());
        if op == 1 {
            if let Some(bytes) = &cached {
                let node = u64_at(bytes, 0)?;
                if node > 1 {
                    self.refs.get_mut(&node).ok_or(EIO)?.users += 1;
                }
            }
        }
        Ok((cached, forgets))
    }
    pub fn record(&mut self, packet: &[u8], bytes: &[u8], epoch: u64) -> Result<(), Errno> {
        let op = u32_at(packet, 4)?;
        let node = u64_at(packet, 16)?;
        let current = epoch == self.epoch;
        if mutates(op, &packet[40..])? {
            self.invalidate();
        }
        if matches!(op, 1 | 6 | 8 | 9 | 13 | 35 | 51) {
            let child = u64_at(bytes, 0)?;
            if child > 1 {
                let refs = self.refs.entry(child).or_default();
                refs.real += 1;
                refs.users += 1;
            }
            if !current {
                return Ok(());
            }
            let offset = match op {
                1 | 6 => Some(40),
                8 | 35 => Some(56),
                9 | 13 => Some(48),
                _ => None,
            };
            if let Some(offset) = offset {
                let name = packet.get(offset..).ok_or(EINVAL)?;
                let length = name.iter().position(|byte| *byte == 0).ok_or(EINVAL)? + 1;
                let key = (node, name[..length].to_vec());
                self.entries.remove(&key);
                if u64_at(bytes, 16)? != 0 || u32_at(bytes, 32)? != 0 {
                    self.entries.insert(
                        key,
                        Timed {
                            until: deadline(u64_at(bytes, 16)?, u32_at(bytes, 32)?),
                            bytes: bytes[..128].to_vec(),
                        },
                    );
                }
            }
            if child != 0 {
                self.attribute(
                    child,
                    &bytes[40..128],
                    u64_at(bytes, 24)?,
                    u32_at(bytes, 36)?,
                );
            }
        } else if matches!(op, 3 | 4) && current {
            self.attribute(node, &bytes[16..104], u64_at(bytes, 0)?, u32_at(bytes, 8)?);
        }
        Ok(())
    }
    fn attribute(&mut self, node: u64, bytes: &[u8], seconds: u64, nanos: u32) {
        self.attrs.remove(&node);
        if seconds != 0 || nanos != 0 {
            let mut reply = vec![0; 16];
            reply.extend(bytes);
            self.attrs.insert(
                node,
                Timed {
                    until: deadline(seconds, nanos),
                    bytes: reply,
                },
            );
        }
    }
}
