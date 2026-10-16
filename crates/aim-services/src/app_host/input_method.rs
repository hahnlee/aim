//! `input_method`: InputMethodManager for the app host, which has no input
//! method yet: clients are taken, and their starts are not answered
//! (#1432).

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Parcel};
use aim_service_aidl::com_android_internal_view_iinputmethodmanager as imm;

use super::{AppHost, reply};

pub struct InputMethodManager {
    host: Arc<AppHost>,
    clients: Mutex<Vec<Binder>>,
}

impl InputMethodManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        InputMethodManager { host, clients: Mutex::new(Vec::new()) }
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
            imm::START_INPUT_OR_WINDOW_GAINED_FOCUS_ASYNC | imm::REMOVE_IME_SURFACE_FROM_WINDOW_ASYNC => {
                let mut p = Parcel::new();
                p.write_no_exception();
                Ok(p)
            }
            code => reply(Err(self.host.unimplemented("input_method", code))),
        }
    }
}
