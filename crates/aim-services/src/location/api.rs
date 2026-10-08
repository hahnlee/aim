//! The binder interface of the location service (ILocationManager): each
//! method as `LocationManagerService`'s at the pinned tag, its checks and
//! its messages.

use std::sync::Arc;

use aim_binder_host::local::{Call, Strong};
use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::{
    android_location_ilocationmanager as ilm,
    android_location_provider_igeocodecallback as geocode_callback,
};

use super::env::{self, Identity, PERMISSION_COARSE, PERMISSION_FINE, PERMISSION_NONE};
use super::parcels::{
    self, Criteria, Geofence, LastLocationRequest, Location, LocationRequest, LocationTime,
    PendingIntent, ProviderProperties, Unread,
};
use super::provider::{GPS, Key, Mock, ProviderManager, ProviderState, Registration, Transport};
use super::*;

impl LocationManagerService {
    // ---- the binder interface ----

    pub(super) fn dispatch(&self, call: &mut Call<'_>) -> Result<Option<Parcel>> {
        let caller = Caller {
            pid: call.sender_pid,
            uid: call.sender_euid as i32,
        };
        let mut reply = Parcel::new();
        let r = &mut call.data;
        match call.code {
            ilm::GET_LAST_LOCATION => {
                let a = ilm::GetLastLocation::<LastLocationRequest>::read(r).map_err(bad_parcel)?;
                let location = self.get_last_location(
                    caller,
                    a.provider,
                    a.request.ok_or_else(|| null("request"))?,
                    a.package_name,
                    a.attribution_tag,
                )?;
                ilm::write_get_last_location_reply(&mut reply, location.as_ref());
            }
            ilm::GET_CURRENT_LOCATION => {
                let a = ilm::GetCurrentLocation::<LocationRequest>::read(r).map_err(bad_parcel)?;
                let signal = self.get_current_location(caller, a)?;
                ilm::write_get_current_location_reply(&mut reply, signal);
            }
            ilm::REGISTER_LOCATION_LISTENER => {
                let a = ilm::RegisterLocationListener::<LocationRequest>::read(r)
                    .map_err(bad_parcel)?;
                self.register_listener(caller, a)?;
                ilm::write_register_location_listener_reply(&mut reply);
            }
            ilm::UNREGISTER_LOCATION_LISTENER => {
                let a = ilm::UnregisterLocationListener::read(r).map_err(bad_parcel)?;
                let handle = handle(a.listener).ok_or_else(|| null("listener"))?;
                self.unregister(Key::Binder(handle));
                ilm::write_unregister_location_listener_reply(&mut reply);
            }
            ilm::REGISTER_LOCATION_PENDING_INTENT => {
                let a =
                    ilm::RegisterLocationPendingIntent::<LocationRequest, PendingIntent>::read(r)
                        .map_err(bad_parcel)?;
                self.register_pending_intent(caller, a)?;
                ilm::write_register_location_pending_intent_reply(&mut reply);
            }
            ilm::UNREGISTER_LOCATION_PENDING_INTENT => {
                let a = ilm::UnregisterLocationPendingIntent::<PendingIntent>::read(r)
                    .map_err(bad_parcel)?;
                let pi = a.pending_intent.ok_or_else(|| null("pendingIntent"))?;
                if let Some(h) = handle(Some(pi.0)) {
                    self.unregister(Key::Binder(h));
                }
                ilm::write_unregister_location_pending_intent_reply(&mut reply);
            }
            ilm::INJECT_LOCATION => {
                let a = ilm::InjectLocation::<Location>::read(r).map_err(bad_parcel)?;
                self.enforce_all_of(caller, &[LOCATION_HARDWARE, env::ACCESS_FINE_LOCATION])?;
                let location = a.location.ok_or_else(|| null("location"))?;
                if !location.is_complete() {
                    return Err(Exception::illegal_argument(""));
                }
                let user = caller.user_id();
                self.with(|inner, env, effects| {
                    let name = location.provider.clone().unwrap_or_default();
                    if let Some(m) = visible(inner, env, &name, caller)
                        && m.is_enabled(env, user, effects)
                    {
                        m.inject_last_location(env, &location, user, effects);
                    }
                });
                ilm::write_inject_location_reply(&mut reply);
            }
            ilm::REQUEST_LISTENER_FLUSH => {
                let a = ilm::RequestListenerFlush::read(r).map_err(bad_parcel)?;
                let h = handle(a.listener).ok_or_else(|| null("listener"))?;
                self.flush(
                    caller,
                    a.provider,
                    Key::Binder(h),
                    a.request_code,
                    "listener",
                )?;
                ilm::write_request_listener_flush_reply(&mut reply);
            }
            ilm::REQUEST_PENDING_INTENT_FLUSH => {
                let a =
                    ilm::RequestPendingIntentFlush::<PendingIntent>::read(r).map_err(bad_parcel)?;
                let pi = a.pending_intent.ok_or_else(|| null("pendingIntent"))?;
                let h = handle(Some(pi.0)).ok_or_else(|| null("pendingIntent"))?;
                self.flush(
                    caller,
                    a.provider,
                    Key::Binder(h),
                    a.request_code,
                    "pending intent",
                )?;
                ilm::write_request_pending_intent_flush_reply(&mut reply);
            }
            ilm::REQUEST_GEOFENCE => {
                let a =
                    ilm::RequestGeofence::<Geofence, PendingIntent>::read(r).map_err(bad_parcel)?;
                self.add_geofence(caller, a)?;
                ilm::write_request_geofence_reply(&mut reply);
            }
            ilm::REMOVE_GEOFENCE => {
                let a = ilm::RemoveGeofence::<PendingIntent>::read(r).map_err(bad_parcel)?;
                let pi = a.intent.ok_or_else(|| null("pendingIntent"))?;
                if let Some(h) = handle(Some(pi.0)) {
                    self.with(|inner, env, effects| {
                        for undo in inner.geofences.remove_intent(h) {
                            undo();
                        }
                        update_geofences(inner, env, effects, self);
                    });
                }
                ilm::write_remove_geofence_reply(&mut reply);
            }
            ilm::IS_GEOCODE_AVAILABLE => {
                ilm::IsGeocodeAvailable::read(r).map_err(bad_parcel)?;
                let available = self.inner.lock().unwrap().geocoder.is_some();
                ilm::write_is_geocode_available_reply(&mut reply, available);
            }
            ilm::REVERSE_GEOCODE => {
                let a = ilm::ReverseGeocode::<parcels::ReverseGeocodeRequest>::read(r)
                    .map_err(bad_parcel)?;
                let request = a.request.ok_or_else(|| null("request"))?.0;
                self.geocode(caller, request, a.callback, false)?;
                ilm::write_reverse_geocode_reply(&mut reply);
            }
            ilm::FORWARD_GEOCODE => {
                let a = ilm::ForwardGeocode::<parcels::ForwardGeocodeRequest>::read(r)
                    .map_err(bad_parcel)?;
                let request = a.request.ok_or_else(|| null("request"))?.0;
                self.geocode(caller, request, a.callback, true)?;
                ilm::write_forward_geocode_reply(&mut reply);
            }
            ilm::GET_GNSS_CAPABILITIES => {
                ilm::GetGnssCapabilities::read(r).map_err(bad_parcel)?;
                ilm::write_get_gnss_capabilities_reply(&mut reply, Some(&gnss::capabilities()));
            }
            ilm::GET_GNSS_YEAR_OF_HARDWARE => {
                ilm::GetGnssYearOfHardware::read(r).map_err(bad_parcel)?;
                ilm::write_get_gnss_year_of_hardware_reply(&mut reply, gnss::YEAR_OF_HARDWARE);
            }
            ilm::GET_GNSS_HARDWARE_MODEL_NAME => {
                ilm::GetGnssHardwareModelName::read(r).map_err(bad_parcel)?;
                ilm::write_get_gnss_hardware_model_name_reply(
                    &mut reply,
                    &Some(gnss::HARDWARE_MODEL_NAME.into()),
                );
            }
            ilm::GET_GNSS_ANTENNA_INFOS => {
                r.enforce_interface(ilm::DESCRIPTOR).map_err(bad_parcel)?;
                // No antenna information was ever reported: null.
                reply.write_no_exception();
                reply.write_i32(-1);
            }
            ilm::REGISTER_GNSS_STATUS_CALLBACK => {
                let a = ilm::RegisterGnssStatusCallback::read(r).map_err(bad_parcel)?;
                self.add_gnss_listener(
                    caller,
                    gnss::Kind::Status,
                    a.callback,
                    a.package_name,
                    a.attribution_tag,
                    a.listener_id,
                    true,
                )?;
                ilm::write_register_gnss_status_callback_reply(&mut reply);
            }
            ilm::UNREGISTER_GNSS_STATUS_CALLBACK => {
                let a = ilm::UnregisterGnssStatusCallback::read(r).map_err(bad_parcel)?;
                self.remove_gnss_listener(gnss::Kind::Status, a.callback);
                ilm::write_unregister_gnss_status_callback_reply(&mut reply);
            }
            ilm::REGISTER_GNSS_NMEA_CALLBACK => {
                let a = ilm::RegisterGnssNmeaCallback::read(r).map_err(bad_parcel)?;
                self.add_gnss_listener(
                    caller,
                    gnss::Kind::Nmea,
                    a.callback,
                    a.package_name,
                    a.attribution_tag,
                    a.listener_id,
                    true,
                )?;
                ilm::write_register_gnss_nmea_callback_reply(&mut reply);
            }
            ilm::UNREGISTER_GNSS_NMEA_CALLBACK => {
                let a = ilm::UnregisterGnssNmeaCallback::read(r).map_err(bad_parcel)?;
                self.remove_gnss_listener(gnss::Kind::Nmea, a.callback);
                ilm::write_unregister_gnss_nmea_callback_reply(&mut reply);
            }
            ilm::ADD_GNSS_MEASUREMENTS_LISTENER => {
                let a =
                    ilm::AddGnssMeasurementsListener::<parcels::GnssMeasurementRequest>::read(r)
                        .map_err(bad_parcel)?;
                let request = a.request.ok_or_else(|| null("request"))?;
                self.enforce_calling(caller, env::ACCESS_FINE_LOCATION, None)?;
                if request.correlation_vector_outputs_enabled {
                    self.enforce_calling(caller, LOCATION_HARDWARE, None)?;
                }
                self.add_gnss_listener(
                    caller,
                    gnss::Kind::Measurements,
                    a.listener,
                    a.package_name,
                    a.attribution_tag,
                    a.listener_id,
                    false,
                )?;
                ilm::write_add_gnss_measurements_listener_reply(&mut reply);
            }
            ilm::REMOVE_GNSS_MEASUREMENTS_LISTENER => {
                let a = ilm::RemoveGnssMeasurementsListener::read(r).map_err(bad_parcel)?;
                self.remove_gnss_listener(gnss::Kind::Measurements, a.listener);
                ilm::write_remove_gnss_measurements_listener_reply(&mut reply);
            }
            ilm::INJECT_GNSS_MEASUREMENT_CORRECTIONS => {
                ilm::InjectGnssMeasurementCorrections::<Unread>::read(r).map_err(bad_parcel)?;
                self.enforce_calling(caller, LOCATION_HARDWARE, None)?;
                self.enforce_calling(caller, env::ACCESS_FINE_LOCATION, None)?;
                // The HAL has no measurement corrections to take them.
                eprintln!("location: failed to inject GNSS measurement corrections");
                ilm::write_inject_gnss_measurement_corrections_reply(&mut reply);
            }
            ilm::ADD_GNSS_NAVIGATION_MESSAGE_LISTENER => {
                let a = ilm::AddGnssNavigationMessageListener::read(r).map_err(bad_parcel)?;
                self.add_gnss_listener(
                    caller,
                    gnss::Kind::NavigationMessages,
                    a.listener,
                    a.package_name,
                    a.attribution_tag,
                    a.listener_id,
                    true,
                )?;
                ilm::write_add_gnss_navigation_message_listener_reply(&mut reply);
            }
            ilm::REMOVE_GNSS_NAVIGATION_MESSAGE_LISTENER => {
                let a = ilm::RemoveGnssNavigationMessageListener::read(r).map_err(bad_parcel)?;
                self.remove_gnss_listener(gnss::Kind::NavigationMessages, a.listener);
                ilm::write_remove_gnss_navigation_message_listener_reply(&mut reply);
            }
            ilm::ADD_GNSS_ANTENNA_INFO_LISTENER => {
                let a = ilm::AddGnssAntennaInfoListener::read(r).map_err(bad_parcel)?;
                self.add_gnss_listener(
                    caller,
                    gnss::Kind::AntennaInfo,
                    a.listener,
                    a.package_name,
                    a.attribution_tag,
                    a.listener_id,
                    false,
                )?;
                ilm::write_add_gnss_antenna_info_listener_reply(&mut reply);
            }
            ilm::REMOVE_GNSS_ANTENNA_INFO_LISTENER => {
                let a = ilm::RemoveGnssAntennaInfoListener::read(r).map_err(bad_parcel)?;
                self.remove_gnss_listener(gnss::Kind::AntennaInfo, a.listener);
                ilm::write_remove_gnss_antenna_info_listener_reply(&mut reply);
            }
            ilm::ADD_PROVIDER_REQUEST_LISTENER => {
                let a = ilm::AddProviderRequestListener::read(r).map_err(bad_parcel)?;
                self.enforce_permission(caller, INTERACT_ACROSS_USERS)?;
                let h = handle(a.listener).ok_or_else(|| null("listener"))?;
                let strong = Arc::new(self.env.process.strong(h));
                self.with(|inner, env, _| {
                    for m in &mut inner.providers {
                        if m.visible_to(env, caller.uid, caller.pid) {
                            m.request_listeners.push(strong.clone());
                        }
                    }
                });
                ilm::write_add_provider_request_listener_reply(&mut reply);
            }
            ilm::REMOVE_PROVIDER_REQUEST_LISTENER => {
                let a = ilm::RemoveProviderRequestListener::read(r).map_err(bad_parcel)?;
                if let Some(h) = handle(a.listener) {
                    let mut inner = self.inner.lock().unwrap();
                    for m in &mut inner.providers {
                        m.request_listeners.retain(|l| l.handle != h);
                    }
                }
                ilm::write_remove_provider_request_listener_reply(&mut reply);
            }
            ilm::GET_GNSS_BATCH_SIZE => {
                ilm::GetGnssBatchSize::read(r).map_err(bad_parcel)?;
                // The HAL has no batching.
                ilm::write_get_gnss_batch_size_reply(&mut reply, 0);
            }
            ilm::START_GNSS_BATCH => {
                let a = ilm::StartGnssBatch::read(r).map_err(bad_parcel)?;
                self.enforce_permission(caller, LOCATION_HARDWARE)?;
                self.start_gnss_batch(caller, a)?;
                ilm::write_start_gnss_batch_reply(&mut reply);
            }
            ilm::FLUSH_GNSS_BATCH => {
                ilm::FlushGnssBatch::read(r).map_err(bad_parcel)?;
                self.enforce_permission(caller, LOCATION_HARDWARE)?;
                let batching = self.inner.lock().unwrap().batching;
                if let Some(h) = batching {
                    self.flush(caller, Some(GPS.into()), Key::Binder(h), 0, "listener")?;
                }
                ilm::write_flush_gnss_batch_reply(&mut reply);
            }
            ilm::STOP_GNSS_BATCH => {
                ilm::StopGnssBatch::read(r).map_err(bad_parcel)?;
                self.enforce_permission(caller, LOCATION_HARDWARE)?;
                let batching = self.inner.lock().unwrap().batching.take();
                if let Some(h) = batching {
                    self.unregister(Key::Binder(h));
                }
                ilm::write_stop_gnss_batch_reply(&mut reply);
            }
            ilm::HAS_PROVIDER => {
                let a = ilm::HasProvider::read(r).map_err(bad_parcel)?;
                let has = self.with(|inner, env, _| {
                    a.provider
                        .as_deref()
                        .is_some_and(|p| visible(inner, env, p, caller).is_some())
                });
                ilm::write_has_provider_reply(&mut reply, has);
            }
            ilm::GET_ALL_PROVIDERS => {
                ilm::GetAllProviders::read(r).map_err(bad_parcel)?;
                let names = self.with(|inner, env, _| {
                    inner
                        .providers
                        .iter()
                        .filter(|m| m.visible_to(env, caller.uid, caller.pid))
                        .map(|m| Some(m.name.clone()))
                        .collect::<Vec<_>>()
                });
                ilm::write_get_all_providers_reply(&mut reply, &Some(names));
            }
            ilm::GET_PROVIDERS => {
                let a = ilm::GetProviders::<Criteria>::read(r).map_err(bad_parcel)?;
                let names = self.get_providers(caller, a.criteria.as_ref(), a.enabled_only);
                ilm::write_get_providers_reply(
                    &mut reply,
                    &Some(names.into_iter().map(Some).collect()),
                );
            }
            ilm::GET_BEST_PROVIDER => {
                let a = ilm::GetBestProvider::<Criteria>::read(r).map_err(bad_parcel)?;
                let mut names = self.get_providers(caller, a.criteria.as_ref(), a.enabled_only);
                if names.is_empty() {
                    names = self.get_providers(caller, None, a.enabled_only);
                }
                let best = [FUSED, GPS, NETWORK]
                    .into_iter()
                    .find(|p| names.iter().any(|n| n == p))
                    .map(String::from)
                    .or_else(|| names.first().cloned());
                ilm::write_get_best_provider_reply(&mut reply, &best);
            }
            ilm::GET_PROVIDER_PROPERTIES => {
                let a = ilm::GetProviderProperties::read(r).map_err(bad_parcel)?;
                let name = a.provider.unwrap_or_default();
                let properties = self.with(|inner, env, _| {
                    visible(inner, env, &name, caller).map(|m| m.properties())
                });
                let Some(properties) = properties else {
                    return Err(does_not_exist(&name));
                };
                ilm::write_get_provider_properties_reply(&mut reply, properties.as_ref());
            }
            ilm::IS_PROVIDER_PACKAGE => {
                let a = ilm::IsProviderPackage::read(r).map_err(bad_parcel)?;
                self.enforce_permission(caller, READ_DEVICE_CONFIG)?;
                let package = a.package_name.unwrap_or_default();
                let is = self.with(|inner, _, _| {
                    inner.providers.iter().any(|m| {
                        a.provider.as_ref().is_none_or(|p| *p == m.name)
                            && m.identity().is_some_and(|i| {
                                i.package == package
                                    && (a.attribution_tag.is_none()
                                        || i.attribution_tag == a.attribution_tag)
                            })
                    })
                });
                ilm::write_is_provider_package_reply(&mut reply, is);
            }
            ilm::GET_PROVIDER_PACKAGES => {
                let a = ilm::GetProviderPackages::read(r).map_err(bad_parcel)?;
                self.enforce_permission(caller, READ_DEVICE_CONFIG)?;
                let name = a.provider.unwrap_or_default();
                let packages = self.with(|inner, env, _| {
                    visible(inner, env, &name, caller)
                        .and_then(|m| m.identity())
                        .map(|i| vec![Some(i.package)])
                        .unwrap_or_default()
                });
                ilm::write_get_provider_packages_reply(&mut reply, &Some(packages));
            }
            ilm::SET_EXTRA_LOCATION_CONTROLLER_PACKAGE => {
                let a = ilm::SetExtraLocationControllerPackage::read(r).map_err(bad_parcel)?;
                self.enforce_permission(caller, LOCATION_HARDWARE)?;
                self.inner.lock().unwrap().extra_controller = a.package_name;
                ilm::write_set_extra_location_controller_package_reply(&mut reply);
            }
            ilm::GET_EXTRA_LOCATION_CONTROLLER_PACKAGE => {
                ilm::GetExtraLocationControllerPackage::read(r).map_err(bad_parcel)?;
                let package = self.inner.lock().unwrap().extra_controller.clone();
                ilm::write_get_extra_location_controller_package_reply(&mut reply, &package);
            }
            ilm::SET_EXTRA_LOCATION_CONTROLLER_PACKAGE_ENABLED => {
                let a =
                    ilm::SetExtraLocationControllerPackageEnabled::read(r).map_err(bad_parcel)?;
                self.enforce_permission(caller, LOCATION_HARDWARE)?;
                self.inner.lock().unwrap().extra_controller_enabled = a.enabled;
                ilm::write_set_extra_location_controller_package_enabled_reply(&mut reply);
            }
            ilm::IS_EXTRA_LOCATION_CONTROLLER_PACKAGE_ENABLED => {
                ilm::IsExtraLocationControllerPackageEnabled::read(r).map_err(bad_parcel)?;
                let enabled = {
                    let inner = self.inner.lock().unwrap();
                    inner.extra_controller_enabled && inner.extra_controller.is_some()
                };
                ilm::write_is_extra_location_controller_package_enabled_reply(&mut reply, enabled);
            }
            ilm::IS_PROVIDER_ENABLED_FOR_USER => {
                let a = ilm::IsProviderEnabledForUser::read(r).map_err(bad_parcel)?;
                let user = self.incoming_user(caller, a.user_id, "isProviderEnabledForUser")?;
                let name = a.provider.unwrap_or_default();
                let enabled = self.with(|inner, env, effects| {
                    visible(inner, env, &name, caller)
                        .is_some_and(|m| m.is_enabled(env, user, effects))
                });
                ilm::write_is_provider_enabled_for_user_reply(&mut reply, enabled);
            }
            ilm::IS_LOCATION_ENABLED_FOR_USER => {
                let a = ilm::IsLocationEnabledForUser::read(r).map_err(bad_parcel)?;
                let user = self.incoming_user(caller, a.user_id, "isLocationEnabledForUser")?;
                let enabled = self.env.location_enabled(user);
                ilm::write_is_location_enabled_for_user_reply(&mut reply, enabled);
            }
            ilm::SET_LOCATION_ENABLED_FOR_USER => {
                let a = ilm::SetLocationEnabledForUser::read(r).map_err(bad_parcel)?;
                let user = self.incoming_user(caller, a.user_id, "setLocationEnabledForUser")?;
                self.enforce_calling(caller, WRITE_SECURE_SETTINGS, None)?;
                let mode = if a.enabled {
                    LOCATION_MODE_ON
                } else {
                    LOCATION_MODE_OFF
                };
                self.env
                    .settings
                    .put_secure(env::LOCATION_MODE, mode, user)?;
                ilm::write_set_location_enabled_for_user_reply(&mut reply);
            }
            ilm::IS_ADAS_GNSS_LOCATION_ENABLED_FOR_USER => {
                let a = ilm::IsAdasGnssLocationEnabledForUser::read(r).map_err(bad_parcel)?;
                self.incoming_user(caller, a.user_id, "isAdasGnssLocationEnabledForUser")?;
                // Only an automotive device keeps it on (LocationSettings).
                ilm::write_is_adas_gnss_location_enabled_for_user_reply(&mut reply, false);
            }
            ilm::SET_ADAS_GNSS_LOCATION_ENABLED_FOR_USER => {
                let a = ilm::SetAdasGnssLocationEnabledForUser::read(r).map_err(bad_parcel)?;
                self.incoming_user(caller, a.user_id, "setAdasGnssLocationEnabledForUser")?;
                self.enforce_bypass(caller)?;
                // Filtered off on a device that is not automotive.
                ilm::write_set_adas_gnss_location_enabled_for_user_reply(&mut reply);
            }
            ilm::IS_AUTOMOTIVE_GNSS_SUSPENDED | ilm::SET_AUTOMOTIVE_GNSS_SUSPENDED => {
                if call.code == ilm::IS_AUTOMOTIVE_GNSS_SUSPENDED {
                    ilm::IsAutomotiveGnssSuspended::read(r).map(drop)
                } else {
                    ilm::SetAutomotiveGnssSuspended::read(r).map(drop)
                }
                .map_err(bad_parcel)?;
                self.enforce_permission(caller, CONTROL_AUTOMOTIVE_GNSS)?;
                let what = if call.code == ilm::IS_AUTOMOTIVE_GNSS_SUSPENDED {
                    "isAutomotiveGnssSuspended"
                } else {
                    "setAutomotiveGnssSuspended"
                };
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("{what} only allowed on automotive devices"),
                ));
            }
            ilm::ADD_TEST_PROVIDER => {
                let a = ilm::AddTestProvider::<ProviderProperties>::read(r).map_err(bad_parcel)?;
                self.add_test_provider(caller, a)?;
                ilm::write_add_test_provider_reply(&mut reply);
            }
            ilm::REMOVE_TEST_PROVIDER => {
                let a = ilm::RemoveTestProvider::read(r).map_err(bad_parcel)?;
                self.remove_test_provider(caller, a)?;
                ilm::write_remove_test_provider_reply(&mut reply);
            }
            ilm::SET_TEST_PROVIDER_LOCATION => {
                let a = ilm::SetTestProviderLocation::<Location>::read(r).map_err(bad_parcel)?;
                self.set_test_provider_location(caller, a)?;
                ilm::write_set_test_provider_location_reply(&mut reply);
            }
            ilm::SET_TEST_PROVIDER_ENABLED => {
                let a = ilm::SetTestProviderEnabled::read(r).map_err(bad_parcel)?;
                self.set_test_provider_enabled(caller, a)?;
                ilm::write_set_test_provider_enabled_reply(&mut reply);
            }
            ilm::GET_GNSS_TIME_MILLIS => {
                ilm::GetGnssTimeMillis::read(r).map_err(bad_parcel)?;
                let time = self.with(|inner, env, _| {
                    let m = inner.manager(GPS)?;
                    let l = m.last_location_unsafe(
                        env,
                        env::USER_ALL,
                        PERMISSION_FINE,
                        false,
                        i64::MAX,
                    )?;
                    Some(LocationTime {
                        unix_epoch_time_ms: l.time_ms,
                        elapsed_realtime_ns: l.elapsed_realtime_ns,
                    })
                });
                ilm::write_get_gnss_time_millis_reply(&mut reply, time.as_ref());
            }
            ilm::SEND_EXTRA_COMMAND => {
                self.send_extra_command(caller, r, &mut reply)?;
            }
            ilm::GET_BACKGROUND_THROTTLING_WHITELIST => {
                ilm::GetBackgroundThrottlingWhitelist::read(r).map_err(bad_parcel)?;
                let list: Vec<Option<String>> = self
                    .env
                    .background_throttle_package_whitelist()
                    .into_iter()
                    .map(Some)
                    .collect();
                ilm::write_get_background_throttling_whitelist_reply(&mut reply, &Some(list));
            }
            ilm::GET_IGNORE_SETTINGS_ALLOWLIST => {
                ilm::GetIgnoreSettingsAllowlist::read(r).map_err(bad_parcel)?;
                ilm::write_get_ignore_settings_allowlist_reply(
                    &mut reply,
                    Some(&self.env.ignore_settings_allowlist()),
                );
            }
            ilm::GET_ADAS_ALLOWLIST => {
                ilm::GetAdasAllowlist::read(r).map_err(bad_parcel)?;
                ilm::write_get_adas_allowlist_reply(&mut reply, Some(&self.env.adas_allowlist()));
            }
            DUMP_TRANSACTION => {
                self.dump(call);
                reply.write_no_exception();
            }
            _ => return Ok(None),
        }
        Ok(Some(reply))
    }

    // ---- checks ----

    /// `Context.enforceCallingOrSelfPermission(permission, message)`.
    fn enforce_calling(
        &self,
        caller: Caller,
        permission: &str,
        message: Option<&str>,
    ) -> Result<()> {
        if self
            .env
            .check_permission(permission, caller.pid, caller.uid)
        {
            return Ok(());
        }
        Err(Exception::security(format!(
            "{}Neither user {} nor current process has {permission}.",
            message.map(|m| format!("{m}: ")).unwrap_or_default(),
            caller.uid
        )))
    }

    /// What `@EnforcePermission` generates.
    fn enforce_permission(&self, caller: Caller, permission: &str) -> Result<()> {
        if self
            .env
            .check_permission(permission, caller.pid, caller.uid)
        {
            return Ok(());
        }
        Err(Exception::security(format!(
            "Access denied, requires: {permission}"
        )))
    }

    fn enforce_all_of(&self, caller: Caller, permissions: &[&str]) -> Result<()> {
        if permissions
            .iter()
            .all(|p| self.env.check_permission(p, caller.pid, caller.uid))
        {
            return Ok(());
        }
        Err(Exception::security(format!(
            "Access denied, requires: allOf={{{}}}",
            permissions.join(", ")
        )))
    }

    /// `LocationPermissions.enforceCallingOrSelfBypassPermission`.
    fn enforce_bypass(&self, caller: Caller) -> Result<()> {
        if self
            .env
            .check_permission(LOCATION_BYPASS, caller.pid, caller.uid)
        {
            return Ok(());
        }
        Err(Exception::security(format!(
            "uid{} does not have {LOCATION_BYPASS}.",
            caller.uid
        )))
    }

    /// `ActivityManager.handleIncomingUser(pid, uid, user, false, false,
    /// name, null)`.
    fn incoming_user(&self, caller: Caller, user: i32, name: &str) -> Result<i32> {
        self.env
            .system
            .handle_incoming_user(caller.pid, caller.uid, user, false, name, None)
    }

    /// `CallerIdentity.fromBinder`: the package must be the caller's.
    fn identity(
        &self,
        caller: Caller,
        package: Option<String>,
        tag: Option<String>,
        listener_id: Option<String>,
    ) -> Result<Identity> {
        let package = package.ok_or_else(|| null("packageName"))?;
        if self.env.system.check_package(caller.uid, &package).is_err() {
            return Err(Exception::security(format!(
                "invalid package \"{package}\" for uid {}",
                caller.uid
            )));
        }
        Ok(Identity {
            uid: caller.uid,
            pid: caller.pid,
            package,
            attribution_tag: tag,
            listener_id,
        })
    }

    /// The permission level of a location client, with the bypass
    /// permission counting as fine and otherwise at least coarse
    /// required (`enable_location_bypass` is on in the image).
    fn client_permission_level(&self, caller: Caller) -> Result<i32> {
        let level = self.env.permission_level(caller.uid, caller.pid);
        if level != PERMISSION_NONE {
            return Ok(level);
        }
        if self
            .env
            .check_permission(LOCATION_BYPASS, caller.pid, caller.uid)
        {
            return Ok(PERMISSION_FINE);
        }
        Err(Exception::security(format!(
            "uid {} does not have {} or {}.",
            caller.uid,
            env::ACCESS_COARSE_LOCATION,
            env::ACCESS_FINE_LOCATION
        )))
    }

    /// `validateLocationRequest`: the request sanitized, its work source
    /// the caller's unless a valid one was given.
    fn validate_request(
        &self,
        caller: Caller,
        request: LocationRequest,
        identity: &Identity,
    ) -> Result<LocationRequest> {
        if !request.work_source.is_empty() {
            self.enforce_calling(
                caller,
                UPDATE_DEVICE_STATS,
                Some("setting a work source requires android.permission.UPDATE_DEVICE_STATS"),
            )?;
        }
        let mut sanitized = request.rebuilt();
        if !self.env.change_enabled(LOW_POWER_EXCEPTIONS, caller.uid)
            && !self
                .env
                .check_permission(LOCATION_HARDWARE, caller.pid, caller.uid)
        {
            sanitized.low_power = false;
        }
        let mut ws = request.work_source.clone();
        let no_package = !ws.uids.is_empty() && ws.first_name().is_none();
        let no_tag = ws
            .chains
            .as_ref()
            .and_then(|c| c.first())
            .is_some_and(|c| c.tags.first().is_none_or(Option::is_none));
        if no_package || no_tag {
            ws = parcels::WorkSource::default();
        }
        if ws.is_empty() {
            ws.add_named(identity.uid, &identity.package);
        }
        sanitized.work_source = ws;
        let is_provider = self.env.is_provider(None, identity);
        if sanitized.low_power && self.env.change_enabled(LOW_POWER_EXCEPTIONS, identity.uid) {
            self.enforce_calling(
                caller,
                LOCATION_HARDWARE,
                Some("low power request requires android.permission.LOCATION_HARDWARE"),
            )?;
        }
        if sanitized.hidden_from_app_ops {
            self.enforce_calling(
                caller,
                UPDATE_APP_OPS_STATS,
                Some("hiding from app ops requires android.permission.UPDATE_APP_OPS_STATS"),
            )?;
        }
        if sanitized.adas_gnss_bypass {
            return Err(Exception::illegal_argument(
                "adas gnss bypass requests are only allowed on automotive devices",
            ));
        }
        if sanitized.location_settings_ignored && !is_provider {
            self.enforce_bypass(caller)?;
        }
        Ok(sanitized)
    }

    // ---- operations ----

    fn get_last_location(
        &self,
        caller: Caller,
        provider: Option<String>,
        request: LastLocationRequest,
        package: Option<String>,
        tag: Option<String>,
    ) -> Result<Option<Location>> {
        let identity = self.identity(caller, package, tag, None)?;
        let level = self.client_permission_level(caller)?;
        // validateLastLocationRequest
        if request.hidden_from_app_ops {
            self.enforce_calling(
                caller,
                UPDATE_APP_OPS_STATS,
                Some("hiding from app ops requires android.permission.UPDATE_APP_OPS_STATS"),
            )?;
        }
        if request.adas_gnss_bypass {
            return Err(Exception::illegal_argument(
                "adas gnss bypass requests are only allowed on automotive devices",
            ));
        }
        if request.location_settings_ignored && !self.env.is_provider(None, &identity) {
            self.enforce_bypass(caller)?;
        }
        let Some(name) = provider else {
            return Ok(None);
        };
        Ok(self.with(|inner, env, effects| {
            let m = visible(inner, env, &name, caller)?;
            m.get_last_location(env, &request, &identity, level, effects)
        }))
    }

    fn get_current_location(
        &self,
        caller: Caller,
        a: ilm::GetCurrentLocation<LocationRequest>,
    ) -> Result<Option<Binder>> {
        self.ensure_watchers();
        let identity = self.identity(caller, a.package_name, a.attribution_tag, a.listener_id)?;
        let level = self.client_permission_level(caller)?;
        let request = a.request.ok_or_else(|| null("request"))?;
        let mut request = self.validate_request(caller, request, &identity)?;
        let name = a.provider.unwrap_or_default();
        let h = handle(a.callback).ok_or_else(|| null("callback"))?;
        if request.duration_ms > provider::MAX_GET_CURRENT_LOCATION_TIMEOUT_MS {
            request.duration_ms = provider::MAX_GET_CURRENT_LOCATION_TIMEOUT_MS;
        }
        let exists = self.with(|inner, env, _| visible(inner, env, &name, caller).is_some());
        if !exists {
            return Err(does_not_exist(&name));
        }
        let callback = Arc::new(self.env.process.strong(h));
        let key = Key::Binder(h);
        let mut reg = Registration::new(
            Transport::Current(callback.clone()),
            request,
            identity,
            level,
        );
        reg.cleanup.push(self.death_link(&callback, &name, key));
        self.with(|inner, env, effects| {
            if let Some(m) = inner.manager(&name) {
                m.put(env, key, reg, effects);
            }
        });
        let signal = self.env.process.add_service(Arc::new(CancelCurrent {
            service: self.this.clone(),
            provider: name,
            key,
            callback,
        }));
        Ok(Some(signal))
    }

    fn register_listener(
        &self,
        caller: Caller,
        a: ilm::RegisterLocationListener<LocationRequest>,
    ) -> Result<()> {
        self.ensure_watchers();
        let identity = self.identity(caller, a.package_name, a.attribution_tag, a.listener_id)?;
        let level = self.client_permission_level(caller)?;
        let request = a.request.ok_or_else(|| null("request"))?;
        let request = self.validate_request(caller, request, &identity)?;
        let name = a.provider.unwrap_or_default();
        let h = handle(a.listener).ok_or_else(|| null("listener"))?;
        self.put_registration(
            caller,
            &name,
            h,
            Transport::Listener,
            request,
            identity,
            level,
        )
    }

    fn register_pending_intent(
        &self,
        caller: Caller,
        a: ilm::RegisterLocationPendingIntent<LocationRequest, PendingIntent>,
    ) -> Result<()> {
        self.ensure_watchers();
        let pi = a.pending_intent.ok_or_else(|| null("pendingIntent"))?;
        let h = handle(Some(pi.0)).ok_or_else(|| null("pendingIntent"))?;
        let identity = self.identity(
            caller,
            a.package_name,
            a.attribution_tag,
            Some(format!("PendingIntent@{h}")),
        )?;
        let level = self.client_permission_level(caller)?;
        let request = a.request.ok_or_else(|| null("request"))?;
        if self
            .env
            .change_enabled(BLOCK_PENDING_INTENT_SYSTEM_API_USAGE, identity.uid)
            && (request.low_power
                || request.hidden_from_app_ops
                || request.location_settings_ignored
                || !request.work_source.is_empty())
        {
            return Err(Exception::security(format!(
                "PendingIntent location requests may not use system APIs: {request}"
            )));
        }
        let request = self.validate_request(caller, request, &identity)?;
        let name = a.provider.unwrap_or_default();
        self.put_registration(
            caller,
            &name,
            h,
            Transport::PendingIntent,
            request,
            identity,
            level,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn put_registration(
        &self,
        caller: Caller,
        name: &str,
        h: u32,
        transport: impl FnOnce(Arc<Strong>) -> Transport,
        request: LocationRequest,
        identity: Identity,
        level: i32,
    ) -> Result<()> {
        let exists = self.with(|inner, env, _| visible(inner, env, name, caller).is_some());
        if !exists {
            return Err(does_not_exist(name));
        }
        let strong = Arc::new(self.env.process.strong(h));
        let key = Key::Binder(h);
        let transport = transport(strong.clone());
        let pending_intent = matches!(transport, Transport::PendingIntent(_));
        let mut reg = Registration::new(transport, request, identity, level);
        if pending_intent {
            match self.cancel_listener(&strong, name, key) {
                Some(undo) => reg.cleanup.push(undo),
                // Already cancelled: `addCancelListener` fails and the
                // registration goes at once.
                None => return Ok(()),
            }
        } else {
            reg.cleanup.push(self.death_link(&strong, name, key));
        }
        self.with(|inner, env, effects| {
            if let Some(m) = inner.manager(name) {
                m.put(env, key, reg, effects);
            }
        });
        Ok(())
    }

    /// Removes `key`'s registration from the provider when its client
    /// dies; the returned closure undoes the link.
    fn death_link(&self, strong: &Arc<Strong>, name: &str, key: Key) -> Box<dyn FnOnce() + Send> {
        let this = self.this.clone();
        let provider = name.to_string();
        let fg = self.fg.clone();
        let cookie = self.env.process.link_to_death(
            strong,
            Box::new(move || {
                fg.post(move || {
                    if let Some(service) = this.upgrade() {
                        service.with(|inner, env, effects| {
                            if let Some(m) = inner.manager(&provider) {
                                m.remove(env, key, effects);
                            }
                        });
                    }
                });
            }),
        );
        let process = self.env.process.clone();
        let strong = strong.clone();
        Box::new(move || process.clear_death(&strong, cookie))
    }

    /// `PendingIntent.addCancelListener`: `None` if the intent is already
    /// cancelled.
    fn cancel_listener(
        &self,
        pi: &Arc<Strong>,
        name: &str,
        key: Key,
    ) -> Option<Box<dyn FnOnce() + Send>> {
        use aim_service_aidl::android_app_iactivitymanager as am;
        let receiver = self.env.process.add_service(Arc::new(CancelReceiver {
            service: self.this.clone(),
            provider: name.into(),
            key,
        }));
        let args = am::RegisterIntentSenderCancelListenerEx {
            sender: Some(pi.binder()),
            receiver: Some(receiver),
        };
        let registered = self
            .env
            .system
            .call(
                "activity",
                am::REGISTER_INTENT_SENDER_CANCEL_LISTENER_EX,
                |p| args.write(p),
                am::read_register_intent_sender_cancel_listener_ex_reply,
            )
            .unwrap_or(false);
        if !registered {
            return None;
        }
        let env = self.env.clone();
        let pi = pi.clone();
        Some(Box::new(move || {
            let args = am::UnregisterIntentSenderCancelListener {
                sender: Some(pi.binder()),
                receiver: Some(receiver),
            };
            let _ = env.system.call(
                "activity",
                am::UNREGISTER_INTENT_SENDER_CANCEL_LISTENER,
                |p| args.write(p),
                am::read_unregister_intent_sender_cancel_listener_reply,
            );
        }))
    }

    /// `unregisterLocationRequest` on every provider.
    fn unregister(&self, key: Key) {
        self.with(|inner, env, effects| {
            for m in &mut inner.providers {
                m.remove(env, key, effects);
            }
        });
    }

    fn flush(
        &self,
        caller: Caller,
        provider: Option<String>,
        key: Key,
        request_code: i32,
        what: &str,
    ) -> Result<()> {
        let name = provider.unwrap_or_default();
        let this = self.this.clone();
        let provider = name.clone();
        let deferred = move || -> Box<dyn FnOnce() + Send> {
            Box::new(move || {
                let Some(service) = this.upgrade() else {
                    return;
                };
                let s = service.clone();
                service.fg.post(move || {
                    let inner = s.inner.lock().unwrap();
                    if let Some(m) = inner.providers.iter().find(|m| m.name == provider) {
                        m.flush_complete(&s.env, key, request_code);
                    }
                });
            })
        };
        let flushed = self.with(|inner, env, _| {
            visible(inner, env, &name, caller).map(|m| m.flush(env, key, request_code, deferred))
        });
        match flushed {
            None => Err(does_not_exist(&name)),
            Some(false) => Err(Exception::illegal_argument(format!(
                "unregistered {what} cannot be flushed"
            ))),
            Some(true) => Ok(()),
        }
    }

    fn get_providers(
        &self,
        caller: Caller,
        criteria: Option<&Criteria>,
        enabled_only: bool,
    ) -> Vec<String> {
        if self.env.permission_level(caller.uid, caller.pid) < PERMISSION_COARSE {
            return Vec::new();
        }
        self.with(|inner, env, effects| {
            let mut names = Vec::new();
            for m in &mut inner.providers {
                if !m.visible_to(env, caller.uid, caller.pid) {
                    continue;
                }
                if enabled_only && !m.is_enabled(env, caller.user_id(), effects) {
                    continue;
                }
                if criteria.is_some_and(|c| !c.met_by(&m.name, m.properties().as_ref())) {
                    continue;
                }
                names.push(m.name.clone());
            }
            names
        })
    }

    /// `reverseGeocode` and `forwardGeocode`: the request goes to the
    /// geocode provider's service; its callback fails without one
    /// (`null`) or while it is not bound (`runOnBinder`'s error).
    fn geocode(
        &self,
        caller: Caller,
        request: parcels::GeocodeRequest,
        callback: Option<Binder>,
        forward: bool,
    ) -> Result<()> {
        use aim_service_aidl::android_location_provider_igeocodeprovider as provider;
        let identity = self.identity(
            caller,
            Some(request.calling_package.clone()),
            request.calling_attribution_tag.clone(),
            None,
        )?;
        if identity.uid != request.calling_uid {
            return Err(Exception::illegal_argument(""));
        }
        let geocoder = self.inner.lock().unwrap().geocoder.clone();
        let error = match geocoder {
            Some(Some(p)) => {
                let mut data = Parcel::new();
                if forward {
                    provider::ForwardGeocode {
                        request: Some(request),
                        callback,
                    }
                    .write(&mut data);
                } else {
                    provider::ReverseGeocode {
                        request: Some(request),
                        callback,
                    }
                    .write(&mut data);
                }
                let code = if forward {
                    provider::FORWARD_GEOCODE
                } else {
                    provider::REVERSE_GEOCODE
                };
                if p.transact(code, &data, true).is_ok() {
                    return Ok(());
                }
                Some("android.os.DeadObjectException".to_string())
            }
            Some(None) => Some("android.os.DeadObjectException".to_string()),
            None => None,
        };
        if let Some(Binder::Handle(h)) = callback {
            let mut data = Parcel::new();
            geocode_callback::OnError { error }.write(&mut data);
            let _ = self
                .env
                .process
                .transact(h, geocode_callback::ON_ERROR, &data, true);
        }
        Ok(())
    }

    fn start_gnss_batch(&self, caller: Caller, a: ilm::StartGnssBatch) -> Result<()> {
        let previous = self.inner.lock().unwrap().batching.take();
        if let Some(h) = previous {
            self.unregister(Key::Binder(h));
        }
        let interval = a.period_nanos / 1_000_000;
        let mut request = LocationRequest::new(interval);
        // The batch size is 0: no delay.
        request.max_update_delay_ms = 0;
        request.hidden_from_app_ops = true;
        let h = handle(a.listener).ok_or_else(|| null("listener"))?;
        self.register_listener(
            caller,
            ilm::RegisterLocationListener {
                provider: Some(GPS.into()),
                request: Some(request),
                listener: a.listener,
                package_name: a.package_name,
                attribution_tag: a.attribution_tag,
                listener_id: a.listener_id,
            },
        )?;
        self.inner.lock().unwrap().batching = Some(h);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn add_gnss_listener(
        &self,
        caller: Caller,
        kind: gnss::Kind,
        listener: Option<Binder>,
        package: Option<String>,
        tag: Option<String>,
        listener_id: Option<String>,
        needs_fine: bool,
    ) -> Result<()> {
        if needs_fine {
            self.enforce_calling(caller, env::ACCESS_FINE_LOCATION, None)?;
        }
        let identity = self.identity(caller, package, tag, listener_id)?;
        let h = handle(listener).ok_or_else(|| null("listener"))?;
        let strong = Arc::new(self.env.process.strong(h));
        let this = self.this.clone();
        let fg = self.fg.clone();
        let cookie = self.env.process.link_to_death(
            &strong,
            Box::new(move || {
                fg.post(move || {
                    if let Some(service) = this.upgrade() {
                        service.inner.lock().unwrap().gnss.remove(kind, h);
                    }
                })
            }),
        );
        let process = self.env.process.clone();
        let s = strong.clone();
        let registration = gnss::Registration {
            identity,
            listener: strong,
            cleanup: vec![Box::new(move || process.clear_death(&s, cookie))],
        };
        self.inner.lock().unwrap().gnss.add(kind, h, registration);
        Ok(())
    }

    fn remove_gnss_listener(&self, kind: gnss::Kind, listener: Option<Binder>) {
        if let Some(h) = handle(listener) {
            self.inner.lock().unwrap().gnss.remove(kind, h);
        }
    }

    fn add_geofence(
        &self,
        caller: Caller,
        a: ilm::RequestGeofence<Geofence, PendingIntent>,
    ) -> Result<()> {
        if self.env.permission_level(caller.uid, caller.pid) < PERMISSION_FINE {
            return Err(Exception::security(format!(
                "uid {} does not have {}.",
                caller.uid,
                env::ACCESS_FINE_LOCATION
            )));
        }
        let pi = a.intent.ok_or_else(|| null("pendingIntent"))?;
        let h = handle(Some(pi.0)).ok_or_else(|| null("pendingIntent"))?;
        let geofence = a.geofence.ok_or_else(|| null("geofence"))?;
        let identity = self.identity(
            caller,
            a.package_name,
            a.attribution_tag,
            Some(format!("PendingIntent@{h}")),
        )?;
        let strong = Arc::new(self.env.process.strong(h));
        let mut registration =
            geofence::Registration::new(strong.clone(), geofence.clone(), identity);
        let this = self.this.clone();
        let fg = self.fg.clone();
        let cookie = self.env.process.link_to_death(
            &strong,
            Box::new(move || {
                fg.post(move || {
                    if let Some(service) = this.upgrade() {
                        service.with(|inner, env, effects| {
                            for undo in inner.geofences.remove_intent(h) {
                                undo();
                            }
                            update_geofences(inner, env, effects, &service);
                        });
                    }
                })
            }),
        );
        let process = self.env.process.clone();
        registration
            .cleanup
            .push(Box::new(move || process.clear_death(&strong, cookie)));
        self.with(|inner, env, effects| {
            for undo in inner.geofences.add(env, registration) {
                undo();
            }
            // `onActive`: the new fence is evaluated at the last location.
            let last = inner
                .geofences
                .last_location
                .clone()
                .or_else(|| fused_last_location(inner, env, effects));
            if let Some(l) =
                last.filter(|l| l.age_ms(env.now_ms()) <= geofence::MAX_LOCATION_AGE_MS)
            {
                let active = geofence_caller_active(env);
                for undo in inner
                    .geofences
                    .on_location(env, &active, &l, Some((h, &geofence)))
                {
                    undo();
                }
            }
            update_geofences(inner, env, effects, self);
        });
        Ok(())
    }

    /// The geofence manager's fused location arrived.
    pub(super) fn on_geofence_location(&self, l: Location) {
        self.with(|inner, env, effects| {
            let active = geofence_caller_active(env);
            for undo in inner.geofences.on_location(env, &active, &l, None) {
                undo();
            }
            update_geofences(inner, env, effects, self);
        });
    }

    fn add_test_provider(
        &self,
        caller: Caller,
        a: ilm::AddTestProvider<ProviderProperties>,
    ) -> Result<()> {
        let identity = self.unsafe_identity(caller, a.package_name, a.attribution_tag)?;
        if !self.env.note_op(OP_MOCK_LOCATION, &identity)? {
            return Ok(());
        }
        let name = a.name.ok_or_else(|| null("provider"))?;
        let properties = a.properties.ok_or_else(|| null("properties"))?;
        let tags: Vec<String> = a
            .location_tags
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .collect();
        self.with(|inner, env, effects| {
            if inner.index(&name).is_none() {
                let mut m = ProviderManager::new(&name, true, Vec::new(), env);
                m.start(env, effects);
                inner.providers.push(m);
            }
            let m = inner.manager(&name).unwrap();
            if m.is_passive() {
                return Err(Exception::illegal_argument(
                    "Cannot mock the passive provider",
                ));
            }
            m.set_mock(
                env,
                Some(Mock {
                    state: ProviderState {
                        allowed: false,
                        properties: Some(properties),
                        identity: Some(identity),
                        extra_attribution_tags: tags,
                    },
                    location: None,
                }),
                effects,
            );
            Ok(())
        })
    }

    fn remove_test_provider(&self, caller: Caller, a: ilm::RemoveTestProvider) -> Result<()> {
        let identity = self.unsafe_identity(caller, a.package_name, a.attribution_tag)?;
        if !self.env.note_op(OP_MOCK_LOCATION, &identity)? {
            return Ok(());
        }
        let name = a.provider.unwrap_or_default();
        self.with(|inner, env, effects| {
            let Some(i) = inner.index(&name) else { return };
            if !inner.providers[i].visible_to(env, caller.uid, caller.pid) {
                return;
            }
            let m = &mut inner.providers[i];
            m.set_mock(env, None, effects);
            if !m.has_provider() {
                // `removeLocationProviderManager`: every user disabled,
                // every registration dropped.
                let mut m = inner.providers.remove(i);
                for key in m.keys() {
                    m.remove(env, key, effects);
                }
            }
        });
        Ok(())
    }

    fn set_test_provider_location(
        &self,
        caller: Caller,
        a: ilm::SetTestProviderLocation<Location>,
    ) -> Result<()> {
        let identity = self.unsafe_identity(caller, a.package_name, a.attribution_tag)?;
        if !self.env.note_op(OP_MOCK_LOCATION, &identity)? {
            return Ok(());
        }
        let mut location = a.location.ok_or_else(|| null("location"))?;
        if !location.is_complete() {
            return Err(Exception::illegal_argument(
                "incomplete location object, missing timestamp or accuracy?",
            ));
        }
        let name = a.provider.unwrap_or_default();
        location.fields |= parcels::HAS_MOCK_PROVIDER;
        if let Err(e) = location.validate(0, now_ns()) {
            return Err(Exception::illegal_argument(format!(
                "android.location.LocationResult$BadLocationException: {e}"
            )));
        }
        self.with(|inner, env, effects| {
            let m = visible(inner, env, &name, caller).ok_or_else(|| {
                Exception::illegal_argument(format!("provider doesn't exist: {name}"))
            })?;
            let Some(mock) = &mut m.mock else {
                return Err(Exception::illegal_argument(format!(
                    "{name} provider is not a test provider"
                )));
            };
            mock.location = Some(location.clone());
            report_locations(inner, env, &name, vec![location], effects);
            Ok(())
        })
    }

    fn set_test_provider_enabled(
        &self,
        caller: Caller,
        a: ilm::SetTestProviderEnabled,
    ) -> Result<()> {
        let identity = self.unsafe_identity(caller, a.package_name, a.attribution_tag)?;
        if !self.env.note_op(OP_MOCK_LOCATION, &identity)? {
            return Ok(());
        }
        let name = a.provider.unwrap_or_default();
        self.with(|inner, env, effects| {
            let m = visible(inner, env, &name, caller).ok_or_else(|| {
                Exception::illegal_argument(format!("provider doesn't exist: {name}"))
            })?;
            if !m.is_mock() {
                return Err(Exception::illegal_argument(format!(
                    "{name} provider is not a test provider"
                )));
            }
            m.set_mock_allowed(env, a.enabled, effects);
            update_geofences(inner, env, effects, self);
            Ok(())
        })
    }

    /// `CallerIdentity.fromBinderUnsafe`: app ops check the package.
    fn unsafe_identity(
        &self,
        caller: Caller,
        package: Option<String>,
        tag: Option<String>,
    ) -> Result<Identity> {
        Ok(Identity {
            uid: caller.uid,
            pid: caller.pid,
            package: package.ok_or_else(|| null("packageName"))?,
            attribution_tag: tag,
            listener_id: None,
        })
    }

    /// `sendExtraCommand(provider, command, inout extras)`: no provider
    /// here acts on commands; the extras go back as they came.
    fn send_extra_command(
        &self,
        caller: Caller,
        r: &mut aim_binder_host::parcel::Reader<'_>,
        reply: &mut Parcel,
    ) -> Result<()> {
        r.enforce_interface(ilm::DESCRIPTOR).map_err(bad_parcel)?;
        let _provider = r.read_string16().map_err(bad_parcel)?;
        r.read_string16()
            .map_err(bad_parcel)?
            .ok_or_else(|| null("command"))?;
        let extras = match r.read_i32().map_err(bad_parcel)? {
            0 => None,
            _ => parcels::RawBundle::read(r).map_err(bad_parcel)?,
        };
        if self.env.permission_level(caller.uid, caller.pid) < PERMISSION_COARSE {
            return Err(Exception::security(format!(
                "uid {} does not have {} or {}.",
                caller.uid,
                env::ACCESS_COARSE_LOCATION,
                env::ACCESS_FINE_LOCATION
            )));
        }
        self.enforce_calling(caller, ACCESS_LOCATION_EXTRA_COMMANDS, None)?;
        reply.write_no_exception();
        // The `inout` bundle: written back as a typed object.
        match &extras {
            Some(b) => {
                reply.write_i32(1);
                parcels::RawBundle::write(Some(b), reply);
            }
            None => reply.write_i32(0),
        }
        Ok(())
    }

    /// `dump`: each provider's state, as `dumpsys location` shows it.
    fn dump(&self, call: &mut Call<'_>) {
        use std::io::Write;
        let Ok(fd) = call.data.read_fd() else { return };
        let Some(file) = self.env.process.file(fd) else {
            return;
        };
        let Some(mut fd) = aim_binder_host::server::file_fd(&file) else {
            return;
        };
        let text = self.with(|inner, env, effects| {
            let mut s = String::from("Location Manager State:\n  Location Providers:\n");
            for m in &mut inner.providers {
                s += &format!(
                    "    {} provider{}:\n      service: {}\n",
                    m.name,
                    if m.is_mock() { " [mock]" } else { "" },
                    m.current_request()
                );
                for key in m.keys() {
                    if let Some(r) = m.registration(key) {
                        s += &format!("        {} {}\n", r.identity, r.request());
                    }
                }
                for user in env.running_users() {
                    let enabled = m.is_enabled(env, user, effects);
                    let last = m.last_location_unsafe(env, user, PERMISSION_FINE, false, i64::MAX);
                    s += &format!(
                        "      last location={}\n      enabled={enabled}\n",
                        match last {
                            Some(l) => format!(
                                "Location[{} {:.6},{:.6}]",
                                l.provider.unwrap_or_default(),
                                l.latitude,
                                l.longitude
                            ),
                            None => "null".into(),
                        }
                    );
                }
                let state = m.provider_state();
                s += &format!("      allowed={}\n", state.allowed);
                if let Some(i) = state.identity {
                    s += &format!("      identity={i}\n");
                }
            }
            s
        });
        let _ = fd.write_all(text.as_bytes());
    }
}
