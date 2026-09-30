//! The command line.

use std::path::PathBuf;

pub const USAGE: &str = "usage: aimctl [--data DIR] COMMAND

  start [--windows]     start the guest in the background (one per data directory)
  run [--windows]       the same in the foreground, until it stops or is stopped
  stop                  stop the guest
  status                boot state, uptime, CPU, memory and data image usage
  shell [COMMAND...]    a shell in the running guest, or COMMAND run by one
  apps                  the launcher apps
  install APK...        install an app, with its splits (the APKs are copied,
                        never changed)
  uninstall PACKAGE     uninstall an app
  open PACKAGE          open an app (in its own window in window mode)
  logs [--follow] [LOGCAT OPTION...]
                        the guest's log

  --data DIR            the data directory (default: ~/Library/Application Support/aim/data)";

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Start { windows: bool },
    Run { windows: bool },
    Stop,
    Status,
    Shell(Vec<String>),
    Apps,
    Install(Vec<PathBuf>),
    Uninstall(String),
    Open(String),
    Logs { follow: bool, args: Vec<String> },
}

#[derive(Debug, PartialEq)]
pub struct Args {
    pub data: Option<PathBuf>,
    pub command: Command,
}

fn usage_error(what: String) -> String {
    format!("{what}\n{USAGE}")
}

/// Parses aimctl's arguments (without the program name). `--data` may
/// come before or after the command's name; a shell's command and the
/// options forwarded to logcat are taken verbatim.
pub fn parse(argv: &[String]) -> Result<Args, String> {
    let mut data = None;
    let mut it = argv.iter();
    let value = |it: &mut std::slice::Iter<String>, option: &str| {
        it.next()
            .map(PathBuf::from)
            .ok_or_else(|| usage_error(format!("{option} needs a value")))
    };
    let name = loop {
        match it.next().map(String::as_str) {
            None => return Err(USAGE.into()),
            Some("--data") => data = Some(value(&mut it, "--data")?),
            Some("-h" | "--help" | "help") => {
                return Ok(Args {
                    data,
                    command: Command::Help,
                });
            }
            Some(s) if s.starts_with('-') => {
                return Err(usage_error(format!("unknown option {s}")));
            }
            Some(s) => break s,
        }
    };
    let (mut windows, mut follow, mut verbatim) = (false, false, false);
    let mut rest: Vec<String> = Vec::new();
    while let Some(a) = it.next() {
        if verbatim || (name == "shell" && !rest.is_empty()) {
            rest.push(a.clone());
            continue;
        }
        match a.as_str() {
            "--" => verbatim = true,
            "--data" => data = Some(value(&mut it, "--data")?),
            "--windows" if matches!(name, "start" | "run") => windows = true,
            "--follow" if name == "logs" => follow = true,
            s if s.starts_with('-') && name != "logs" => {
                return Err(usage_error(format!("{name}: unknown option {s}")));
            }
            _ => rest.push(a.clone()),
        }
    }
    let one = |rest: Vec<String>| match <[String; 1]>::try_from(rest) {
        Ok([v]) => Ok(v),
        Err(_) => Err(usage_error(format!("{name} takes one argument"))),
    };
    let none = |rest: &[String]| match rest {
        [] => Ok(()),
        _ => Err(usage_error(format!("{name} takes no arguments"))),
    };
    let command = match name {
        "start" => none(&rest).map(|_| Command::Start { windows })?,
        "run" => none(&rest).map(|_| Command::Run { windows })?,
        "stop" => none(&rest).map(|_| Command::Stop)?,
        "status" => none(&rest).map(|_| Command::Status)?,
        "apps" => none(&rest).map(|_| Command::Apps)?,
        "shell" => Command::Shell(rest),
        "install" if rest.is_empty() => return Err(usage_error("install needs an APK".into())),
        "install" => Command::Install(rest.into_iter().map(PathBuf::from).collect()),
        "uninstall" => Command::Uninstall(one(rest)?),
        "open" => Command::Open(one(rest)?),
        "logs" => Command::Logs { follow, args: rest },
        _ => return Err(usage_error(format!("unknown command {name}"))),
    };
    Ok(Args { data, command })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &str) -> Result<Args, String> {
        let argv: Vec<String> = args.split_whitespace().map(String::from).collect();
        parse(&argv)
    }

    fn command(args: &str) -> Command {
        p(args).unwrap().command
    }

    #[test]
    fn data_before_or_after_the_command() {
        for args in ["--data /d start --windows", "start --windows --data /d"] {
            assert_eq!(
                p(args).unwrap(),
                Args {
                    data: Some("/d".into()),
                    command: Command::Start { windows: true }
                }
            );
        }
        assert_eq!(p("status").unwrap().data, None);
        assert!(p("--data").is_err());
        assert!(p("status --data").is_err());
    }

    #[test]
    fn commands() {
        assert_eq!(command("start"), Command::Start { windows: false });
        assert_eq!(command("run --windows"), Command::Run { windows: true });
        assert_eq!(command("stop"), Command::Stop);
        assert_eq!(command("apps"), Command::Apps);
        assert_eq!(
            command("install a.apk b.apk"),
            Command::Install(vec!["a.apk".into(), "b.apk".into()])
        );
        assert_eq!(
            command("uninstall com.x"),
            Command::Uninstall("com.x".into())
        );
        assert_eq!(command("open com.x"), Command::Open("com.x".into()));
        assert_eq!(command("--help"), Command::Help);
    }

    #[test]
    fn a_shells_command_is_verbatim() {
        assert_eq!(command("shell"), Command::Shell(vec![]));
        assert_eq!(
            command("shell --data /d ls -l --data"),
            Command::Shell(vec!["ls".into(), "-l".into(), "--data".into()])
        );
        assert_eq!(p("shell --data /d ls").unwrap().data, Some("/d".into()));
        assert_eq!(command("shell -- -x"), Command::Shell(vec!["-x".into()]));
        assert!(p("shell -x").is_err());
    }

    #[test]
    fn logcat_options_are_forwarded() {
        assert_eq!(
            command("logs --follow -s ActivityManager"),
            Command::Logs {
                follow: true,
                args: vec!["-s".into(), "ActivityManager".into()]
            }
        );
        assert_eq!(
            command("logs"),
            Command::Logs {
                follow: false,
                args: vec![]
            }
        );
    }

    #[test]
    fn mistakes() {
        for args in [
            "",
            "frobnicate",
            "--verbose status",
            "stop --windows",
            "status now",
            "install",
            "uninstall",
            "open a b",
            "start -w",
        ] {
            assert!(p(args).is_err(), "{args:?}");
        }
    }
}
