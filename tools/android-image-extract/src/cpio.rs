//! "newc" cpio archives (Android ramdisks), possibly several concatenated,
//! decoded into an in-memory tree. Later entries replace earlier ones, as
//! the kernel's initramfs unpacker does.
use crate::tree::{Node, S_IFDIR, S_IFMT, S_IFREG, Tree};
use crate::{Result, invalid};
use std::collections::BTreeMap;
use std::io::Write;

const HEADER: usize = 110;
const TRAILER: &[u8] = b"TRAILER!!!";

struct Entry {
    node: Node,
    data: Vec<u8>,
    children: BTreeMap<Vec<u8>, u64>,
}

pub struct Archive {
    entries: Vec<Entry>,
}

fn hex(field: &[u8]) -> Result<u32> {
    let text = std::str::from_utf8(field).map_err(|_| invalid("cpio field is not ASCII"))?;
    u32::from_str_radix(text, 16).map_err(|_| invalid("cpio field is not hexadecimal"))
}

fn directory(mode: u32) -> Node {
    Node {
        mode: S_IFDIR | mode,
        uid: 0,
        gid: 0,
        size: 0,
        nlink: 2,
    }
}

impl Archive {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut archive = Self {
            entries: vec![Entry {
                node: directory(0o755),
                data: Vec::new(),
                children: BTreeMap::new(),
            }],
        };
        let mut at = 0usize;
        let mut members = 0usize;
        while at < bytes.len() {
            // Zero padding may separate or follow concatenated archives.
            if bytes[at] == 0 {
                at += 1;
                continue;
            }
            let header = bytes
                .get(at..at + HEADER)
                .ok_or_else(|| invalid("truncated cpio header"))?;
            if &header[..6] != b"070701" && &header[..6] != b"070702" {
                return Err(invalid(format!("invalid cpio magic at {at}")));
            }
            let field = |i: usize| hex(&header[6 + i * 8..14 + i * 8]);
            let mode = field(1)?;
            let size = field(6)? as usize;
            let name_size = field(11)? as usize;
            let name_at = at + HEADER;
            let name = bytes
                .get(name_at..name_at + name_size)
                .ok_or_else(|| invalid("truncated cpio name"))?;
            let name = name
                .strip_suffix(&[0])
                .ok_or_else(|| invalid("cpio name lacks NUL"))?;
            let data_at = (name_at + name_size).div_ceil(4) * 4;
            let data = bytes
                .get(data_at..data_at + size)
                .ok_or_else(|| invalid("truncated cpio data"))?;
            at = (data_at + size).div_ceil(4) * 4;
            if name == TRAILER {
                members += 1;
                continue;
            }
            if mode & S_IFMT == S_IFREG && field(4)? > 1 {
                return Err(invalid("hard-linked cpio members are unsupported"));
            }
            let node = Node {
                mode,
                uid: field(2)?,
                gid: field(3)?,
                size: size as u64,
                nlink: 1,
            };
            archive.insert(name, node, data.to_vec())?;
        }
        if members == 0 {
            return Err(invalid("cpio archive has no trailer"));
        }
        Ok(archive)
    }

    fn insert(&mut self, path: &[u8], node: Node, data: Vec<u8>) -> Result<()> {
        let parts: Vec<&[u8]> = path
            .split(|b| *b == b'/')
            .filter(|p| !p.is_empty() && *p != b".")
            .collect();
        if parts.iter().any(|p| *p == b"..") {
            return Err(invalid("cpio path escapes the archive"));
        }
        let Some((last, parents)) = parts.split_last() else {
            // "." sets the root's metadata.
            if node.mode & S_IFMT == S_IFDIR {
                self.entries[0].node = node;
            }
            return Ok(());
        };
        let mut id = 0u64;
        for part in parents {
            id = match self.entries[id as usize].children.get(*part) {
                Some(child) if self.entries[*child as usize].node.mode & S_IFMT == S_IFDIR => {
                    *child
                }
                Some(_) => return Err(invalid("cpio path traverses a non-directory")),
                None => self.push(id, part, directory(0o755), Vec::new()),
            };
        }
        match self.entries[id as usize].children.get(*last).copied() {
            // Re-declaring a directory keeps its contents.
            Some(child)
                if node.mode & S_IFMT == S_IFDIR
                    && self.entries[child as usize].node.mode & S_IFMT == S_IFDIR =>
            {
                self.entries[child as usize].node = node;
            }
            _ => {
                self.push(id, last, node, data);
            }
        }
        Ok(())
    }

    fn push(&mut self, parent: u64, name: &[u8], node: Node, data: Vec<u8>) -> u64 {
        let id = self.entries.len() as u64;
        self.entries.push(Entry {
            node,
            data,
            children: BTreeMap::new(),
        });
        self.entries[parent as usize]
            .children
            .insert(name.to_vec(), id);
        id
    }
}

impl Tree for Archive {
    fn root(&self) -> u64 {
        0
    }
    fn node(&self, id: u64) -> Result<Node> {
        Ok(self.entries[id as usize].node)
    }
    fn children(&self, id: u64) -> Result<Vec<(Vec<u8>, u64)>> {
        Ok(self.entries[id as usize]
            .children
            .iter()
            .map(|(name, id)| (name.clone(), *id))
            .collect())
    }
    fn read_link(&self, id: u64) -> Result<Vec<u8>> {
        Ok(self.entries[id as usize].data.clone())
    }
    fn copy_file(&self, id: u64, out: &mut dyn Write) -> Result<u64> {
        let entry = &self.entries[id as usize];
        if entry.node.mode & S_IFMT != S_IFREG {
            return Err(invalid("cpio entry is not a regular file"));
        }
        out.write_all(&entry.data)?;
        Ok(entry.data.len() as u64)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::tree::{Materializer, lookup, read_file, tests::temp_dir};

    pub(crate) fn member(out: &mut Vec<u8>, name: &str, mode: u32, data: &[u8]) {
        let fields = [
            0,
            mode,
            0,
            2000,
            1,
            0,
            data.len() as u32,
            0,
            0,
            1,
            3,
            name.len() as u32 + 1,
            0,
        ];
        out.extend_from_slice(b"070701");
        for value in fields {
            out.extend_from_slice(format!("{value:08x}").as_bytes());
        }
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        out.resize(out.len().div_ceil(4) * 4, 0);
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(4) * 4, 0);
    }

    pub(crate) fn build() -> Vec<u8> {
        let mut out = Vec::new();
        member(&mut out, "dev", 0o040755, b"");
        member(&mut out, "dev/null", 0o020666, b"");
        member(&mut out, "init", 0o100750, b"\x7fELF");
        member(&mut out, TRAILER_STR, 0, b"");
        out.extend_from_slice(&[0; 512]);
        member(&mut out, "system/bin/init", 0o120777, b"/init");
        member(&mut out, "init", 0o100750, b"second");
        member(&mut out, TRAILER_STR, 0, b"");
        out
    }

    const TRAILER_STR: &str = "TRAILER!!!";

    #[test]
    fn parses_concatenated_archives() {
        let archive = Archive::parse(&build()).unwrap();
        let init = lookup(&archive, "/init").unwrap().unwrap();
        assert_eq!(read_file(&archive, init, 64).unwrap(), b"second");
        let link = lookup(&archive, "/system/bin/init").unwrap().unwrap();
        assert_eq!(archive.read_link(link).unwrap(), b"/init");
        let out = temp_dir("cpio");
        let mut materializer = Materializer::default();
        materializer.extract(&archive, &out).unwrap();
        let report = materializer.finish().unwrap();
        assert_eq!(
            (report.files, report.symlinks, report.special.len()),
            (1, 1, 1)
        );
        std::fs::remove_dir_all(&out).unwrap();
    }

    #[test]
    fn rejects_malformed_archives() {
        let good = build();
        assert!(Archive::parse(&good[..good.len() - 200]).is_err());
        let mut escape = Vec::new();
        member(&mut escape, "../x", 0o100644, b"");
        assert!(Archive::parse(&escape).is_err());
        let mut bad = good.clone();
        bad[0] = b'1';
        assert!(Archive::parse(&bad).is_err());
    }
}
