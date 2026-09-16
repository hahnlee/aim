use darwin_art_binder_device::{
    object_fields::Fields,
    objects::{self, Kind},
};
use std::io::{self, Read};

fn main() -> io::Result<()> {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;
    let flags = 0x12345678;
    let pointer = 0xabcdef0123456789;
    let cookie = 0x8877665544332211;
    let cases = [
        (
            Kind::Binder,
            Fields::Node {
                weak: false,
                flags,
                pointer,
                cookie,
            },
        ),
        (
            Kind::WeakBinder,
            Fields::Node {
                weak: true,
                flags,
                pointer,
                cookie,
            },
        ),
        (
            Kind::Handle,
            Fields::Handle {
                weak: false,
                flags,
                handle: 42,
                cookie,
            },
        ),
        (
            Kind::WeakHandle,
            Fields::Handle {
                weak: true,
                flags,
                handle: 42,
                cookie,
            },
        ),
        (Kind::Fd, Fields::Fd { fd: 42, cookie }),
        (
            Kind::Buffer,
            Fields::Buffer {
                flags,
                pointer,
                length: cookie,
                parent: 13,
                parent_offset: 29,
            },
        ),
        (
            Kind::FdArray,
            Fields::FdArray {
                count: cookie,
                parent: 13,
                parent_offset: 29,
            },
        ),
    ];
    let mut cursor = 0;
    for (kind, expected) in cases {
        let end = cursor + kind.size();
        // Place original C++ bytes at four-byte alignment, not native u64 alignment.
        let mut data = vec![0; 4];
        data.extend_from_slice(input.get(cursor..end).expect("complete C++ fixture"));
        let offsets = 4u64.to_le_bytes();
        let objects = objects::validate(&data, &offsets).unwrap();
        assert_eq!(objects[0].kind(), kind);
        assert_eq!(objects[0].fields(), expected);
        cursor = end;
    }
    assert_eq!(cursor, input.len());
    println!("Original Binder C++ fields -> Rust: all 7 object variants PASS");
    Ok(())
}
