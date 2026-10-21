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
    /// The next argument `next_arg` returns, and a short option's data
    /// attached to it (`mCurArgData`), which comes first.
    next: usize,
    cur_arg_data: Option<String>,
    input: Option<aim_binder_host::server::RetainedFd>,
    out: Option<aim_binder_host::server::RetainedFd>,
    err: Option<aim_binder_host::server::RetainedFd>,
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
        let file = |r: &mut Reader<'_>| -> Result<Option<aim_binder_host::server::RetainedFd>> {
            let fd = r.read_fd()?;
            Ok(process
                .file(fd)
                .and_then(|f| aim_binder_host::server::file_fd(&f)))
        };
        let input = file(r)?;
        let out = file(r)?;
        let err = file(r)?;
        let n = r.read_i32()?;
        let args = (0..n.max(0))
            .map(|_| r.read_string16().map(Option::unwrap_or_default))
            .collect::<Result<Vec<_>>>()?;
        let callback = match r.read_binder()? {
            Some(Binder::Handle(handle)) => Some(process.strong(handle)),
            _ => None,
        };
        let result = match r.read_binder()? {
            Some(Binder::Handle(handle)) => Some(process.strong(handle)),
            _ => None,
        };
        // Without an output nothing runs, and no result is sent.
        let err = err.or_else(|| out.as_ref().and_then(|o| o.try_clone().ok()));
        Ok(Self {
            args,
            next: 1,
            cur_arg_data: None,
            input,
            out,
            err,
            result,
            callback,
        })
    }

    /// Actual transferred input for streaming install commands.
    pub fn input(&mut self) -> Option<&mut aim_binder_host::server::RetainedFd> {
        self.input.as_mut()
    }
    pub fn output(&mut self) -> Option<&mut aim_binder_host::server::RetainedFd> {
        self.out.as_mut()
    }
    pub fn error(&mut self) -> Option<&mut aim_binder_host::server::RetainedFd> {
        self.err.as_mut()
    }
    /// `ShellCommand.openFileForSystem`: `None` after printing why, as it
    /// returns null.
    pub fn open_input(
        &mut self,
        process: &Arc<LocalProcess>,
        path: Option<&str>,
    ) -> std::result::Result<
        Option<aim_binder_host::server::RetainedFd>,
        aim_binder_host::parcel::Exception,
    > {
        use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};
        let error = |message: String| Exception::new(EX_ILLEGAL_STATE, message);
        if path.is_none() || path == Some("-") {
            return self
                .input
                .as_ref()
                .ok_or_else(|| error("shell input unavailable".into()))?
                .try_clone()
                .map(Some)
                .map_err(|cause| error(cause.to_string()));
        }
        let callback = self
            .callback
            .as_ref()
            .ok_or_else(|| error("shell file callback unavailable".into()))?;
        let mut request = Parcel::new();
        aim_service_aidl::com_android_internal_os_ishellcallback::OpenFile {
            path: path.map(str::to_owned),
            se_linux_context: Some("u:r:system_server:s0".into()),
            mode: Some("r".into()),
        }
        .write(&mut request);
        let reply = callback
            .transact(
                aim_service_aidl::com_android_internal_os_ishellcallback::OPEN_FILE,
                &request,
                false,
            )
            .map_err(|code| error(format!("shell openFile transport: {code}")))?;
        let mut reader = reply.reader();
        let opened = match reader
            .read_exception()
            .map_err(|code| error(format!("shell openFile reply: {code}")))?
        {
            Err(failure) => {
                self.eprintln(&format!("Failure opening file: {}", failure.message));
                false
            }
            Ok(()) => {
                reader
                    .read_i32()
                    .map_err(|code| error(format!("shell openFile presence: {code}")))?
                    == 1
            }
        };
        if !opened {
            let path = path.unwrap_or("null");
            self.eprintln(&format!("Error: Unable to open file: {path}"));
            if !path.starts_with("/data/local/tmp/") {
                self.eprintln("Consider using a file under /data/local/tmp/");
            }
            return Ok(None);
        }
        if reader
            .read_i32()
            .map_err(|code| error(format!("shell openFile commfd: {code}")))?
            != 0
        {
            return Err(error(
                "shell openFile unexpected communication descriptor".into(),
            ));
        }
        let fd = reader
            .read_fd()
            .map_err(|code| error(format!("shell openFile fd: {code}")))?;
        if reader.remaining() != 0 {
            return Err(error("shell openFile reply trailing bytes".into()));
        }
        process
            .file(fd)
            .and_then(|file| aim_binder_host::server::file_fd(&file))
            .map(Some)
            .ok_or_else(|| error("shell openFile descriptor owner missing".into()))
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

    /// `getNextOption`: the next argument if it is an option (a short
    /// option's attached data kept for `next_arg`); none at the end, at a
    /// plain argument or after `--`. An option whose data was not taken
    /// is the exception it throws, as `toString` prints it.
    pub fn next_option(&mut self) -> std::result::Result<Option<String>, String> {
        if self.cur_arg_data.is_some() {
            let previous = &self.args[self.next - 1];
            return Err(format!(
                "java.lang.IllegalArgumentException: No argument expected after \"{previous}\""
            ));
        }
        let Some(arg) = self.args.get(self.next).cloned() else {
            return Ok(None);
        };
        if !arg.starts_with('-') {
            return Ok(None);
        }
        self.next += 1;
        if arg == "--" {
            return Ok(None);
        }
        if arg.len() > 2 && !arg[1..].starts_with('-') {
            self.cur_arg_data = Some(arg[2..].to_string());
            return Ok(Some(arg[..2].to_string()));
        }
        Ok(Some(arg))
    }

    /// `getNextArg`.
    pub fn next_arg(&mut self) -> Option<String> {
        if let Some(data) = self.cur_arg_data.take() {
            return Some(data);
        }
        let arg = self.args.get(self.next).cloned()?;
        self.next += 1;
        Some(arg)
    }

    /// `peekNextArg`.
    pub fn peek_next_arg(&self) -> Option<&str> {
        self.cur_arg_data
            .as_deref()
            .or_else(|| self.args.get(self.next).map(String::as_str))
    }

    /// `getNextArgRequired`: the argument, or the exception it throws as
    /// `toString` prints it.
    pub fn next_arg_required(&mut self) -> std::result::Result<String, String> {
        self.next_arg().ok_or_else(|| {
            let previous = &self.args[self.next - 1];
            format!("java.lang.IllegalArgumentException: Argument expected after \"{previous}\"")
        })
    }

    /// Text on the output (`getOutPrintWriter().print`).
    pub fn print(&mut self, text: &str) {
        if let Some(out) = &mut self.out {
            let _ = write!(out, "{text}");
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn command(args: &[&str]) -> ShellCommand {
        ShellCommand {
            args: args.iter().map(|a| a.to_string()).collect(),
            next: 1,
            cur_arg_data: None,
            input: None,
            out: None,
            err: None,
            result: None,
            callback: None,
        }
    }

    /// `getNextOption`: a short option's attached data is its argument,
    /// `--` ends the options, and data not taken is an error.
    #[test]
    fn options_are_read_as_the_original_reads_them() {
        let mut c = command(&["run", "-u10", "--force", "-n", "ns", "--", "-p", "1"]);
        assert_eq!(c.next_option(), Ok(Some("-u".into())));
        assert_eq!(c.peek_next_arg(), Some("10"));
        assert_eq!(c.next_arg_required(), Ok("10".into()));
        assert_eq!(c.next_option(), Ok(Some("--force".into())));
        assert_eq!(c.next_option(), Ok(Some("-n".into())));
        assert_eq!(c.next_arg_required(), Ok("ns".into()));
        assert_eq!(c.next_option(), Ok(None));
        assert_eq!(c.next_arg(), Some("-p".into()));
        assert_eq!(c.next_option(), Ok(None));
        assert_eq!(c.next_arg(), Some("1".into()));
        assert_eq!(
            c.next_arg_required(),
            Err("java.lang.IllegalArgumentException: Argument expected after \"1\"".into())
        );
        let mut c = command(&["run", "-fx", "-s"]);
        assert_eq!(c.next_option(), Ok(Some("-f".into())));
        assert_eq!(
            c.next_option(),
            Err("java.lang.IllegalArgumentException: No argument expected after \"-fx\"".into())
        );
    }
}
