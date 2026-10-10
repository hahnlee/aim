//! Original ApexManager inputs. Partition resolution follows android-16.0.0_r1
//! InitAppsHelper. Copyright (C) The Android Open Source Project, Apache License 2.0.
use crate::package::scan::{Apex, Partition};
use aim_binder_host::parcel::{BAD_VALUE, Reader, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApexPackage {
    pub module_name: Option<String>,
    pub module_path: String,
    pub preinstalled_path: String,
    pub version_code: i64,
    pub factory: bool,
    pub active: bool,
    pub active_changed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveApex {
    pub module_name: Option<String>,
    pub mount_path: String,
    pub preinstalled_path: String,
    pub factory: bool,
    pub module_path: String,
    pub active_changed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApexInventory {
    pub packages: Option<Vec<ApexPackage>>,
    pub active: Vec<ActiveApex>,
}

fn boolean(r: &mut Reader<'_>) -> Result<bool> {
    match r.read_i32()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(BAD_VALUE),
    }
}
fn path(r: &mut Reader<'_>) -> Result<String> {
    r.read_string16()?.ok_or(BAD_VALUE)
}
fn count(r: &mut Reader<'_>, minimum: usize) -> Result<usize> {
    let n = r.read_i32()?;
    if n < 0 || n as usize > r.remaining() / minimum {
        return Err(BAD_VALUE);
    }
    Ok(n as usize)
}

impl ApexInventory {
    pub fn read_original_record(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes, &[]);
        let n = r.read_i32()?;
        let packages = if n == -1 {
            None
        } else {
            if n < 0 || n as usize > r.remaining() / 32 {
                return Err(BAD_VALUE);
            }
            let mut packages = Vec::new();
            for _ in 0..n {
                packages.push(ApexPackage {
                    module_name: r.read_string16()?,
                    module_path: path(&mut r)?,
                    preinstalled_path: path(&mut r)?,
                    version_code: r.read_i64()?,
                    factory: boolean(&mut r)?,
                    active: boolean(&mut r)?,
                    active_changed: boolean(&mut r)?,
                });
            }
            Some(packages)
        };
        let n = count(&mut r, 24)?;
        let mut active = Vec::new();
        for _ in 0..n {
            active.push(ActiveApex {
                module_name: r.read_string16()?,
                mount_path: path(&mut r)?,
                preinstalled_path: path(&mut r)?,
                factory: boolean(&mut r)?,
                module_path: path(&mut r)?,
                active_changed: boolean(&mut r)?,
            });
        }
        if r.remaining() != 0 || bytes.len() % 4 != 0 {
            return Err(BAD_VALUE);
        }
        Ok(Self { packages, active })
    }

    /// Unknown preinstalled origins do not become scan partitions. The original
    /// returns null for them; mount names never determine the origin or module.
    pub fn scan_apexes(&self) -> Vec<Apex> {
        self.active
            .iter()
            .filter_map(|info| {
                [
                    Partition::System,
                    Partition::Vendor,
                    Partition::Odm,
                    Partition::Oem,
                    Partition::Product,
                    Partition::SystemExt,
                ]
                .into_iter()
                .find(|partition| {
                    let root = format!("/{}", partition.name());
                    info.preinstalled_path == root
                        || info.preinstalled_path.starts_with(&(root + "/"))
                })
                .map(|partition| Apex {
                    module_name: info.module_name.clone(),
                    mount_path: info.mount_path.clone(),
                    partition,
                    factory: info.factory,
                    active_changed: info.active_changed,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::Parcel;

    #[test]
    fn inventory_preserves_absent_all_and_rejects_malformed_frames() {
        for (n, expected) in [(-1, None), (0, Some(Vec::new()))] {
            let mut p = Parcel::new();
            p.write_i32(n);
            p.write_i32(0);
            let bytes = p.data();
            assert_eq!(
                ApexInventory::read_original_record(bytes).unwrap().packages,
                expected
            );
            for end in 0..bytes.len() {
                assert!(ApexInventory::read_original_record(&bytes[..end]).is_err());
            }
            let mut tail = bytes.to_vec();
            tail.extend(0i32.to_le_bytes());
            assert!(ApexInventory::read_original_record(&tail).is_err());
        }
        for words in [[-2, 0], [i32::MAX, 0], [0, -1], [0, i32::MAX]] {
            let bytes: Vec<_> = words.into_iter().flat_map(i32::to_le_bytes).collect();
            assert!(ApexInventory::read_original_record(&bytes).is_err());
        }
        let mut p = Parcel::new();
        p.write_i32(0);
        p.write_i32(1);
        p.write_string16(None);
        p.write_string16(Some("/apex/a"));
        p.write_string16(Some("/vendor/apex/a.apex"));
        p.write_i32(2);
        p.write_string16(Some("/data/apex/a.apex"));
        p.write_bool(true);
        assert!(ApexInventory::read_original_record(p.data()).is_err());
    }

    #[test]
    fn partitions_follow_preinstalled_boundaries_and_active_owner_order() {
        let paths = [
            "/system_ext/apex/a",
            "/vendor",
            "/systematic/a",
            "/product/apex/b",
            "/data/apex/c",
        ];
        let inventory = ApexInventory {
            packages: None,
            active: paths
                .iter()
                .enumerate()
                .map(|(i, path)| ActiveApex {
                    module_name: (i != 1).then(|| format!("raw-{i}")),
                    mount_path: format!("/apex/mount-{i}"),
                    preinstalled_path: (*path).into(),
                    factory: i == 1,
                    module_path: format!("/data/apex/{i}"),
                    active_changed: i == 0,
                })
                .collect(),
        };
        let scan = inventory.scan_apexes();
        assert_eq!(
            scan.iter().map(|a| a.partition).collect::<Vec<_>>(),
            [Partition::SystemExt, Partition::Vendor, Partition::Product]
        );
        assert_eq!(scan[0].module_name.as_deref(), Some("raw-0"));
        assert!(scan[0].active_changed && !scan[0].factory);
        assert_eq!(scan[1].module_name, None);
        assert!(scan[1].factory);
        assert_eq!(scan[2].mount_path, "/apex/mount-3");
    }
}
