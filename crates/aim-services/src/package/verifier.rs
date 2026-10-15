//! VerifierDeviceIdentity's installation-local random nonce, android-16.0.0_r1.
//! Copyright AOSP Apache-2.0; this is not a hardware/device identity.
use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader};
use aim_service_aidl::{ReadParcelable, WriteParcelable};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity(pub u64);
impl Identity {
    pub fn generate() -> Self {
        let mut value = 0u64;
        unsafe {
            libc::arc4random_buf((&mut value as *mut u64).cast(), std::mem::size_of::<u64>());
        }
        Self(value)
    }
    pub fn parse(input: &str) -> Result<Self, String> {
        let mut value = 0u64;
        let mut count = 0;
        for c in input.chars() {
            let digit = match c {
                'A'..='Z' => c as u8 - b'A',
                'a'..='z' => c as u8 - b'a',
                '2'..='7' => c as u8 - b'2' + 26,
                '0' => b'O' - b'A',
                '1' => b'I' - b'A',
                '-' => continue,
                _ => return Err("invalid verifier base32 character".into()),
            };
            count += 1;
            if count > 13 || count == 1 && digit > 15 {
                return Err("verifier identity overflow".into());
            }
            value = (value << 5) | u64::from(digit);
        }
        if count != 13 {
            return Err("verifier identity must have13 base32 characters".into());
        }
        Ok(Self(value))
    }
    pub fn encoded(self) -> String {
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
        let mut out = String::with_capacity(16);
        for index in 0..13 {
            if index > 0 && index % 4 == 0 {
                out.push('-');
            }
            out.push(alphabet[((self.0 >> (5 * (12 - index))) & 31) as usize] as char);
        }
        out
    }
}
impl WriteParcelable for Identity {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i64(self.0 as i64);
    }
}
impl ReadParcelable for Identity {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        if r.remaining() < 8 {
            return Err(BAD_VALUE);
        }
        Ok(Self(r.read_i64()? as u64))
    }
}
