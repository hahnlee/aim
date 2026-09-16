//! Test tool: input is command values emitted by the original C++ UAPI header.
use darwin_art_binder_device::command::{KINDS, decode};
use std::io::{self, BufRead};

fn main() {
    let words: Vec<u32> = io::stdin()
        .lock()
        .lines()
        .map(|line| u32::from_str_radix(&line.expect("catalog read"), 16).expect("UAPI hex word"))
        .collect();
    assert_eq!(words.len(), KINDS.len());
    for (word, expected) in words.into_iter().zip(KINDS) {
        let original_size = ((word >> 16) & 0x3fff) as usize;
        let mut bytes = word.to_le_bytes().to_vec();
        bytes.resize(4 + original_size, 0);
        let (command, consumed) = decode(&bytes).expect("decode original command");
        assert_eq!(command.kind, expected);
        assert_eq!(command.payload.len(), original_size);
        assert_eq!(consumed, bytes.len());
    }
    println!("Original Binder UAPI -> Rust decoder: all 22 command words/sizes PASS");
}
