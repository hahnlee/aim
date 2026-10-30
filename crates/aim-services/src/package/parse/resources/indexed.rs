//! Chunk and type-spec indexing with on-demand entry decoding, following
//! androidfw LoadedArsc/AssetManager2 at android-16.0.0_r1 (Apache-2.0).
//! Copyright (C) The Android Open Source Project. Licensed under Apache-2.0.
use super::*;
use std::{
    ops::Range,
    sync::{Mutex, OnceLock},
};

struct Chunk {
    range: Range<usize>,
    header: usize,
    config: Config,
}

#[derive(Default)]
struct Group {
    flags: Vec<u32>,
    chunks: Vec<Chunk>,
    entries: Vec<OnceLock<Result<Vec<(Config, Entry)>>>>,
}

pub(super) struct Index {
    data: Vec<u8>,
    string_pool: Range<usize>,
    strings: OnceLock<Result<Strings>>,
    packages: Vec<(u8, HashMap<u8, Group>)>,
    pub(super) full: OnceLock<Result<Table>>,
    error: Mutex<Option<String>>,
}

impl Index {
    pub(super) fn new(data: Vec<u8>) -> Result<Self> {
        let top = chunks(&data)
            .next()
            .ok_or_else(|| bad("empty resource table"))??;
        if top.kind != TABLE {
            return Err(bad("not a resource table"));
        }
        let mut packages = Vec::new();
        let mut string_pool = None;
        for package in chunks(&top.data[top.header..]) {
            let package = package?;
            if package.kind == STRING_POOL {
                let start = package.data.as_ptr() as usize - data.as_ptr() as usize;
                string_pool = Some(start..start + package.data.len());
            }
            if package.kind != TABLE_PACKAGE {
                continue;
            }
            let pid = u32_at(package.data, 8)? as u8;
            let mut groups: HashMap<u8, Group> = HashMap::new();
            for c in chunks(&package.data[package.header..]) {
                let c = c?;
                if !matches!(c.kind, TABLE_TYPE | TABLE_TYPE_SPEC) {
                    continue;
                }
                let tid = *c.data.get(8).ok_or_else(|| bad("truncated type"))?;
                let group = groups.entry(tid).or_default();
                let count = u32_at(c.data, 12)? as usize;
                if count > 65536 {
                    return Err(bad("too many type entries"));
                }
                if c.kind == TABLE_TYPE_SPEC {
                    group.flags = (0..count)
                        .map(|i| u32_at(c.data, c.header + 4 * i))
                        .collect::<Result<_>>()?;
                } else {
                    let count = if c.data[9] & FLAG_SPARSE != 0 {
                        (0..count)
                            .map(|i| u16_at(c.data, c.header + 4 * i).map(|id| id as usize + 1))
                            .collect::<Result<Vec<_>>>()?
                            .into_iter()
                            .max()
                            .unwrap_or(0)
                    } else {
                        count
                    };
                    group
                        .entries
                        .resize_with(group.entries.len().max(count), OnceLock::new);
                    let start = c.data.as_ptr() as usize - data.as_ptr() as usize;
                    group.chunks.push(Chunk {
                        range: start..start + c.data.len(),
                        header: c.header,
                        config: Config::parse(
                            c.data
                                .get(20..c.header)
                                .ok_or_else(|| bad("truncated configuration"))?,
                        ),
                    });
                }
            }
            packages.push((pid, groups));
        }
        let string_pool = string_pool.ok_or_else(|| bad("table without strings"))?;
        Ok(Self {
            data,
            string_pool,
            strings: OnceLock::new(),
            packages,
            full: OnceLock::new(),
            error: Mutex::new(None),
        })
    }

    fn group(&self, id: u32) -> Option<&Group> {
        let pid = (id >> 24) as u8;
        self.packages
            .iter()
            .find(|(p, _)| *p == pid || (*p == 0 && pid == SHARED_LIBRARY_ID))?
            .1
            .get(&((id >> 16) as u8))
    }

    fn retain_error<'a, T>(&self, result: &'a Result<T>) -> Option<&'a T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                *self.error.lock().unwrap() = Some(error.to_string());
                None
            }
        }
    }

    pub(super) fn check(&self) -> std::result::Result<(), String> {
        self.error.lock().unwrap().clone().map_or(Ok(()), Err)
    }

    pub(super) fn full(&self) -> Option<&Table> {
        self.retain_error(self.full.get_or_init(|| Table::parse(&self.data)))
    }

    pub(super) fn string(&self, index: u32) -> Option<&str> {
        self.retain_error(
            self.strings
                .get_or_init(|| Strings::parse(&self.data[self.string_pool.clone()])),
        )?
        .get(index)
    }

    pub(super) fn flags(&self, id: u32) -> u32 {
        self.group(id)
            .and_then(|g| g.flags.get(id as u16 as usize))
            .copied()
            .unwrap_or(0)
    }

    pub(super) fn find(&self, id: u32, config: &Config) -> Option<(&Entry, u32, Config)> {
        let group = self.group(id)?;
        let entries = group.entries.get(id as u16 as usize)?.get_or_init(|| {
            let mut entries = Vec::new();
            for chunk in &group.chunks {
                let c = &self.data[chunk.range.clone()];
                if let Some(at) = offset(c, chunk.header, id as u16)? {
                    entries.push((chunk.config, read_entry(c, at)?));
                }
            }
            Ok(entries)
        });
        let mut best: Option<(&Entry, Config)> = None;
        for (candidate, entry) in self.retain_error(entries)? {
            if candidate.matches(config)
                && best.is_none_or(|(_, previous)| candidate.better_than(&previous, config))
            {
                best = Some((entry, *candidate));
            }
        }
        best.map(|(entry, config)| (entry, self.flags(id), config))
    }
}

fn offset(c: &[u8], header: usize, id: u16) -> Result<Option<usize>> {
    let count = u32_at(c, 12)? as usize;
    let start = u32_at(c, 16)? as usize;
    let flags = c[9];
    let value = if flags & FLAG_SPARSE != 0 {
        let mut found = None;
        for i in 0..count {
            if u16_at(c, header + 4 * i)? == id {
                found = Some(u16_at(c, header + 4 * i + 2)? as usize * 4);
            }
        }
        found
    } else if id as usize >= count {
        None
    } else if flags & FLAG_OFFSET16 != 0 {
        let o = u16_at(c, header + 2 * id as usize)?;
        (o != u16::MAX).then_some(o as usize * 4)
    } else {
        let o = u32_at(c, header + 4 * id as usize)?;
        (o != u32::MAX).then_some(o as usize)
    };
    value
        .map(|o| {
            start
                .checked_add(o)
                .ok_or_else(|| bad("entry offset overflow"))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_requested_entries_remain_errors() {
        let mut data = vec![0; 24];
        data[12..16].copy_from_slice(&1u32.to_le_bytes());
        data[16..20].copy_from_slice(&24u32.to_le_bytes());
        let index = Index {
            data,
            string_pool: 0..0,
            strings: OnceLock::new(),
            packages: vec![(
                0x7f,
                [(
                    1,
                    Group {
                        flags: vec![0],
                        chunks: vec![Chunk {
                            range: 0..24,
                            header: 20,
                            config: Config::default(),
                        }],
                        entries: vec![OnceLock::new()],
                    },
                )]
                .into(),
            )],
            full: OnceLock::new(),
            error: Mutex::new(None),
        };
        assert!(index.find(0x7f010000, &Config::default()).is_none());
        assert!(
            index.check().is_err(),
            "failed decoding cannot become a false window attribute"
        );
    }

    #[test]
    fn indexed_offsets_match_dense_sparse_and_offset16_tables() {
        for (flags, bytes) in [
            (0, vec![8, 0, 0, 0, 255, 255, 255, 255]),
            (FLAG_OFFSET16, vec![2, 0, 255, 255]),
            (FLAG_SPARSE, vec![0, 0, 2, 0, 7, 0, 3, 0]),
        ] {
            let mut c = vec![0; 20];
            c[9] = flags;
            c[12..16].copy_from_slice(&2u32.to_le_bytes());
            c[16..20].copy_from_slice(&32u32.to_le_bytes());
            c.extend(bytes);
            let eager = offsets(&c, 20)
                .unwrap()
                .into_iter()
                .collect::<HashMap<_, _>>();
            for id in 0..10 {
                assert_eq!(offset(&c, 20, id).unwrap(), eager.get(&id).copied());
            }
            c.truncate(21);
            assert!(offset(&c, 20, 0).is_err());
        }
    }
}
