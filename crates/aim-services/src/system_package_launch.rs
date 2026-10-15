//! ActivityManager owns launch IntentSenders; only original Intent serialization
//! stays in the Java leaf. AM sees the real inbound Binder UID/PID.
use super::*;
impl System {
    pub(crate) fn package_launch_owner(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<Arc<crate::package::launch::Owner>> {
        self.check_package_bootstrap(bridge)?;
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bootstrap;
        let mut request = Parcel::new();
        bootstrap::GetPackageLaunchBridge {}.write(&mut request);
        let reply = bridge
            .owner
            .transact(bootstrap::GET_PACKAGE_LAUNCH_BRIDGE, &request, false)
            .map_err(|status| unreachable_service("package launch bridge", status))?;
        let mut reader = reply.reader();
        let binder = bootstrap::read_get_package_launch_bridge_reply(&mut reader)
            .map_err(|status| unreachable_service("package launch bridge", status))??;
        if reader.remaining() != 0 {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "launch bridge reply tail",
            ));
        }
        let node = reply
            .retain_remote_binder(binder.ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "launch bridge unavailable",
                )
            })?)
            .map_err(|status| unreachable_service("package launch bridge", status))?;
        let node = Arc::new(node);
        let retained = bridge.clone();
        let system = Arc::downgrade(self);
        Ok(Arc::new(crate::package::launch::Owner::new(Box::new(
            move |request| {
                let system = system.upgrade().ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "launch owner stopped",
                    )
                })?;
                system.check_package_bootstrap(&retained)?;
                if system
                    .process
                    .authenticated_inbound_identity()
                    .is_none_or(|(_, uid)| uid as i32 != request.caller_uid)
                {
                    return Err(Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "launch creator caller lacks matching authenticated Binder provenance",
                    ));
                }
                use aim_service_aidl::dev_aim_server_ipackagelaunchbridge as intent;
                let mut payload = Parcel::new();
                intent::BuildLaunchIntent {
                    category: Some(request.category),
                    target_package: request.target_package,
                    component: request.component.map(LaunchComponent),
                }
                .write(&mut payload);
                let serialized = node
                    .transact(intent::BUILD_LAUNCH_INTENT, &payload, false)
                    .map_err(|status| unreachable_service("launch intent serializer", status))?;
                let mut reader = serialized.reader();
                let bytes = intent::read_build_launch_intent_reply(&mut reader)
                    .map_err(|status| unreachable_service("launch intent serializer", status))??
                    .ok_or_else(|| {
                        Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            "null launch Intent",
                        )
                    })?;
                if reader.remaining() != 0 {
                    return Err(Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "launch Intent reply tail",
                    ));
                }
                let mut data = Parcel::new();
                data.write_interface_token(am::DESCRIPTOR);
                data.write_i32(2); // INTENT_SENDER_ACTIVITY
                data.write_string16(request.caller_package.as_deref());
                data.write_string16(request.feature.as_deref());
                data.write_binder(None);
                data.write_string16(None);
                data.write_i32(1);
                data.write_i32(1);
                data.write_i32(1);
                data.write_raw(&bytes, &[]); // one typed Intent
                data.write_i32(-1); // null resolvedTypes: ACTION_MAIN has no MIME type/data
                data.write_i32(1 << 26); // FLAG_IMMUTABLE
                data.write_i32(0); // null options Bundle
                data.write_i32(request.user);
                let result = match system.service("activity")? {
                    ServiceOwner::Remote(service) => system.process.transact_preserving_inbound(
                        service.handle,
                        am::GET_INTENT_SENDER_WITH_FEATURE,
                        &data,
                    ),
                    ServiceOwner::Local(service) => {
                        service.transact(am::GET_INTENT_SENDER_WITH_FEATURE, &data, false)
                    }
                }
                .map_err(|status| unreachable_service("activity", status))?;
                let mut reader = result.reader();
                reader
                    .read_exception()
                    .map_err(|status| unreachable_service("activity launch sender", status))??;
                let target = reader
                    .read_binder()
                    .map_err(|status| unreachable_service("activity launch sender", status))?
                    .ok_or_else(|| {
                        Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            "ActivityManager returned null launch sender",
                        )
                    })?;
                if reader.remaining() != 0 {
                    return Err(Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "launch sender reply tail",
                    ));
                }
                system.check_package_bootstrap(&retained)?;
                match target {
                    Binder::Local(_) => Ok(crate::package::launch::Sender::local(target)),
                    Binder::Handle(_) => Ok(crate::package::launch::Sender::retained(
                        result.retain_remote_binder(target).map_err(|status| {
                            unreachable_service("launch sender capability", status)
                        })?,
                    )),
                }
            },
        ))))
    }
}

struct LaunchComponent(crate::package::intent::ComponentName);
impl aim_service_aidl::WriteParcelable for LaunchComponent {
    fn write_to(&self,parcel:&mut Parcel){parcel.write_string16(Some(&self.0.package));parcel.write_string16(Some(&self.0.class));}
}
