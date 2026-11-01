//! Native routing to IMS's Java input-filter adapter (#2529), Android 16.
use super::*;
use aim_framework_aidl::dev_aim_server_iinputfiltercallback as api;
use binder::SpIBinder;
use binder::binder_impl::Proxy;

trait Callback: Interface {}
declare_binder_interface! {
    Callback[api::DESCRIPTOR] {
        native: BnCallback(unused),
        proxy: BpCallback,
    }
}
impl Callback for BpCallback {}
impl Callback for binder::binder_impl::Binder<BnCallback> {}
fn unused(
    _: &dyn Callback,
    _: u32,
    _: &BorrowedParcel<'_>,
    _: &mut BorrowedParcel<'_>,
) -> Result<(), StatusCode> {
    Err(StatusCode::UNKNOWN_TRANSACTION)
}
fn dispatcher(agent: &dyn IAppHostAgent) -> Result<Arc<input::Dispatcher>, StatusCode> {
    if !matches!(binder::ThreadState::get_calling_uid(), 0 | 1000) {
        return Err(StatusCode::PERMISSION_DENIED);
    }
    agent.dispatcher().ok_or(StatusCode::INVALID_OPERATION)
}
pub(super) fn set(
    agent: &dyn IAppHostAgent,
    data: &BorrowedParcel<'_>,
    reply: &mut BorrowedParcel<'_>,
) -> Result<(), StatusCode> {
    let dispatcher = dispatcher(agent)?;
    let remote: Option<SpIBinder> = data.read()?;
    let filter: Option<input::InputFilter> = if let Some(mut remote) = remote {
        if !remote.associate_class(BnCallback::get_class()) {
            return Err(StatusCode::BAD_TYPE);
        }
        let proxy = BpCallback::from_binder(remote)?;
        Some(Arc::new(move |bytes, flags| {
            let result = (|| {
                let remote = proxy.as_binder();
                let mut p = remote.prepare_transact()?;
                p.write(bytes)?;
                p.write(&flags)?;
                remote.submit_transact(
                    api::FILTER_INPUT_EVENT,
                    p,
                    binder::binder_impl::FLAG_ONEWAY,
                )?;
                Ok::<_, StatusCode>(())
            })();
            if let Err(e) = result {
                log::error!("input filter delivery: {e}");
            }
        }))
    } else {
        None
    };
    dispatcher.set_input_filter(filter);
    reply.write(&Status::ok())
}
pub(super) fn resend(
    agent: &dyn IAppHostAgent,
    data: &BorrowedParcel<'_>,
    reply: &mut BorrowedParcel<'_>,
) -> Result<(), StatusCode> {
    let dispatcher = dispatcher(agent)?;
    let bytes: Vec<u8> = data.read()?;
    let flags: i32 = data.read()?;
    let (mut event, used) =
        aim_input_transport::injection::Event::decode(&bytes).ok_or(StatusCode::BAD_VALUE)?;
    if used != bytes.len() || !event.valid {
        return Err(StatusCode::BAD_VALUE);
    }
    event.filtered(flags);
    if flags & 0x40000000 != 0 {
        dispatcher.inject_event(event, 0, -1, std::time::Duration::ZERO);
    }
    reply.write(&Status::ok())
}
