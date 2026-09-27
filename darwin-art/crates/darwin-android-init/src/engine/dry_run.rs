//! An executor that only records what init would run.

use super::{Command, CommandExecutor, CommandFlow, Invocation};

/// One command as the engine ran it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutedCommand {
    /// Trigger string of the action (`Action::BuildTriggersString`).
    pub action: String,
    pub file: String,
    pub line: usize,
    /// The command as written (`Command::BuildCommandString`).
    pub command: String,
    /// Arguments after property expansion (empty for init's builtin
    /// actions or when expansion failed).
    pub expanded: Vec<String>,
    /// init's success/failure for the command.
    pub result: Result<(), String>,
}

/// Logs every command in init's log format and succeeds.
#[derive(Clone, Debug, Default)]
pub struct DryRunExecutor {
    pub log: Vec<String>,
    pub commands: Vec<Command>,
}

impl CommandExecutor for DryRunExecutor {
    fn execute(&mut self, invocation: &Invocation<'_>) -> Result<CommandFlow, String> {
        self.log.push(format!(
            "Command '{:?}' action={} ({}:{}) context={}",
            invocation.command,
            invocation.action,
            invocation.file,
            invocation.line,
            invocation.context
        ));
        self.commands.push(invocation.command.clone());
        Ok(CommandFlow::Done)
    }
}
