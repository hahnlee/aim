//! `Parser::ParseData` and the section parsers' `EndSection`/`EndFile`.

use super::ParseEnv;
use super::action::{Action, parse_triggers};
use super::builtins::CommandSpec;
use super::expand::expand_props;
use super::service::{self, OptionOutcome, Service};
use super::tokenizer::{Token, Tokenizer};
use crate::diag::Diagnostic;
use crate::image::ImageRoot;

/// Which section keywords a parser accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    /// `CreateParser`: `service`, `on` and `import`.
    Boot,
    /// `CreateApexConfigParser`: `service` and `on` only.
    Apex,
}

/// The result of parsing a set of scripts.
#[derive(Clone, Debug, Default)]
pub struct ParsedScripts {
    /// init's `ServiceList`, in definition order (an `override` moves the
    /// service to the end, as `RemoveService` + `AddService` do).
    pub services: Vec<Service>,
    /// init's `ActionManager::actions_`, in definition order. Actions with
    /// no commands are dropped, as init does.
    pub actions: Vec<Action>,
    /// Guest paths of the files parsed, in order.
    pub files: Vec<String>,
    /// What init would log while parsing.
    pub diagnostics: Vec<Diagnostic>,
    /// init's `parse_error_count_`.
    pub parse_error_count: usize,
}

impl ParsedScripts {
    pub fn service(&self, name: &str) -> Option<&Service> {
        self.services.iter().find(|service| service.name == name)
    }

    pub fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == crate::Severity::Warning)
            .count()
    }

    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == crate::Severity::Error)
            .count()
    }
}

enum Section {
    Service(Box<Service>),
    Action(Action),
    Import,
}

/// A stateful parser: files parsed with it share one service list and one
/// action list, like init's parser singletons.
pub struct Parser<'a> {
    env: ParseEnv<'a>,
    kind: SectionKind,
    image: Option<&'a ImageRoot>,
    /// Paths that are vendor subcontext (`/vendor`, `/odm`, vendor APEXes).
    vendor_apexes: Vec<String>,
    scripts: ParsedScripts,
    /// `ImportParser::imports_`.
    pending_imports: Vec<(String, usize)>,
}

impl<'a> Parser<'a> {
    pub fn new(env: ParseEnv<'a>, kind: SectionKind) -> Self {
        Self::continue_from(env, kind, ParsedScripts::default())
    }

    /// Continues with an existing service/action state (the APEX parser
    /// adds to the same `ServiceList` and `ActionManager` as the boot one).
    pub fn continue_from(env: ParseEnv<'a>, kind: SectionKind, scripts: ParsedScripts) -> Self {
        Self {
            env,
            kind,
            image: None,
            vendor_apexes: Vec::new(),
            scripts,
            pending_imports: Vec::new(),
        }
    }

    /// Where `import` and directory parsing read from.
    pub fn with_image(mut self, image: &'a ImageRoot) -> Self {
        self.image = Some(image);
        self
    }

    /// `Subcontext::SetApexList`: APEXes from the vendor or odm partition.
    pub fn with_vendor_apexes(mut self, apexes: Vec<String>) -> Self {
        self.vendor_apexes = apexes;
        self
    }

    pub fn finish(self) -> ParsedScripts {
        self.scripts
    }

    pub fn scripts(&self) -> &ParsedScripts {
        &self.scripts
    }

    /// `Subcontext::PathMatchesSubcontext` for init's vendor subcontext.
    /// init only creates the subcontext for vendor images P or newer.
    fn path_in_vendor_subcontext(&self, path: &str) -> bool {
        if self.env.vendor_api_level < super::ANDROID_API_P {
            return false;
        }
        if let Some(apex) = apex_name_from_file_name(path) {
            return self.vendor_apexes.iter().any(|name| name == apex);
        }
        path.starts_with("/vendor") || path.starts_with("/odm")
    }

    fn error(&mut self, file: &str, line: usize, message: impl Into<String>) {
        self.scripts.parse_error_count += 1;
        self.scripts
            .diagnostics
            .push(Diagnostic::error(file, line, message));
    }

    fn log_error(&mut self, file: &str, line: usize, message: impl Into<String>) {
        self.scripts
            .diagnostics
            .push(Diagnostic::error(file, line, message));
    }

    fn warning(&mut self, file: &str, line: usize, message: impl Into<String>) {
        self.scripts
            .diagnostics
            .push(Diagnostic::warning(file, line, message));
    }

    fn is_section_keyword(&self, keyword: &str) -> bool {
        match keyword {
            "service" | "on" => true,
            "import" => self.kind == SectionKind::Boot,
            _ => false,
        }
    }

    /// `Parser::ParseData`.
    pub fn parse_data(&mut self, filename: &str, contents: &[u8]) {
        self.scripts.files.push(filename.to_string());
        let mut tokenizer = Tokenizer::new(contents);
        let mut section: Option<Section> = None;
        let mut section_start_line = 0usize;
        let mut bad_section_found = false;
        let mut args: Vec<String> = Vec::new();

        loop {
            match tokenizer.next_token() {
                Token::Eof => {
                    self.end_section(filename, section_start_line, section.take());
                    self.end_file();
                    return;
                }
                Token::Newline => {
                    tokenizer.line += 1;
                    let line = tokenizer.line;
                    if args.is_empty() {
                        continue;
                    }
                    let line_args = std::mem::take(&mut args);
                    if self.is_section_keyword(&line_args[0]) {
                        self.end_section(filename, section_start_line, section.take());
                        bad_section_found = false;
                        section_start_line = line;
                        match self.begin_section(line_args, filename, line) {
                            Ok(new_section) => section = Some(new_section),
                            Err(error) => {
                                self.error(filename, line, error);
                                bad_section_found = true;
                            }
                        }
                    } else if let Some(current) = section.as_mut() {
                        if let Err(error) = self.parse_line(current, line_args, line) {
                            self.error(filename, line, error);
                        }
                    } else if !bad_section_found {
                        self.error(filename, line, "Invalid section keyword found");
                    }
                }
                Token::Text(text) => args.push(text),
            }
        }
    }

    fn begin_section(
        &mut self,
        args: Vec<String>,
        filename: &str,
        line: usize,
    ) -> Result<Section, String> {
        match args[0].as_str() {
            "service" => {
                let vendor = self.path_in_vendor_subcontext(filename);
                service::begin_service(&args, filename, line, vendor, &self.env)
                    .map(|service| Section::Service(Box::new(service)))
            }
            "on" => {
                let triggers = &args[1..];
                if triggers.is_empty() {
                    return Err("Actions must have a trigger".to_string());
                }
                let vendor = self.path_in_vendor_subcontext(filename);
                if filename.starts_with("/apex/") && !vendor {
                    return Err(
                        "ParseSection() failed: 'on' is supported for only Vendor APEXes."
                            .to_string(),
                    );
                }
                let (event_trigger, property_triggers) =
                    parse_triggers(triggers, self.env.vendor_api_level)
                        .map_err(|error| format!("ParseTriggers() failed: {error}"))?;
                Ok(Section::Action(Action {
                    event_trigger,
                    property_triggers,
                    commands: Vec::new(),
                    filename: filename.to_string(),
                    line,
                    vendor_subcontext: vendor,
                }))
            }
            _ => {
                // import
                if args.len() != 2 {
                    return Err("single argument needed for import\n".to_string());
                }
                let path = expand_props(&args[1], self.env.properties, self.env.vendor_api_level)
                    .map_err(|error| format!("Could not expand import: {error}"))?;
                self.pending_imports.push((path, line));
                Ok(Section::Import)
            }
        }
    }

    fn parse_line(
        &mut self,
        section: &mut Section,
        args: Vec<String>,
        line: usize,
    ) -> Result<(), String> {
        match section {
            Section::Service(service) => {
                let filename = service.filename.clone();
                match service::parse_option(service, args, line, &self.scripts.services, &self.env)
                {
                    OptionOutcome::Ok => Ok(()),
                    OptionOutcome::Warning(warning) => {
                        self.warning(&filename, line, warning);
                        Ok(())
                    }
                    OptionOutcome::Error(error) => Err(error),
                }
            }
            Section::Action(action) => {
                let command = CommandSpec::parse(args, line)?;
                action.commands.push(command);
                Ok(())
            }
            Section::Import => Err("Unexpected line found after import statement".to_string()),
        }
    }

    fn end_section(&mut self, filename: &str, start_line: usize, section: Option<Section>) {
        match section {
            None | Some(Section::Import) => {}
            Some(Section::Action(action)) => {
                if !action.commands.is_empty() {
                    self.scripts.actions.push(action);
                }
            }
            Some(Section::Service(service)) => {
                if let Err(error) = self.end_service(*service, filename, start_line) {
                    self.error(filename, start_line, error);
                }
            }
        }
    }

    /// `ServiceParser::EndSection`.
    fn end_service(&mut self, service: Service, filename: &str, line: usize) -> Result<(), String> {
        if let Some(warning) = service::end_service_checks(&service, &self.env)? {
            self.warning(filename, line, warning)
        }
        if let Some(index) = self
            .scripts
            .services
            .iter()
            .position(|old| old.name == service.name)
        {
            if !service.is_override {
                return Err(format!(
                    "ignored duplicate definition of service '{}'",
                    service.name
                ));
            }
            let old = &self.scripts.services[index];
            if filename.starts_with("/apex/") && !old.updatable {
                return Err(format!(
                    "cannot update a non-updatable service '{}' with a config in APEX",
                    service.name
                ));
            }
            if old.vendor_subcontext != service.vendor_subcontext {
                return Err(format!(
                    "service '{}' overrides another service across the treble boundary.",
                    service.name
                ));
            }
            self.scripts.services.remove(index);
        }
        self.scripts.services.push(service);
        Ok(())
    }

    /// `ImportParser::EndFile`: parse this file's imports, depth first.
    fn end_file(&mut self) {
        let imports = std::mem::take(&mut self.pending_imports);
        for (path, _line) in imports {
            self.parse_config(&path);
        }
    }

    /// `Parser::ParseConfig`: a directory's regular files in sorted order,
    /// or a single file. Returns false when nothing could be read.
    pub fn parse_config(&mut self, path: &str) -> bool {
        let Some(image) = self.image else {
            self.log_error(path, 0, "no image to read scripts from");
            return false;
        };
        if image.is_dir(path) {
            return self.parse_config_dir(path);
        }
        match image.read(path) {
            Ok(contents) => {
                self.parse_data(path, &contents);
                true
            }
            Err(error) => {
                // init logs this at INFO level.
                self.warning(
                    path,
                    0,
                    format!("Unable to read config file '{path}': {error}"),
                );
                false
            }
        }
    }

    /// `Parser::ParseConfigDir`.
    fn parse_config_dir(&mut self, path: &str) -> bool {
        let Some(image) = self.image else {
            return false;
        };
        let mut files = match image.regular_files(path) {
            Ok(files) => files,
            Err(error) => {
                self.warning(
                    path,
                    0,
                    format!("Could not import directory '{path}': {error}"),
                );
                return false;
            }
        };
        // Sort first so we load files in a consistent order (bug 31996208).
        files.sort();
        for file in files {
            match image.read(&file) {
                Ok(contents) => self.parse_data(&file, &contents),
                Err(error) => self.log_error(
                    &file,
                    0,
                    format!("could not import file '{file}': Unable to read config file '{file}': {error}"),
                ),
            }
        }
        true
    }

    /// `Parser::ParseConfigFile` (used for APEX scripts).
    pub fn parse_config_file(&mut self, path: &str) -> Result<(), String> {
        let Some(image) = self.image else {
            return Err("no image to read scripts from".to_string());
        };
        let contents = image
            .read(path)
            .map_err(|error| format!("Unable to read config file '{path}': {error}"))?;
        self.parse_data(path, &contents);
        Ok(())
    }

    pub(crate) fn push_diagnostic(&mut self, diagnostic: Diagnostic) {
        self.scripts.diagnostics.push(diagnostic);
    }
}

/// `GetApexNameFromFileName`.
pub(crate) fn apex_name_from_file_name(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/apex/")?;
    Some(rest.split('/').next().unwrap_or(rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rc::{IdResolver, SocketType};
    use std::collections::BTreeMap;

    fn parse(text: &str, props: &BTreeMap<String, String>) -> ParsedScripts {
        let ids = IdResolver::builtin();
        let env = ParseEnv {
            properties: props,
            vendor_api_level: 36,
            ids: &ids,
        };
        let mut parser = Parser::new(env, SectionKind::Boot);
        parser.parse_data("/system/etc/init/test.rc", text.as_bytes());
        parser.finish()
    }

    #[test]
    fn parses_services_and_actions() {
        let props = BTreeMap::new();
        let scripts = parse(
            r#"
on early-init && property:ro.debuggable=1
    setprop a.b ${ro.x:-y}
    trigger late

on property:sys.boot_completed=1
    # a comment
    start foo

on empty

service foo /system/bin/foo --flag "two words"
    class core main
    user system
    group system inet readproc
    capabilities NET_ADMIN SYS_NICE
    socket foo stream+passcred 0660 system system u:object_r:foo_socket:s0
    file /dev/kmsg w
    onrestart restart bar
    oneshot
    disabled
    seclabel u:r:foo:s0
    writepid /dev/cpuset/foreground/tasks /dev/foo
    task_profiles HighPerformance
    interface aidl foo.IFoo/default
    ioprio rt 4
    priority -10
    rlimit nofile 1024 unlimited
    setenv HOME /data
    namespace mnt pid
    updatable
    restart_period 10
    memcg.swappiness 10
    bogus_option 1
"#,
            &props,
        );
        assert_eq!(scripts.actions.len(), 2, "{:?}", scripts.diagnostics);
        let first = &scripts.actions[0];
        assert_eq!(first.event_trigger, "early-init");
        assert_eq!(first.property_triggers["ro.debuggable"], "1");
        assert_eq!(first.commands.len(), 2);
        assert_eq!(first.commands[0].args, vec!["setprop", "a.b", "${ro.x:-y}"]);
        assert_eq!(first.triggers_string(), "ro.debuggable=1 && early-init");

        let foo = scripts.service("foo").unwrap();
        assert_eq!(foo.args, vec!["/system/bin/foo", "--flag", "two words"]);
        assert_eq!(
            foo.classnames.iter().collect::<Vec<_>>(),
            vec!["core", "main"]
        );
        assert_eq!(foo.uid(), 1000);
        assert_eq!(foo.supplementary_groups.len(), 2);
        assert_eq!(foo.supplementary_groups[0].id, 3003);
        assert_eq!(foo.capabilities, Some((1 << 12) | (1 << 23)));
        assert_eq!(foo.sockets[0].socket_type, SocketType::Stream);
        assert!(foo.sockets[0].passcred);
        assert_eq!(foo.sockets[0].perm, 0o660);
        assert_eq!(foo.sockets[0].context, "u:object_r:foo_socket:s0");
        assert_eq!(foo.files[0].name, "/dev/kmsg");
        assert_eq!(foo.onrestart[0].args, vec!["restart", "bar"]);
        assert!(foo.oneshot && foo.disabled && foo.updatable);
        assert_eq!(foo.writepid_files, vec!["/dev/foo"]);
        assert_eq!(
            foo.task_profiles,
            vec!["ProcessCapacityHigh", "HighPerformance"]
        );
        assert!(foo.interfaces.contains("aidl/foo.IFoo/default"));
        assert_eq!(foo.ioprio_priority, 4);
        assert_eq!(foo.priority, -10);
        assert_eq!(foo.rlimits[0].resource, 7);
        assert_eq!(foo.rlimits[0].hard, u64::MAX);
        assert_eq!(
            foo.environment[0],
            ("HOME".to_string(), "/data".to_string())
        );
        assert!(foo.namespaces.new_pid && foo.namespaces.new_mount);
        assert_eq!(foo.restart_period, 10);
        assert_eq!(foo.other_options.len(), 2);
        assert_eq!(scripts.parse_error_count, 1, "{:?}", scripts.diagnostics);
    }

    #[test]
    fn duplicate_and_override_services() {
        let props = BTreeMap::new();
        let scripts = parse(
            "service a /a\n    user root\nservice a /b\n    user root\nservice a /c\n    override\n    user root\n",
            &props,
        );
        assert_eq!(scripts.services.len(), 1);
        assert_eq!(scripts.services[0].args[0], "/c");
        assert_eq!(scripts.parse_error_count, 1);
    }

    #[test]
    fn bad_sections_suppress_following_lines() {
        let props = BTreeMap::new();
        let scripts = parse(
            "service x\n    user root\n    class main\nstray line\n",
            &props,
        );
        assert!(scripts.services.is_empty());
        // Only the bad section header is an error; its lines are skipped.
        assert_eq!(scripts.parse_error_count, 1);
    }

    #[test]
    fn line_numbers_count_continuations() {
        let props = BTreeMap::new();
        let scripts = parse("\n\non boot\n    write /a \\\n  b\n    start x\n", &props);
        let action = &scripts.actions[0];
        assert_eq!(action.line, 3);
        assert_eq!(action.commands[0].line, 5);
        assert_eq!(action.commands[1].line, 6);
    }
}
