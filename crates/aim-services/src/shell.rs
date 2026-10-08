//! `cmd <service> ...`: `IBinder.SHELL_COMMAND_TRANSACTION`, read as
//! `Binder.onTransact` reads it and answered as `ShellCommand.exec`
//! answers it: text on the caller's output and error, then the result
//! through its `ResultReceiver`.

use std::io::Write;
use std::sync::Arc;

use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Binder, Parcel, Reader, Result};
use aim_service_aidl::{WriteParcelable, com_android_internal_os_iresultreceiver as receiver};

/// `IBinder.SHELL_COMMAND_TRANSACTION`.
pub const SHELL_COMMAND_TRANSACTION: u32 = u32::from_be_bytes(*b"_CMD");

/// One shell command's arguments and streams.
pub struct ShellCommand {
    pub args: Vec<String>,
    /// The next argument `next_arg` returns.
    next: usize,
    input: Option<std::fs::File>,
    out: Option<std::fs::File>,
    err: Option<std::fs::File>,
    result: Option<Strong>,
    callback: Option<Strong>,
}

/// A result's data, which `ShellCommand` never sends.
enum NoBundle {}

impl WriteParcelable for NoBundle {
    fn write_to(&self, _: &mut Parcel) {
        match *self {}
    }
}

impl ShellCommand {
    /// A call's data (`IBinder::shellCommand`): the input, output and error
    /// descriptors, the arguments, the shell callback and the result
    /// receiver.
    pub fn read(process: &Arc<LocalProcess>, r: &mut Reader<'_>) -> Result<Self> {
        let file = |r: &mut Reader<'_>| -> Result<Option<std::fs::File>> {
            let fd = r.read_fd()?;
            Ok(process
                .file(fd)
                .and_then(|f| aim_binder_host::server::file_fd(&f))
                .map(std::fs::File::from))
        };
        let input=file(r)?;
        let out = file(r)?;
        let err = file(r)?;
        let n = r.read_i32()?;
        let args = (0..n.max(0))
            .map(|_| r.read_string16().map(Option::unwrap_or_default))
            .collect::<Result<Vec<_>>>()?;
        let callback=match r.read_binder()?{Some(Binder::Handle(handle))=>Some(process.strong(handle)),_=>None};
        let result = match r.read_binder()? {
            Some(Binder::Handle(handle)) => Some(process.strong(handle)),
            _ => None,
        };
        // Without an output nothing runs, and no result is sent.
        let err = err.or_else(|| out.as_ref().and_then(|o| o.try_clone().ok()));
        Ok(Self {
            args,
            next: 1,
            input,
            out,
            err,
            result,
            callback,
        })
    }

    /// Actual transferred input for streaming install commands.
    pub fn input(&mut self)->Option<&mut std::fs::File>{self.input.as_mut()}
    pub fn output(&mut self)->Option<&mut std::fs::File>{self.out.as_mut()}
    pub fn error(&mut self)->Option<&mut std::fs::File>{self.err.as_mut()}
    pub fn open_input(&mut self,process:&Arc<LocalProcess>,path:Option<&str>)->std::result::Result<std::fs::File,aim_binder_host::parcel::Exception>{
        use aim_binder_host::parcel::{Exception,EX_ILLEGAL_STATE};
        let error=|message:String|Exception::new(EX_ILLEGAL_STATE,message);
        if path.is_none()||path==Some("-"){
            return self.input.as_ref().ok_or_else(||error("shell input unavailable".into()))?.try_clone().map_err(|cause|error(cause.to_string()));
        }
        let callback=self.callback.as_ref().ok_or_else(||error("shell file callback unavailable".into()))?;
        let mut request=Parcel::new();
        aim_service_aidl::com_android_internal_os_ishellcallback::OpenFile{path:path.map(str::to_owned),se_linux_context:Some("u:r:system_server:s0".into()),mode:Some("r".into())}.write(&mut request);
        let reply=callback.transact(aim_service_aidl::com_android_internal_os_ishellcallback::OPEN_FILE,&request,false).map_err(|code|error(format!("shell openFile transport: {code}")))?;
        let mut reader=reply.reader();reader.read_exception().map_err(|code|error(format!("shell openFile reply: {code}")))??;
        if reader.read_i32().map_err(|code|error(format!("shell openFile presence: {code}")))?!=1{return Err(error("shell openFile returned no descriptor".into()));}
        if reader.read_i32().map_err(|code|error(format!("shell openFile commfd: {code}")))?!=0{return Err(error("shell openFile unexpected communication descriptor".into()));}
        let fd=reader.read_fd().map_err(|code|error(format!("shell openFile fd: {code}")))?;
        if reader.remaining()!=0{return Err(error("shell openFile reply trailing bytes".into()));}
        process.file(fd).and_then(|file|aim_binder_host::server::file_fd(&file)).map(std::fs::File::from).ok_or_else(||error("shell openFile descriptor owner missing".into()))
    }
    /// Whether the command runs: `Binder.onTransact` runs it only with an
    /// output.
    pub fn has_output(&self) -> bool {
        self.out.is_some()
    }

    /// The command, the first argument.
    pub fn command(&self) -> Option<&str> {
        self.args.first().map(String::as_str)
    }

    /// `getNextArg`.
    pub fn next_arg(&mut self) -> Option<String> {
        let arg = self.args.get(self.next).cloned();
        self.next += 1;
        arg
    }

    /// `getNextArgRequired`: the argument, or the exception it throws as
    /// `toString` prints it.
    pub fn next_arg_required(&mut self) -> std::result::Result<String, String> {
        let previous = self.args[self.next - 1].clone();
        self.next_arg().ok_or_else(|| {
            format!("java.lang.IllegalArgumentException: Argument expected after \"{previous}\"")
        })
    }

    /// A line on the output (`getOutPrintWriter().println`).
    pub fn println(&mut self, line: &str) {
        if let Some(out) = &mut self.out {
            let _ = writeln!(out, "{line}");
        }
    }

    /// A line on the error output (`getErrPrintWriter().println`).
    pub fn eprintln(&mut self, line: &str) {
        if let Some(err) = &mut self.err {
            let _ = writeln!(err, "{line}");
        }
    }

    /// What `exec` prints for an exception the command threw.
    pub fn exception(&mut self, exception: &str) {
        let command = self.command().unwrap_or_default().to_string();
        if let Some(err) = &mut self.err {
            let _ = write!(
                err,
                "\nException occurred while executing '{command}':\n{exception}\n"
            );
        }
    }

    /// Sends the command's result: `ResultReceiver.send(result, null)`,
    /// one-way.
    pub fn finish(self, result: i32) {
        let Some(to) = self.result else { return };
        let mut data = Parcel::new();
        receiver::Send::<NoBundle> {
            result_code: result,
            result_data: None,
        }
        .write(&mut data);
        let _ = to.transact(receiver::SEND, &data, true);
    }
}
