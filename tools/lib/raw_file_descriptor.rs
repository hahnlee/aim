//! Raw AIDL FileDescriptor over original libbinder_ndk descriptor ownership.
//! Android 16 ParcelFileDescriptor adds presence and hasComm words; raw FD
//! methods omit both. appendFrom retains and relocates actual native FD objects.
use binder::binder_impl::{BorrowedParcel,Deserialize,DeserializeArray,DeserializeOption,
    Parcel,Serialize,SerializeArray,SerializeOption};
use std::os::fd::{AsFd,AsRawFd,BorrowedFd,IntoRawFd,OwnedFd,RawFd};

#[derive(Debug,PartialEq,Eq)]
pub struct RawFileDescriptor(binder::ParcelFileDescriptor);
impl RawFileDescriptor {
    pub fn new<F:Into<OwnedFd>>(fd:F)->Self {Self(binder::ParcelFileDescriptor::new(fd))}
}
impl From<OwnedFd> for RawFileDescriptor {fn from(fd:OwnedFd)->Self {Self::new(fd)}}
impl From<binder::ParcelFileDescriptor> for RawFileDescriptor {
    fn from(fd:binder::ParcelFileDescriptor)->Self {Self(fd)}
}
impl From<RawFileDescriptor> for OwnedFd {fn from(fd:RawFileDescriptor)->Self {fd.0.into()}}
impl AsRef<OwnedFd> for RawFileDescriptor {fn as_ref(&self)->&OwnedFd {self.0.as_ref()}}
impl AsFd for RawFileDescriptor {fn as_fd(&self)->BorrowedFd<'_> {self.0.as_ref().as_fd()}}
impl AsRawFd for RawFileDescriptor {fn as_raw_fd(&self)->RawFd {self.0.as_raw_fd()}}
impl IntoRawFd for RawFileDescriptor {fn into_raw_fd(self)->RawFd {self.0.into_raw_fd()}}

const PREFIX:i32=8;
impl Serialize for RawFileDescriptor {
    fn serialize(&self,parcel:&mut BorrowedParcel<'_>)->Result<(),binder::StatusCode> {
        let mut encoded=Parcel::new();encoded.write(&self.0)?;
        let size=encoded.get_data_size();
        if size<=PREFIX{return Err(binder::StatusCode::BAD_VALUE);}
        // This is inside the just-created native parcel's data bounds.
        unsafe {encoded.set_data_position(0)?;}
        if encoded.read::<i32>()?!=1||encoded.read::<i32>()?!=0{return Err(binder::StatusCode::BAD_VALUE);}
        parcel.append_from(&encoded,PREFIX,size-PREFIX)
    }
}
impl Deserialize for RawFileDescriptor {
    type UninitType=Option<Self>;
    fn uninit()->Self::UninitType {None}
    fn from_init(value:Self)->Self::UninitType {Some(value)}
    fn deserialize(parcel:&BorrowedParcel<'_>)->Result<Self,binder::StatusCode> {
        let start=parcel.get_data_position();let remaining=parcel.get_data_size()-start;
        if remaining<=0{return Err(binder::StatusCode::NOT_ENOUGH_DATA);}
        let mut encoded=Parcel::new();encoded.write(&1_i32)?;encoded.write(&0_i32)?;
        encoded.borrowed().append_from(parcel,start,remaining)?;
        // AParcel's read transfers an independently owned descriptor. Native
        // appendFrom manages temporary descriptors and their object offsets.
        unsafe {encoded.set_data_position(0)?;}
        let fd=encoded.read::<binder::ParcelFileDescriptor>()?;
        let consumed=encoded.get_data_position()-PREFIX;
        if consumed<=0||consumed>remaining{return Err(binder::StatusCode::BAD_VALUE);}
        // The consumed range was read from the copied original parcel range.
        unsafe {parcel.set_data_position(start+consumed)?;}
        Ok(Self(fd))
    }
}
impl SerializeArray for RawFileDescriptor {}
impl DeserializeArray for RawFileDescriptor {}
impl SerializeOption for RawFileDescriptor {
    fn serialize_option(value:Option<&Self>,parcel:&mut BorrowedParcel<'_>)->Result<(),binder::StatusCode> {
        value.ok_or(binder::StatusCode::UNEXPECTED_NULL)?.serialize(parcel)
    }
}
impl DeserializeOption for RawFileDescriptor {
    fn deserialize_option(parcel:&BorrowedParcel<'_>)->Result<Option<Self>,binder::StatusCode> {
        Self::deserialize(parcel).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read,Write};
    #[test]
    fn raw_fd_wire_omits_pfd_headers_retains_ownership_and_following_arguments() {
        let (writer,mut reader)=std::os::unix::net::UnixStream::pair().unwrap();
        let raw=RawFileDescriptor::from(OwnedFd::from(writer));
        let boxed=binder::ParcelFileDescriptor::new(raw.as_ref().try_clone().unwrap());
        let mut pfd_wire=Parcel::new();pfd_wire.write(&boxed).unwrap();
        let mut wire=Parcel::new();wire.write(&raw).unwrap();
        assert_eq!(wire.get_data_size()+PREFIX,pfd_wire.get_data_size());
        wire.write(&boxed).unwrap();wire.write(&0x11223344_i32).unwrap();
        drop(raw);drop(boxed);drop(pfd_wire);
        unsafe {wire.set_data_position(0).unwrap();}
        let decoded=wire.read::<RawFileDescriptor>().unwrap();
        let second=wire.read::<binder::ParcelFileDescriptor>().unwrap();
        assert_eq!(wire.read::<i32>().unwrap(),0x11223344);
        assert_eq!(wire.get_data_position(),wire.get_data_size());
        drop(wire);drop(second);
        // The actual NDK reader supplies owned duplicates; dropping both the
        // source owner and every Parcel must not invalidate the returned FD.
        let mut file=std::fs::File::from(OwnedFd::from(decoded));
        file.write_all(b"raw").unwrap();
        let mut message=[0;3];reader.read_exact(&mut message).unwrap();
        assert_eq!(&message,b"raw");
    }
}
