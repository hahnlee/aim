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
    out: Option<std::fs::File>,
    err: Option<std::fs::File>,
    result: Option<Strong>,
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
        file(r)?; // input, which no command here reads
        let out = file(r)?;
        let err = file(r)?;
        let n = r.read_i32()?;
        let args = (0..n.max(0))
            .map(|_| r.read_string16().map(Option::unwrap_or_default))
            .collect::<Result<Vec<_>>>()?;
        r.read_binder()?; // the shell callback, for files no command opens
        let result = match r.read_binder()? {
            Some(Binder::Handle(handle)) => Some(process.strong(handle)),
            _ => None,
        };
        // Without an output nothing runs, and no result is sent.
        let err = err.or_else(|| out.as_ref().and_then(|o| o.try_clone().ok()));
        Ok(Self {
            args,
            next: 1,
            out,
            err,
            result,
        })
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
