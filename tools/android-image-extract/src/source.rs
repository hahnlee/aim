//! Random-access byte sources: files, memory, sub-ranges and extent maps.
//! Every read is bounds-checked against the source's declared size.
use crate::{Result, invalid};
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;

pub trait ReadAt {
    fn size(&self) -> u64;
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()>;

    fn read_vec(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        check_range(self.size(), offset, length as u64)?;
        let mut out = vec![0; length];
        self.read_exact_at(&mut out, offset)?;
        Ok(out)
    }
}

pub fn check_range(size: u64, offset: u64, length: u64) -> Result<()> {
    match offset.checked_add(length) {
        Some(end) if end <= size => Ok(()),
        _ => Err(invalid(format!(
            "read of {length} bytes at {offset} lies outside a {size}-byte source"
        ))),
    }
}

pub struct FileSource {
    file: File,
    size: u64,
}

impl FileSource {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)
            .map_err(|error| invalid(format!("open {}: {error}", path.display())))?;
        let size = file.metadata()?.len();
        Ok(Self { file, size })
    }
}

impl ReadAt for FileSource {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        FileExt::read_exact_at(&self.file, buf, offset)
    }
}

impl ReadAt for [u8] {
    fn size(&self) -> u64 {
        self.len() as u64
    }
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let start = usize::try_from(offset).map_err(|_| io::ErrorKind::UnexpectedEof)?;
        let bytes = start
            .checked_add(buf.len())
            .and_then(|end| self.get(start..end))
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        buf.copy_from_slice(bytes);
        Ok(())
    }
}

impl ReadAt for Vec<u8> {
    fn size(&self) -> u64 {
        self.len() as u64
    }
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        self.as_slice().read_exact_at(buf, offset)
    }
}

/// A window `[offset, offset + size)` of another source.
pub struct Slice<'a> {
    inner: &'a dyn ReadAt,
    offset: u64,
    size: u64,
}

impl<'a> Slice<'a> {
    pub fn new(inner: &'a dyn ReadAt, offset: u64, size: u64) -> Result<Self> {
        check_range(inner.size(), offset, size)?;
        Ok(Self {
            inner,
            offset,
            size,
        })
    }
}

impl ReadAt for Slice<'_> {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        if offset
            .checked_add(buf.len() as u64)
            .is_none_or(|end| end > self.size)
        {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.inner.read_exact_at(buf, self.offset + offset)
    }
}

/// One contiguous logical range; `physical == None` reads as zeroes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub logical: u64,
    pub physical: Option<u64>,
    pub length: u64,
}

/// A logical device assembled from sorted, gapless extents of another source.
pub struct ExtentView<'a> {
    inner: &'a dyn ReadAt,
    extents: Vec<Extent>,
    size: u64,
}

impl<'a> ExtentView<'a> {
    pub fn new(inner: &'a dyn ReadAt, extents: Vec<Extent>) -> Result<Self> {
        let mut logical = 0u64;
        for extent in &extents {
            if extent.logical != logical || extent.length == 0 {
                return Err(invalid("extent map has a hole, overlap or empty extent"));
            }
            if let Some(physical) = extent.physical {
                check_range(inner.size(), physical, extent.length)?;
            }
            logical = logical
                .checked_add(extent.length)
                .ok_or_else(|| invalid("extent map size overflow"))?;
        }
        Ok(Self {
            inner,
            extents,
            size: logical,
        })
    }
}

impl ReadAt for ExtentView<'_> {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        if offset
            .checked_add(buf.len() as u64)
            .is_none_or(|end| end > self.size)
        {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let mut at = offset;
        let mut done = 0usize;
        while done < buf.len() {
            let index = self.extents.partition_point(|e| e.logical + e.length <= at);
            let extent = self.extents[index];
            let within = at - extent.logical;
            let take = ((extent.length - within) as usize).min(buf.len() - done);
            let out = &mut buf[done..done + take];
            match extent.physical {
                Some(physical) => self.inner.read_exact_at(out, physical + within)?,
                None => out.fill(0),
            }
            done += take;
            at += take as u64;
        }
        Ok(())
    }
}

/// `std::io::Read` over a source range, for streaming decoders.
pub struct Reader<'a> {
    inner: &'a dyn ReadAt,
    position: u64,
    end: u64,
}

impl<'a> Reader<'a> {
    pub fn new(inner: &'a dyn ReadAt, offset: u64, length: u64) -> Result<Self> {
        check_range(inner.size(), offset, length)?;
        Ok(Self {
            inner,
            position: offset,
            end: offset + length,
        })
    }
}

impl io::Read for Reader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let take = (self.end - self.position).min(buf.len() as u64) as usize;
        self.inner.read_exact_at(&mut buf[..take], self.position)?;
        self.position += take as u64;
        Ok(take)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_and_extents_are_bounded() {
        let data: Vec<u8> = (0..32).collect();
        let slice = Slice::new(&data, 8, 8).unwrap();
        assert_eq!(slice.read_vec(0, 8).unwrap(), (8..16).collect::<Vec<u8>>());
        assert!(slice.read_vec(4, 5).is_err());
        assert!(Slice::new(&data, 30, 3).is_err());

        let view = ExtentView::new(
            &data,
            vec![
                Extent {
                    logical: 0,
                    physical: Some(20),
                    length: 4,
                },
                Extent {
                    logical: 4,
                    physical: None,
                    length: 2,
                },
                Extent {
                    logical: 6,
                    physical: Some(0),
                    length: 2,
                },
            ],
        )
        .unwrap();
        assert_eq!(view.size(), 8);
        assert_eq!(view.read_vec(2, 6).unwrap(), vec![22, 23, 0, 0, 0, 1]);
        assert!(view.read_vec(7, 2).is_err());
        let hole = Extent {
            logical: 1,
            physical: Some(0),
            length: 1,
        };
        assert!(ExtentView::new(&data, vec![hole]).is_err());
    }
}
