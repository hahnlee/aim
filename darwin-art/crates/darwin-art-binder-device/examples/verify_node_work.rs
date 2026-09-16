use darwin_art_binder_device::{node_wire, node_work::Notification};
use std::io::{self, Read};
fn main() -> io::Result<()> {
    let mut original = Vec::new();
    io::stdin().read_to_end(&mut original)?;
    let commands = [
        Notification::Increfs,
        Notification::Acquire,
        Notification::Release,
        Notification::Decrefs,
    ];
    let mut encoded = [0; 80];
    assert_eq!(
        node_wire::encode(
            0x1122334455667788,
            0x8877665544332211,
            &commands,
            &mut encoded
        )?,
        80
    );
    assert_eq!(original, encoded);
    println!("Original Binder UAPI -> Rust: all 4 node work records byte-for-byte PASS");
    Ok(())
}
