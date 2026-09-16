use darwin_art_binder_device::transaction_request::{self, SenderRange, Target};
use std::io::{self, Read};

fn main() -> io::Result<()> {
    let mut original = vec![0xff]; // Also exercise an unaligned stream start.
    io::stdin().read_to_end(&mut original)?;
    let mut consumed = 1;
    for index in 0..4 {
        let (request, bytes) = transaction_request::decode(&original[consumed..]).unwrap();
        assert_eq!(bytes, if index < 2 { 68 } else { 76 });
        assert_eq!(
            request.target(),
            if index % 2 == 0 {
                Target::Handle(0x87654321)
            } else {
                Target::Reply
            }
        );
        assert_eq!(request.code(), 0x12345678);
        assert_eq!(request.flags(), 0x11);
        assert_eq!(
            request.data(),
            SenderRange {
                address: 0xabcdef0123456789,
                size: 0x123456789
            }
        );
        assert_eq!(
            request.offsets(),
            SenderRange {
                address: 0xfedcba9876543210,
                size: 0x234567890
            }
        );
        assert_eq!(
            request.extra_size(),
            if index < 2 { 0 } else { 0x345678908 }
        );
        consumed += bytes;
    }
    assert_eq!(consumed, original.len());
    println!("Original Binder UAPI -> Rust: all 4 transaction headers/64-bit fields PASS");
    Ok(())
}
