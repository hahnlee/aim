//! Controlling-terminal state follows Linux tty_jobctrl.c, not Darwin revocation.
use super::Pair;
use crate::{posix_broker::{Credentials,read_credentials},process_namespace::ProcessIdentity};
use std::{io,path::PathBuf,sync::{Arc,Weak}};
fn error(code:i32)->io::Error{io::Error::from_raw_os_error(code)}
#[derive(Clone)]
struct Binding{leader:ProcessIdentity,pair:Weak<Pair>,foreground:i32,detached:Vec<ProcessIdentity>,members:Vec<ProcessIdentity>}
#[derive(Clone)]
pub(super) struct Sessions{table:Option<PathBuf>,bindings:Vec<Binding>}
impl Sessions{
    pub(super) fn new()->Self{Self{table:None,bindings:Vec::new()}}
    pub(super) fn set_table(&mut self,table:PathBuf){self.table=Some(table);}
    pub(super) fn inherit(&mut self,parent:ProcessIdentity,child:ProcessIdentity)->io::Result<()> {
        self.credentials(parent)?;self.credentials(child)?;
        let mut info:libc::proc_bsdinfo=unsafe{std::mem::zeroed()};let size=std::mem::size_of_val(&info)as i32;
        if unsafe{libc::proc_pidinfo(child.host_pid,libc::PROC_PIDTBSDINFO,1,(&mut info as*mut libc::proc_bsdinfo).cast(),size)}!=size||info.pbi_ppid as i32!=parent.host_pid||!parent.is_live()||!child.is_live(){return Err(error(libc::EPERM));}
        let sid=Self::session(child)?;
        for binding in &mut self.bindings{
            if binding.detached.contains(&parent)&&!binding.detached.contains(&child){binding.detached.push(child);}
            if binding.leader.host_pid==sid&&!binding.members.contains(&child){binding.members.push(child);}
        }
        Ok(())
    }
    fn credentials(&self,actor:ProcessIdentity)->io::Result<Credentials>{
        read_credentials(self.table.as_deref().ok_or_else(||error(libc::EPERM))?,actor)
    }
    fn session(actor:ProcessIdentity)->io::Result<i32>{
        if !actor.is_live(){return Err(error(libc::ESRCH));}
        let sid=unsafe{libc::getsid(actor.host_pid)};if sid<0{Err(io::Error::last_os_error())}else{Ok(sid)}
    }
    fn pair_index(&self,pair:&Arc<Pair>)->Option<usize>{self.bindings.iter().position(|binding|binding.pair.upgrade().is_some_and(|current|Arc::ptr_eq(&current,pair)))}
    pub(super) fn claim(&mut self,pair:&Arc<Pair>,actor:ProcessIdentity,force:bool,automatic:bool,readable:bool)->io::Result<bool>{
        let sid=Self::session(actor)?;
        if sid!=actor.host_pid{return if automatic{Ok(false)}else{Err(error(libc::EPERM))};}
        let credentials=self.credentials(actor)?;
        if let Some(current)=self.bindings.iter().find(|binding|binding.leader==actor){
            return if current.pair.upgrade().is_some_and(|current|Arc::ptr_eq(&current,pair)){Ok(false)}else if automatic{Ok(false)}else{Err(error(libc::EPERM))};
        }
        let previous=self.pair_index(pair);
        if previous.is_some(){
            if automatic{return Ok(false);}
            if !force||credentials.effective_capabilities&(1<<21)==0{return Err(error(libc::EPERM));}
        }
        if !readable&&credentials.effective_capabilities&(1<<21)==0{return Err(error(libc::EPERM));}
        let foreground=unsafe{libc::getpgid(actor.host_pid)};
        if foreground<0{return Err(io::Error::last_os_error());}
        let members=self.members(sid,None)?;
        if let Some(index)=previous{self.bindings.remove(index);}
        self.bindings.push(Binding{leader:actor,pair:Arc::downgrade(pair),foreground,detached:Vec::new(),members});
        Ok(true)
    }
    fn controlling(&self,index:usize,actor:ProcessIdentity)->io::Result<()> {
        self.credentials(actor)?;
        let binding=&self.bindings[index];
        if Self::session(actor)?!=binding.leader.host_pid||binding.detached.contains(&actor){return Err(error(libc::ENOTTY));}Ok(())
    }
    pub(super) fn state(&self,pair:&Arc<Pair>,actor:ProcessIdentity,master:bool)->io::Result<(i32,i32)>{
        let index=self.pair_index(pair).ok_or_else(||error(libc::ENOTTY))?;
        if !master{self.controlling(index,actor)?;}
        let binding=&self.bindings[index];Ok((binding.leader.host_pid,binding.foreground))
    }
    pub(super) fn set_foreground(&mut self,pair:&Arc<Pair>,actor:ProcessIdentity,pgrp:i32)->io::Result<()> {
        if pgrp<0{return Err(error(libc::EINVAL));}
        let index=self.pair_index(pair).ok_or_else(||error(libc::ENOTTY))?;
        self.controlling(index,actor)?;
        let sid=self.bindings[index].leader.host_pid;
        let members=self.members(sid,Some(pgrp))?;
        if members.is_empty(){
            let target=ProcessIdentity::running(pgrp)?;
            self.credentials(target)?;
            if Self::session(target)?!=sid{return Err(error(libc::EPERM));}
            if unsafe{libc::getpgid(pgrp)}!=pgrp{return Err(error(libc::ESRCH));}
        }
        for member in members{if !self.bindings[index].members.contains(&member){self.bindings[index].members.push(member);}}
        self.bindings[index].foreground=pgrp;Ok(())
    }
    fn members(&self,sid:i32,pgrp:Option<i32>)->io::Result<Vec<ProcessIdentity>>{
        let table=self.table.as_ref().ok_or_else(||error(libc::EPERM))?;
        let mut members=Vec::new();
        for entry in std::fs::read_dir(table)?{
            let entry=entry?;let Some(pid)=entry.file_name().to_str().and_then(|name|name.parse::<i32>().ok())else{continue;};
            if pid<=1{continue;}
            let actor=match ProcessIdentity::running(pid){Ok(actor)=>actor,Err(failure)if failure.raw_os_error()==Some(libc::ESRCH)=>continue,Err(failure)=>return Err(failure)};
            if unsafe{libc::getsid(pid)}!=sid||pgrp.is_some_and(|pgrp|unsafe{libc::getpgid(pid)}!=pgrp){continue;}
            match read_credentials(table,actor){Ok(_)=>members.push(actor),Err(failure)if matches!(failure.raw_os_error(),Some(libc::ESRCH)|Some(libc::ENOENT))=>{},Err(failure)=>return Err(failure)}
        }Ok(members)
    }
    fn signal(&self,binding:&Binding,signal:i32)->io::Result<()> {
        if ProcessIdentity::running(binding.leader.host_pid).is_ok_and(|current|current!=binding.leader){return Err(error(libc::ESTALE));}
        for member in &binding.members{
            if !member.is_live()||unsafe{libc::getsid(member.host_pid)}!=binding.leader.host_pid
                ||unsafe{libc::getpgid(member.host_pid)}!=binding.foreground{continue;}
            if unsafe{libc::kill(member.host_pid,signal)}<0{let failure=io::Error::last_os_error();if failure.raw_os_error()!=Some(libc::ESRCH){return Err(failure);}}
        }Ok(())
    }
    pub(super) fn detach(&mut self,pair:&Arc<Pair>,actor:ProcessIdentity)->io::Result<()> {
        let index=self.pair_index(pair).ok_or_else(||error(libc::ENOTTY))?;
        self.controlling(index,actor)?;
        if self.bindings[index].leader==actor{
            self.signal(&self.bindings[index],libc::SIGHUP)?;
            self.signal(&self.bindings[index],libc::SIGCONT)?;
            self.bindings.remove(index);
        }else if !self.bindings[index].detached.contains(&actor){self.bindings[index].detached.push(actor);}
        Ok(())
    }
    pub(super) fn leader_exit(&mut self,pid:i32)->io::Result<()> {
        let Some(index)=self.bindings.iter().position(|binding|binding.leader.host_pid==pid&&!binding.leader.is_live())else{return Ok(());};
        // PTY exit does not vhangup or revoke the keeper. Linux sends SIGHUP.
        self.signal(&self.bindings[index],libc::SIGHUP)?;self.bindings.remove(index);Ok(())
    }
    pub(super) fn master_closed(&mut self,pair:&Arc<Pair>)->io::Result<()> {
        let Some(index)=self.pair_index(pair)else{return Ok(());};
        self.signal(&self.bindings[index],libc::SIGHUP)?;
        self.signal(&self.bindings[index],libc::SIGCONT)?;
        self.bindings.remove(index);Ok(())
    }
}

#[cfg(test)]
#[path = "session_owner_tests.rs"]
mod owner_tests;
