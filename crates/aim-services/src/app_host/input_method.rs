//! `input_method`: InputMethodManager for the app host, whose input
//! method is the Mac's (docs/input.md, "IME"; ADR 0013, "Services with a
//! macOS owner"). It takes clients as InputMethodManagerService does; an
//! editor's start (`startInputOrWindowGainedFocusAsync` with an
//! `EditorInfo` and an input connection) is told to the display server
//! (`aim_host_display::text_input`), whose window takes the keys of the
//! editor's task through the Mac's input method, and the marked and
//! committed text and editing commands it gives are made on the app's
//! input connection (`IRemoteInputConnection`), as an input method's
//! `InputConnection` makes them. No Android input method runs: each start
//! is answered with `InputBindResult.NO_IME`, so an app's invalidation of
//! its input falls back to a restart (`doInvalidateInput`), which starts
//! a new connection.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Parcel, Reader};
use aim_host_display::text_input::Message;
use aim_host_display::wire;
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::com_android_internal_inputmethod_iimetracker as tracker;
use aim_service_aidl::com_android_internal_view_iinputmethodmanager as imm;

use super::{AppHost, reply};

/// `IInputMethodClient.onStartInputResult`, `IRemoteInputConnection`'s
/// calls, in their interfaces' order.
const CLIENT_DESCRIPTOR: &str = "com.android.internal.inputmethod.IInputMethodClient";
const ON_START_INPUT_RESULT: u32 = 2;
const CONNECTION_DESCRIPTOR: &str = "com.android.internal.inputmethod.IRemoteInputConnection";
const DELETE_SURROUNDING_TEXT: u32 = 5;
const SET_COMPOSING_TEXT: u32 = 7;
const COMMIT_TEXT: u32 = 10;
const PERFORM_EDITOR_ACTION: u32 = 15;
const SEND_KEY_EVENT: u32 = 19;
/// `InputBindResult.ResultCode.ERROR_NO_IME`.
const ERROR_NO_IME: i32 = 6;
/// `KeyEvent.ACTION_DOWN`, `ACTION_UP`; `KeyCharacterMap.VIRTUAL_KEYBOARD`;
/// `KeyEvent.FLAG_SOFT_KEYBOARD | FLAG_KEEP_TOUCH_MODE`, as
/// `InputMethodService.sendDownUpKeyEvents` sends a key; `InputDevice
/// .SOURCE_KEYBOARD`.
const ACTION_DOWN: i32 = 0;
const ACTION_UP: i32 = 1;
const VIRTUAL_KEYBOARD: i32 = -1;
const SOFT_KEYBOARD_FLAGS: i32 = 0x2 | 0x4;
const SOURCE_KEYBOARD: i32 = 0x101;
/// A `flat_binder_object`'s size in a parcel, with its stability.
const BINDER_OBJECT: usize = 24;

/// The editor input goes to: its app's uid, its input connection, the
/// task the display server shows it in.
struct Editor {
    uid: u32,
    connection: Strong,
    task: i32,
}

pub struct InputMethodManager {
    host: Arc<AppHost>,
    clients: Mutex<Vec<Binder>>,
    tracker: std::sync::OnceLock<Binder>,
}

impl InputMethodManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        InputMethodManager {
            host,
            clients: Mutex::new(Vec::new()),
            tracker: std::sync::OnceLock::new(),
        }
    }

    /// `startInputOrWindowGainedFocusAsync`: the editor, if the call names
    /// one with a connection, takes the text input; the client is told
    /// there is no input method.
    fn start_input(&self, call: &mut Call<'_>) -> Result<(), i32> {
        let data = &mut call.data;
        data.enforce_interface(imm::DESCRIPTOR)
            .map_err(|_| BAD_VALUE)?;
        let _reason = data.read_i32()?;
        let client = data.read_binder()?;
        let _window = data.read_binder()?;
        let (_flags, _soft_input_mode, _window_flags) =
            (data.read_i32()?, data.read_i32()?, data.read_i32()?);
        let editor = match data.read_i32()? {
            0 => None,
            _ => Some((data.read_i32()?, data.read_i32()?)),
        };
        // The input connection, if any, is the third binder from the end:
        // the accessibility connection comes with it (`startInputInner`),
        // and the back dispatcher's result receiver always follows.
        let objects = objects_after(data);
        let connection = (objects.len() >= 3)
            .then(|| objects[objects.len() - 3])
            .map(|at| {
                let mut r = data.clone();
                r.set_position(at);
                r.read_binder()
            });
        // `startInputSeq` and `useAsyncShowHideMethod` end the call.
        let mut tail = data.clone();
        let end = tail.position() + tail.remaining();
        tail.set_position(end.checked_sub(8).ok_or(BAD_VALUE)?);
        let start_input_seq = tail.read_i32()?;
        let uid = call.sender_euid;
        match (editor, connection) {
            (Some((input_type, ime_options)), Some(Ok(Some(Binder::Handle(handle))))) => {
                self.host.start_editor(
                    uid,
                    self.host.process.strong(handle),
                    input_type,
                    ime_options,
                );
            }
            _ => self.host.end_editor(uid),
        }
        if let Some(Binder::Handle(client)) = client {
            let mut p = Parcel::new();
            p.write_interface_token(CLIENT_DESCRIPTOR);
            // `InputBindResult.NO_IME`.
            p.write_i32(1);
            p.write_i32(ERROR_NO_IME);
            p.write_binder(None);
            p.write_i32(-1); // no accessibility sessions
            p.write_i32(0); // no channel
            p.write_string16(None);
            p.write_i32(-1); // sequence
            p.write_bool(false);
            p.write_i32(start_input_seq);
            let _ = self
                .host
                .process
                .strong(client)
                .transact(ON_START_INPUT_RESULT, &p, true);
        }
        Ok(())
    }
}

/// The positions of the binder objects from the reader's on.
fn objects_after(r: &Reader<'_>) -> Vec<usize> {
    let mut r = r.clone();
    let mut found = Vec::new();
    while let Some(at) = r.next_object() {
        found.push(at);
        if at + BINDER_OBJECT > r.position() + r.remaining() {
            break;
        }
        r.set_position(at + BINDER_OBJECT);
    }
    found
}

impl InputMethodManager {
    /// `enforceCallingOrSelfPermission(INTERACT_ACROSS_USERS_FULL)`.
    fn enforce_across_users(&self, uid: i32) -> Result<(), aim_binder_host::parcel::Exception> {
        const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
        if self
            .host
            .check_permission(Some(INTERACT_ACROSS_USERS_FULL), uid)?
            != 0
        {
            return Err(aim_binder_host::parcel::Exception::security(format!(
                "Neither user {uid} nor current process has {INTERACT_ACROSS_USERS_FULL}."
            )));
        }
        Ok(())
    }
}

impl Service for InputMethodManager {
    fn descriptor(&self) -> &str {
        imm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            imm::ADD_CLIENT => {
                let args = imm::AddClient::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(client) = args.client {
                    self.clients.lock().unwrap().push(client);
                }
                let mut p = Parcel::new();
                imm::write_add_client_reply(&mut p);
                Ok(p)
            }
            // Oneway: its outcome goes to the client.
            imm::START_INPUT_OR_WINDOW_GAINED_FOCUS_ASYNC => {
                self.start_input(call)?;
                Ok(Parcel::new())
            }
            // The input method has no surface of its own.
            imm::REMOVE_IME_SURFACE_FROM_WINDOW_ASYNC => Ok(Parcel::new()),
            // No Android input method shows or hides: the Mac's input
            // method takes an editor's keys whenever it has the focus.
            imm::SHOW_SOFT_INPUT | imm::HIDE_SOFT_INPUT => {
                call.data
                    .enforce_interface(imm::DESCRIPTOR)
                    .map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                imm::write_show_soft_input_reply(&mut p, false);
                Ok(p)
            }
            // The device has no Android input method: to Android it is
            // one with a physical keyboard, the Mac's input method behind
            // it (the owner's choice, 2026-10-19). The lists are empty,
            // asked across users with INTERACT_ACROSS_USERS_FULL.
            imm::GET_INPUT_METHOD_LIST
            | imm::GET_ENABLED_INPUT_METHOD_LIST
            | imm::GET_INPUT_METHOD_LIST_LEGACY
            | imm::GET_ENABLED_INPUT_METHOD_LIST_LEGACY => {
                let r = &mut call.data;
                r.enforce_interface(imm::DESCRIPTOR)
                    .map_err(|_| BAD_VALUE)?;
                let user = r.read_i32().map_err(|_| BAD_VALUE)?;
                if user != (call.sender_euid / super::PER_USER_RANGE) as i32
                    && let Err(e) = self.enforce_across_users(call.sender_euid as i32)
                {
                    return reply(Err(e));
                }
                let mut p = Parcel::new();
                p.write_no_exception();
                if matches!(
                    call.code,
                    imm::GET_INPUT_METHOD_LIST | imm::GET_ENABLED_INPUT_METHOD_LIST
                ) {
                    // `InputMethodInfoSafeList.empty()`: a null blob.
                    p.write_i32(1);
                    p.write_i32(-1);
                } else {
                    p.write_i32(0);
                }
                Ok(p)
            }
            imm::GET_IME_TRACKER_SERVICE => {
                call.data
                    .enforce_interface(imm::DESCRIPTOR)
                    .map_err(|_| BAD_VALUE)?;
                let tracker = *self.tracker.get_or_init(|| {
                    self.host
                        .process
                        .add_service(Arc::new(ImeTracker(self.host.clone())))
                });
                let mut p = Parcel::new();
                imm::write_get_ime_tracker_service_reply(&mut p, Some(tracker));
                Ok(p)
            }
            code => reply(Err(self.host.unimplemented(
                "input_method",
                self.descriptor(),
                code,
            ))),
        }
    }
}

/// `ImeTrackerService`: a request to show or hide the input method gets
/// a token; its progress is reported to no history (no `dumpsys`).
struct ImeTracker(Arc<AppHost>);

/// `ImeTracker.Token`: its binder and tag.
struct Token {
    binder: Binder,
    tag: Option<String>,
}

impl WriteParcelable for Token {
    fn write_to(&self, p: &mut Parcel) {
        p.write_binder(Some(self.binder));
        p.write_string8(self.tag.as_deref());
    }
}

impl Service for ImeTracker {
    fn descriptor(&self) -> &str {
        tracker::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            tracker::ON_START => {
                let args = tracker::OnStart::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let token = Token {
                    binder: self.0.process.add_service(Arc::new(super::activity::Token)),
                    tag: args.tag,
                };
                let mut p = Parcel::new();
                tracker::write_on_start_reply(&mut p, Some(&token));
                Ok(p)
            }
            // Oneway reports.
            tracker::ON_PROGRESS
            | tracker::ON_FAILED
            | tracker::ON_CANCELLED
            | tracker::ON_SHOWN
            | tracker::ON_HIDDEN
            | tracker::ON_DISPATCHED => Ok(Parcel::new()),
            code => reply(Err(self.0.unimplemented(
                "ime tracker",
                self.descriptor(),
                code,
            ))),
        }
    }
}

/// The text input's state: the display server's connection and the
/// editor.
#[derive(Default)]
pub struct TextInput {
    link: Mutex<Option<UnixStream>>,
    editor: Mutex<Option<Editor>>,
}

impl AppHost {
    /// Connects to the display server's text input at `socket`, in a
    /// thread of its own, and makes what its windows' input method gives
    /// on the editor.
    pub fn connect_text_input(self: &Arc<Self>, socket: &Path) {
        let host = self.clone();
        let socket = socket.to_path_buf();
        std::thread::spawn(move || {
            let link = match text_input_link(&socket) {
                Ok(link) => link,
                Err(e) => return eprintln!("app host: text input: {e}"),
            };
            let Ok(writer) = link.try_clone() else { return };
            *host.text_input.link.lock().unwrap() = Some(writer);
            let mut reader = link;
            while let Ok(Some(m)) = Message::read(&mut reader) {
                if let Err(e) = host.type_text(&m) {
                    eprintln!("app host: text input: {e}");
                }
            }
            *host.text_input.link.lock().unwrap() = None;
        });
    }

    /// `uid`'s editor on `connection` takes the text input, in its task.
    fn start_editor(&self, uid: u32, connection: Strong, input_type: i32, ime_options: i32) {
        let task = self
            .tasks
            .of_uid(uid)
            .filter(|_| self.tasks.window_mode())
            .unwrap_or(-1);
        *self.text_input.editor.lock().unwrap() = Some(Editor {
            uid,
            connection,
            task,
        });
        self.tell_display(&Message::Editor {
            task,
            active: true,
            input_type,
            ime_options,
        });
    }

    /// `uid` has no editor (any more).
    fn end_editor(&self, uid: u32) {
        let mut editor = self.text_input.editor.lock().unwrap();
        if let Some(e) = editor.take_if(|e| e.uid == uid) {
            drop(editor);
            self.tell_display(&Message::Editor {
                task: e.task,
                active: false,
                input_type: 0,
                ime_options: 0,
            });
        }
    }

    fn tell_display(&self, m: &Message) {
        let mut link = self.text_input.link.lock().unwrap();
        if let Some(l) = link.as_mut()
            && let Err(e) = l.write_all(&m.frame())
        {
            eprintln!("app host: text input: {e}");
        }
    }

    /// What the input method gave, made on the editor's connection as the
    /// input method's `InputConnection` calls (oneway, in the current
    /// session: 0, a new connection's).
    fn type_text(&self, m: &Message) -> Result<(), String> {
        let editor = self.text_input.editor.lock().unwrap();
        let Some(editor) = editor.as_ref() else {
            return Ok(());
        };
        let call = |code: u32, write: &dyn Fn(&mut Parcel)| -> Result<(), String> {
            let mut p = Parcel::new();
            p.write_interface_token(CONNECTION_DESCRIPTOR);
            // `InputConnectionCommandHeader`: the session.
            p.write_i32(1);
            p.write_i32(0);
            write(&mut p);
            editor
                .connection
                .transact(code, &p, true)
                .map(|_| ())
                .map_err(|s| format!("input connection #{code}: {s}"))
        };
        // A `CharSequence` (`TextUtils.writeToParcel`): present, plain,
        // its UTF-8.
        let text = |p: &mut Parcel, s: &str| {
            p.write_i32(1);
            p.write_i32(1);
            p.write_string8(Some(s));
        };
        match m {
            Message::Compose { text: t, cursor } => call(SET_COMPOSING_TEXT, &|p| {
                text(p, t);
                p.write_i32(*cursor);
            }),
            Message::Commit { text: t } => call(COMMIT_TEXT, &|p| {
                text(p, t);
                p.write_i32(1);
            }),
            Message::Delete { before, after } => call(DELETE_SURROUNDING_TEXT, &|p| {
                p.write_i32(*before);
                p.write_i32(*after);
            }),
            Message::Action { code } => call(PERFORM_EDITOR_ACTION, &|p| p.write_i32(*code)),
            Message::Key { code } => {
                let now = super::clock_ms(libc::CLOCK_UPTIME_RAW);
                for action in [ACTION_DOWN, ACTION_UP] {
                    call(SEND_KEY_EVENT, &|p| {
                        p.write_i32(1);
                        write_key_event(p, now, action, *code);
                    })?;
                }
                Ok(())
            }
            Message::Editor { .. } => Ok(()),
        }
    }
}

/// `KeyEvent.writeToParcel` of a soft keyboard's key (`InputMethodService
/// .sendDownUpKeyEvents`): pressed at `time` (uptime ms).
fn write_key_event(p: &mut Parcel, time: i64, action: i32, code: i32) {
    p.write_i32(2); // `PARCEL_TOKEN_KEY_EVENT`
    p.write_i32(0); // id
    p.write_i32(VIRTUAL_KEYBOARD);
    p.write_i32(SOURCE_KEYBOARD);
    p.write_i32(-1); // display: `INVALID_DISPLAY`
    p.write_i32(-1); // no HMAC
    p.write_i32(action);
    p.write_i32(code);
    p.write_i32(0); // repeat
    p.write_i32(0); // meta state
    p.write_i32(0); // scan code
    p.write_i32(SOFT_KEYBOARD_FLAGS);
    p.write_i64(time * 1_000_000); // down time, ns
    p.write_i64(time * 1_000_000); // event time, ns
    p.write_string16(None); // no characters
}

/// A connection to the text input of the display server at `socket`.
fn text_input_link(socket: &Path) -> Result<UnixStream, String> {
    let link = UnixStream::connect(socket).map_err(|e| format!("{}: {e}", socket.display()))?;
    let hello = wire::Request {
        op: wire::OP_TEXT_INPUT,
        id: wire::VERSION,
        ..Default::default()
    };
    wire::send(link.as_fd(), wire::bytes(&hello), None).map_err(|e| e.to_string())?;
    let mode = wire::recv_record::<u32>(link.as_fd())
        .map_err(|e| e.to_string())?
        .ok_or("the display server closed the connection")?;
    if mode == aim_hostcall::display::mode::HEADLESS {
        return Err("headless display has no native UI surface".into());
    }
    Ok(link)
}
