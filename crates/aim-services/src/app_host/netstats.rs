//! `netstats`: NetworkStatsService for the app host (`android-16.0.0_r1`):
//! the TrafficStats rate-limit cache configuration a client gets. The
//! statistics, sessions and providers are not implemented (#1671).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_net_inetworkstatsservice as ns;

use super::settings::Table;
use super::{AppHost, reply};

/// `NetworkStatsService`'s DeviceConfig names and defaults.
const CLIENT_CACHE_ENABLED_FLAG: &str = "trafficstats_client_rate_limit_cache_enabled_flag";
const CACHE_EXPIRY_DURATION_NAME: &str = "trafficstats_cache_expiry_duration_ms";
const CACHE_MAX_ENTRIES_NAME: &str = "trafficstats_cache_max_entries";
const DEFAULT_CACHE_EXPIRY_DURATION_MS: i32 = 1000;
const DEFAULT_CACHE_MAX_ENTRIES: i32 = 400;

/// `TrafficStatsRateLimitCacheConfig`, a structured parcelable.
struct CacheConfig {
    enabled: bool,
    expiry_duration_ms: i32,
    max_entries: i32,
}

impl WriteParcelable for CacheConfig {
    fn write_to(&self, p: &mut Parcel) {
        let start = p.position();
        p.write_i32(0);
        p.write_bool(self.enabled);
        p.write_i32(self.expiry_duration_ms);
        p.write_i32(self.max_entries);
        let size = (p.position() - start) as i32;
        p.set_i32_at(start, size);
    }
}

pub struct NetworkStats {
    host: Arc<AppHost>,
}

impl NetworkStats {
    pub fn new(host: Arc<AppHost>) -> Self {
        NetworkStats { host }
    }

    /// `DeviceConfigUtils.getDeviceConfigPropertyInt` in the tethering
    /// namespace.
    fn tethering_int(&self, name: &str, default: i32) -> Result<i32, Exception> {
        let value = self.host.inputs()?.settings.get(Table::Config, &format!("tethering/{name}"));
        Ok(value.and_then(|v| v.parse().ok()).unwrap_or(default))
    }

    /// `getTrafficStatsRateLimitCacheClientSideConfig`; the cache is on by
    /// `isTetheringFeatureEnabled` (off by default, -1 forces it off and 1
    /// on; another value compares with the module's version, which is not
    /// modelled).
    fn cache_config(&self) -> Result<CacheConfig, Exception> {
        let enabled = match self.tethering_int(CLIENT_CACHE_ENABLED_FLAG, 0)? {
            0 | -1 => false,
            1 => true,
            _ => return Err(self.host.unimplemented_named("netstats", "a feature flag against the tethering module's version (#1671)")),
        };
        Ok(CacheConfig {
            enabled,
            expiry_duration_ms: self.tethering_int(CACHE_EXPIRY_DURATION_NAME, DEFAULT_CACHE_EXPIRY_DURATION_MS)?,
            max_entries: self.tethering_int(CACHE_MAX_ENTRIES_NAME, DEFAULT_CACHE_MAX_ENTRIES)?,
        })
    }
}

impl Service for NetworkStats {
    fn descriptor(&self) -> &str {
        ns::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            // `getRateLimitCacheConfig`: the client side configuration,
            // enabled for a caller only if the cache is on (on V+,
            // `useClientSideCache` needs no compat change).
            ns::GET_RATE_LIMIT_CACHE_CONFIG => {
                ns::GetRateLimitCacheConfig::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.cache_config() {
                    Ok(config) => ns::write_get_rate_limit_cache_config_reply(&mut p, Some(&config)),
                    Err(e) => return reply(Err(e)),
                }
            }
            code => {
                let name = ns::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                return reply(Err(self.host.unimplemented_named("netstats", &format!("{name} (#1671)"))));
            }
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The structured parcelable's size, then its fields.
    #[test]
    fn cache_config_is_a_structured_parcelable() {
        let mut p = Parcel::new();
        CacheConfig { enabled: false, expiry_duration_ms: 1000, max_entries: 400 }.write_to(&mut p);
        let mut expected = Parcel::new();
        for v in [16, 0, 1000, 400] {
            expected.write_i32(v);
        }
        assert_eq!(p.data(), expected.data());
    }
}
