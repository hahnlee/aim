//! BaseParceledListSlice android-16 image protocol (AOSP Apache-2.0):
//! 64 KiB inline/retriever windows, typed creator, and one-shot retained list.
use aim_binder_host::{local::{LocalProcess,Service,Call,Reply},parcel::{Parcel,Exception,BAD_VALUE,UNKNOWN_TRANSACTION}};
use aim_service_aidl::WriteParcelable;
use std::sync::{Arc,Mutex};
pub const MAX_IPC_SIZE:usize=64*1024;

pub struct Slice<T> {
    process:Arc<LocalProcess>,
    creator:&'static str,
    items:Mutex<Option<Vec<T>>>,
}
impl<T> Slice<T> {
    pub fn new(process:Arc<LocalProcess>,creator:&'static str,items:Vec<T>)->Self {
        Self{process,creator,items:Mutex::new(Some(items))}
    }
}
impl<T:WriteParcelable+Send+Sync+'static> WriteParcelable for Slice<T> {
    fn write_to(&self,parcel:&mut Parcel) {
        // Framework BaseParceledListSlice is explicitly parcelled only once.
        let items=self.items.lock().unwrap().take().expect("ParceledListSlice was already parcelled");
        let count=i32::try_from(items.len()).expect("ParceledListSlice length exceeds Android range");
        parcel.write_i32(count);
        if count==0{return;}
        parcel.write_string16(Some(self.creator));
        let mut index=0;
        while index<items.len()&&parcel.data().len()<MAX_IPC_SIZE {
            parcel.write_i32(1);items[index].write_to(parcel);index+=1;
        }
        if index<items.len() {
            parcel.write_i32(0);
            let binder=self.process.add_service(Arc::new(Retriever{items:Mutex::new(Some(items))}));
            parcel.write_binder(Some(binder));
        }
    }
}
struct Retriever<T> {items:Mutex<Option<Vec<T>>>}
impl<T:WriteParcelable+Send+Sync+'static> Service for Retriever<T> {
    fn descriptor(&self)->&str{""}
    fn has_descriptor(&self)->bool{false}
    fn transact(&self,call:&mut Call<'_>)->Reply {
        if call.code!=1{return Err(UNKNOWN_TRANSACTION);}
        let index=call.data.read_i32()?;
        if call.data.remaining()!=0{return Err(BAD_VALUE);}
        let mut state=self.items.lock().unwrap();
        let mut reply=Parcel::new();
        let Some(items)=state.as_ref()else {
            reply.write_exception(&Exception::illegal_argument("Attempt to transfer null list, did transfer finish?"));
            return Ok(reply);
        };
        let Ok(mut index)=usize::try_from(index)else {
            reply.write_exception(&Exception::illegal_argument("negative list index"));return Ok(reply);
        };
        if index>items.len(){reply.write_exception(&Exception::illegal_argument("list index exceeds count"));return Ok(reply);}
        reply.write_no_exception();
        while index<items.len()&&reply.data().len()<MAX_IPC_SIZE {
            reply.write_i32(1);items[index].write_to(&mut reply);index+=1;
        }
        if index<items.len(){reply.write_i32(0);}else{*state=None;}
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_driver::{Driver,Device,Credentials,GuestProcess,Errno,File,errno,uapi::*};
    use aim_binder_host::parcel::{Binder,Reader};
    use crate::package::info::ApplicationInfo;
    struct NoMemory;
    impl GuestProcess for NoMemory {
        fn copy_from_user(&mut self,_:u64,_:&mut[u8])->Result<(),Errno>{Err(errno::EFAULT)}
        fn copy_to_user(&mut self,_:u64,_:&[u8])->Result<(),Errno>{Err(errno::EFAULT)}
        fn get_file(&mut self,_:u32)->Result<File,Errno>{Err(errno::EBADF)}
        fn install_file(&mut self,_:File)->Result<u32,Errno>{Err(errno::EBADF)}
        fn close_fd(&mut self,_:u32){panic!("unexpected fd")}
    }
    struct ProcessGuard {driver:Arc<Driver>,processes:Vec<Arc<LocalProcess>>}
    impl Drop for ProcessGuard {fn drop(&mut self){for process in &self.processes{self.driver.release(process.proc_handle());}}}
    struct Applications {process:std::sync::Weak<LocalProcess>,items:Vec<ApplicationInfo>}
    impl Service for Applications {
        fn descriptor(&self)->&str{"fixture.applications"}
        fn transact(&self,call:&mut Call<'_>)->Reply {
            if call.code!=7{return Err(UNKNOWN_TRANSACTION)}
            let slice=Slice::new(self.process.upgrade().unwrap(),"android.content.pm.ApplicationInfo",self.items.clone());
            let mut parcel=Parcel::new();parcel.write_no_exception();aim_service_aidl::write_typed(&mut parcel,Some(&slice));Ok(parcel)
        }
    }
    #[test]
    fn application_slice_transfers_over_one_mib_through_bounded_binder_chunks() {
        let driver=Driver::new();let open=|pid|LocalProcess::open(&driver,Device::Binder,Credentials{pid,euid:1000,security_context:None});
        let server=open(99201);let client=open(99202);
        let _guard=ProcessGuard{driver:driver.clone(),processes:vec![server.clone(),client.clone()]};
        let items=(0..300).map(|index|{
            let mut app=ApplicationInfo::default();app.uid=10000+index;
            app.item.package_name=Some(format!("fixture.application{index}"));
            app.item.non_localized_label=Some("a".repeat(6000));
            app.source_dir=Some(format!("/data/app/fixture{index}/base.apk"));app
        }).collect::<Vec<_>>();
        let expected=items.iter().map(|item|{let mut p=Parcel::new();item.write_to(&mut p);p.data().to_vec()}).collect::<Vec<_>>();
        assert!(expected.iter().map(Vec::len).sum::<usize>()>1024*1024);
        let largest=expected.iter().map(Vec::len).max().unwrap();
        let Binder::Local(ptr)=server.add_service(Arc::new(Applications{process:Arc::downgrade(&server),items}))else{unreachable!()};
        let mut object=FlatBinderObject{kind:BINDER_TYPE_BINDER,flags:0,binder:ptr,cookie:ptr}.encode();
        driver.ioctl(server.proc_handle(),99203,BINDER_SET_CONTEXT_MGR_EXT,&mut object,&mut NoMemory).unwrap();server.start();client.start();
        let received=client.strong(0).transact(7,&Parcel::new(),false).unwrap();
        let first=received.into_parcel();assert!(first.data().len()<MAX_IPC_SIZE+largest+128);
        let mut reader=first.reader();reader.read_exception().unwrap().unwrap();assert_eq!(reader.read_i32().unwrap(),1);
        assert_eq!(reader.read_i32().unwrap(),300);assert_eq!(reader.read_string16().unwrap().as_deref(),Some("android.content.pm.ApplicationInfo"));
        fn take(reader:&mut Reader<'_>,expected:&[Vec<u8>],index:&mut usize){
            while *index<expected.len(){if reader.read_i32().unwrap()==0{return;}
                let start=reader.position();reader.set_position(start+expected[*index].len());
                assert_eq!(reader.since(start).0,expected[*index].as_slice());*index+=1;
            }
        }
        let mut index=0;take(&mut reader,&expected,&mut index);assert!(index>0&&index<300);
        let Some(Binder::Handle(handle))=reader.read_binder().unwrap()else{panic!("retriever missing")};assert_eq!(reader.remaining(),0);
        let retriever=client.strong(handle);let mut chunks=0;
        while index<300{
            let before=index;let mut request=Parcel::new();request.write_i32(index as i32);
            let chunk=retriever.transact(1,&request,false).unwrap().into_parcel();
            assert!(chunk.data().len()<MAX_IPC_SIZE+largest+4);
            let mut reader=chunk.reader();reader.read_exception().unwrap().unwrap();take(&mut reader,&expected,&mut index);
            assert_eq!(reader.remaining(),0);assert!(index>before);chunks+=1;
        }
        assert!(chunks>1);
        let mut request=Parcel::new();request.write_i32(0);
        let exhausted=retriever.transact(1,&request,false).unwrap();
        assert_eq!(exhausted.reader().read_exception().unwrap().unwrap_err().message,"Attempt to transfer null list, did transfer finish?");
    }
}
