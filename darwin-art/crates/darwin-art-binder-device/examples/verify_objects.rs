//! Test-only original C++ UAPI tag/sizeof catalog comparison.
use darwin_art_binder_device::objects::{Kind, validate};
use std::io::{self, BufRead};
fn main() {
    let entries: Vec<_> = io::stdin()
        .lock()
        .lines()
        .map(|line| {
            let line = line.expect("read UAPI object");
            let (tag, size) = line.split_once(' ').expect("tag size");
            (
                u32::from_str_radix(tag, 16).unwrap(),
                size.parse::<usize>().unwrap(),
            )
        })
        .collect();
    let kinds = [
        Kind::Binder,
        Kind::WeakBinder,
        Kind::Handle,
        Kind::WeakHandle,
        Kind::Fd,
        Kind::Buffer,
        Kind::FdArray,
    ];
    assert_eq!(entries.len(), kinds.len());
    for ((tag, size), kind) in entries.into_iter().zip(kinds) {
        let mut data = vec![0; 4 + size];
        data[4..8].copy_from_slice(&tag.to_le_bytes());
        let objects = validate(&data, &4u64.to_le_bytes()).unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].kind(), kind);
        assert_eq!(objects[0].bytes().len(), size);
    }
    println!("Original Binder UAPI -> Rust: all 7 object tags/sizes PASS");
}
