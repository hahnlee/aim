//! Text input in the display server (docs/input.md, "IME"): the app host
//! says which task has an editor; the window showing the task, a window
//! host's (window mode) or this process's, takes its keys through the
//! Mac's input method (`crate::input`), and what the input method gives
//! goes back to the app host.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::Mutex;

use aim_host_display::text_input::Message;

use crate::objc::on_main;

struct State {
    /// The app host's connection, to write to.
    link: Option<UnixStream>,
    /// The task with the editor, as last said, and whether it went to a
    /// window host.
    editor: Option<(Message, bool)>,
}

static STATE: Mutex<State> = Mutex::new(State { link: None, editor: None });

/// Serve the app host's connection until it closes; in window mode
/// (`windows`) the window host showing the editor's task takes its keys.
pub fn serve_bridge(sock: UnixStream, windows: bool) {
    let Ok(writer) = sock.try_clone() else { return };
    STATE.lock().unwrap().link = Some(writer);
    let mut reader = sock;
    while let Ok(Some(m)) = Message::read(&mut reader) {
        if let Message::Editor { task, .. } = m {
            route(m, windows.then(|| crate::hosts::showing(task)).flatten());
        }
    }
    let mut s = STATE.lock().unwrap();
    s.link = None;
    if let Some((Message::Editor { task, .. }, _)) = s.editor.take() {
        drop(s);
        let none = Message::Editor { task, active: false, input_type: 0, ime_options: 0 };
        if let Some(h) = crate::hosts::showing(task) {
            h.send_text_input(&none);
        }
        on_main(move || crate::input::editor(&none));
    }
}

/// The editor goes to the host showing its task, else to this process's
/// windows; an editor's end goes where it had gone.
fn route(m: Message, host: Option<std::sync::Arc<crate::hosts::Host>>) {
    let mut s = STATE.lock().unwrap();
    if let Some((previous, hosted)) = s.editor.take()
        && let Message::Editor { task, .. } = previous
    {
        let end = Message::Editor { task, active: false, input_type: 0, ime_options: 0 };
        match crate::hosts::showing(task).filter(|_| hosted) {
            Some(h) => h.send_text_input(&end),
            None => on_main(move || crate::input::editor(&end)),
        }
    }
    let Message::Editor { active, .. } = m else { return };
    if active {
        s.editor = Some((m.clone(), host.is_some()));
    }
    match host {
        Some(h) => h.send_text_input(&m),
        None => on_main(move || crate::input::editor(&m)),
    }
}

/// What the input method gave, from a window host or this process: to
/// the app host.
pub fn to_app_host(m: &Message) {
    if matches!(m, Message::Editor { .. }) {
        return;
    }
    let mut s = STATE.lock().unwrap();
    if let Some(link) = s.link.as_mut()
        && let Err(e) = link.write_all(&m.frame())
    {
        eprintln!("aim-display: text input: {e}");
    }
}
