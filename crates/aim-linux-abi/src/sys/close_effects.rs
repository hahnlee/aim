//! Socket carrier retirement runs after actual descriptor destruction and locks.
use crate::errno::Errno;
thread_local!{static SOCKET_CLOSED:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};}
pub(crate) fn note_socket_close(){SOCKET_CLOSED.with(|pending|pending.set(true));}
pub(crate) fn flush()->Result<(),Errno>{
    if !SOCKET_CLOSED.with(|pending|pending.replace(false)){return Ok(());}
    if let Err(error)=super::net::drain_socket_carriers(){
        SOCKET_CLOSED.with(|pending|pending.set(true));return Err(error);
    }
    Ok(())
}
pub(crate) fn run(operation:impl FnOnce()->i64)->i64{
    let result=operation();
    match flush(){Ok(())=>result,Err(error)=>-(error as i64)}
}
