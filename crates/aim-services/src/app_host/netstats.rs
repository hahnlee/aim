//! `netstats`: NetworkStatsService for the app host (`android-16.0.0_r1`):
//! the TrafficStats rate-limit cache configuration a client gets, and a
//! uid's traffic (`getUidStats`), which `nativeGetUidStat` reads from
//! netd's `app_uid_stats_map` (filled by the Linux layer's socket
//! accounting), through the service-side rate-limit cache. The other
//! statistics, sessions and providers are not implemented (#1671).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_net_inetworkstatsservice as ns;
use aim_storage::bpf_object::Mapped;

use super::settings::Table;
use super::{AppHost, reply};

/// `NetworkStatsService`'s DeviceConfig names and defaults.
const CLIENT_CACHE_ENABLED_FLAG: &str = "trafficstats_client_rate_limit_cache_enabled_flag";
const SERVICE_CACHE_ENABLED_FLAG: &str = "trafficstats_rate_limit_cache_enabled_flag";
const CACHE_EXPIRY_DURATION_NAME: &str = "trafficstats_cache_expiry_duration_ms";
const CACHE_MAX_ENTRIES_NAME: &str = "trafficstats_cache_max_entries";
const DEFAULT_CACHE_EXPIRY_DURATION_MS: i32 = 1000;
const DEFAULT_CACHE_MAX_ENTRIES: i32 = 400;
/// `DeviceConfigUtils`' forced values of a feature flag.
const FORCE_ENABLE_FEATURE_FLAG_VALUE: i32 = 1;
const FORCE_DISABLE_FEATURE_FLAG_VALUE: i32 = -1;
const SYSTEM_UID: i32 = 1000;
/// `APP_UID_STATS_MAP_PATH` below bpffs.
const APP_UID_STATS_MAP: &str = "netd_shared/map_netd_app_uid_stats_map";

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

/// libnetworkstats' `StatsValue`, as `NetworkStats.Entry` carries it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Entry {
    rx_packets: i64,
    rx_bytes: i64,
    tx_packets: i64,
    tx_bytes: i64,
}

impl Entry {
    /// The map's value: rxPackets, rxBytes, txPackets, txBytes.
    fn of(v: &[u8]) -> Option<Entry> {
        let at =
            |i: usize| Some(u64::from_ne_bytes(v.get(i * 8..i * 8 + 8)?.try_into().ok()?) as i64);
        Some(Entry {
            rx_packets: at(0)?,
            rx_bytes: at(1)?,
            tx_packets: at(2)?,
            tx_bytes: at(3)?,
        })
    }

    /// `NetworkStats.Entry.isEmpty` (no operations either).
    fn is_empty(&self) -> bool {
        *self == Entry::default()
    }
}

/// `StatsResult`, a structured parcelable.
impl WriteParcelable for Entry {
    fn write_to(&self, p: &mut Parcel) {
        let start = p.position();
        p.write_i32(0);
        for v in [
            self.rx_bytes,
            self.rx_packets,
            self.tx_bytes,
            self.tx_packets,
        ] {
            p.write_i64(v);
        }
        let size = (p.position() - start) as i32;
        p.set_i32_at(start, size);
    }
}

/// `TrafficStatsRateLimitCache` (an `LruCacheWithExpiry`) of uids: values
/// younger than the expiry, at most `max` of them, the least recently used
/// evicted first; only non-empty ones are kept.
struct RateLimitCache {
    expiry_ms: i64,
    max: usize,
    /// (uid, when computed, the entry), the most recently used last.
    entries: VecDeque<(i32, i64, Entry)>,
}

impl RateLimitCache {
    fn get_or_compute<E>(
        &mut self,
        uid: i32,
        now: i64,
        compute: impl FnOnce() -> Result<Entry, E>,
    ) -> Result<Entry, E> {
        if let Some(i) = self.entries.iter().position(|e| e.0 == uid) {
            let e = self.entries.remove(i).unwrap();
            if now <= e.1 + self.expiry_ms {
                self.entries.push_back(e);
                return Ok(e.2);
            }
        }
        let entry = compute()?;
        if !entry.is_empty() && self.max > 0 {
            if self.entries.len() == self.max {
                self.entries.pop_front();
            }
            self.entries.push_back((uid, now, entry));
        }
        Ok(entry)
    }
}

pub struct NetworkStats {
    host: Arc<AppHost>,
    /// The service-side cache when enabled, made on the first call (the
    /// original's constructor reads the flags).
    uid_cache: OnceLock<Option<Mutex<RateLimitCache>>>,
    app_uid_stats: OnceLock<Mapped>,
}

impl NetworkStats {
    pub fn new(host: Arc<AppHost>) -> Self {
        NetworkStats {
            host,
            uid_cache: OnceLock::new(),
            app_uid_stats: OnceLock::new(),
        }
    }

    /// `DeviceConfigUtils.getDeviceConfigPropertyInt` in the tethering
    /// namespace.
    fn tethering_int(&self, name: &str, default: i32) -> Result<i32, Exception> {
        let value = self
            .host
            .inputs()?
            .settings
            .get(Table::Config, &format!("tethering/{name}"));
        Ok(value.and_then(|v| v.parse().ok()).unwrap_or(default))
    }

    /// `DeviceConfigUtils.isFeatureEnabled` in the tethering namespace: a
    /// flag other than 0, -1 or 1 compares with the module's version,
    /// which is not modelled.
    fn tethering_feature(&self, name: &str, default: bool) -> Result<bool, Exception> {
        match self.tethering_int(name, 0)? {
            0 => Ok(default),
            FORCE_DISABLE_FEATURE_FLAG_VALUE => Ok(false),
            FORCE_ENABLE_FEATURE_FLAG_VALUE => Ok(true),
            _ => Err(self.host.unimplemented_named(
                "netstats",
                "a feature flag against the tethering module's version (#1671)",
            )),
        }
    }

    /// `getTrafficStatsRateLimitCacheClientSideConfig`; the cache is on by
    /// `isTetheringFeatureEnabled` (off by default).
    fn cache_config(&self) -> Result<CacheConfig, Exception> {
        Ok(CacheConfig {
            enabled: self.tethering_feature(CLIENT_CACHE_ENABLED_FLAG, false)?,
            expiry_duration_ms: self
                .tethering_int(CACHE_EXPIRY_DURATION_NAME, DEFAULT_CACHE_EXPIRY_DURATION_MS)?,
            max_entries: self.tethering_int(CACHE_MAX_ENTRIES_NAME, DEFAULT_CACHE_MAX_ENTRIES)?,
        })
    }

    /// `mTrafficStatsUidCache`: on (`isTrafficStatsServiceRateLimitCacheEnabled`)
    /// unless the client side cache is or its flag chickens it out; on V+
    /// it serves every caller.
    fn uid_cache(&self) -> Result<Option<&Mutex<RateLimitCache>>, Exception> {
        if let Some(c) = self.uid_cache.get() {
            return Ok(c.as_ref());
        }
        let config = self.cache_config()?;
        let enabled =
            !config.enabled && self.tethering_feature(SERVICE_CACHE_ENABLED_FLAG, true)?;
        let cache = enabled.then(|| {
            Mutex::new(RateLimitCache {
                expiry_ms: config.expiry_duration_ms as i64,
                max: config.max_entries.max(0) as usize,
                entries: VecDeque::new(),
            })
        });
        Ok(self.uid_cache.get_or_init(|| cache).as_ref())
    }

    /// `nativeGetUidStat` (libnetworkstats `bpfGetUidStats`): the uid's
    /// counters, zero when it has none.
    fn native_uid_stat(&self, uid: i32) -> Result<Entry, Exception> {
        let map = match self.app_uid_stats.get() {
            Some(m) => m,
            None => match Mapped::open(&self.host.bpf()?.join(APP_UID_STATS_MAP)) {
                Ok(m) => self.app_uid_stats.get_or_init(|| m),
                // The original's BpfMapRO aborts without its map.
                Err(e) => {
                    return Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("netstats: {APP_UID_STATS_MAP}: {e}"),
                    ));
                }
            },
        };
        match map.map().lookup(&(uid as u32).to_ne_bytes()) {
            Some(v) => Entry::of(&v).ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!(
                        "netstats: {APP_UID_STATS_MAP}: a value of {} bytes",
                        v.len()
                    ),
                )
            }),
            None => Ok(Entry::default()),
        }
    }

    /// `getUidStats`: a uid's own traffic, or any uid's for the system.
    fn uid_stats(&self, caller: i32, uid: i32) -> Result<Option<Entry>, Exception> {
        if caller != SYSTEM_UID && caller != uid {
            return Ok(None);
        }
        let Some(cache) = self.uid_cache()? else {
            return self.native_uid_stat(uid).map(Some);
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        cache
            .lock()
            .unwrap()
            .get_or_compute(uid, now, || self.native_uid_stat(uid))
            .map(Some)
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
                    Ok(config) => {
                        ns::write_get_rate_limit_cache_config_reply(&mut p, Some(&config))
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            ns::GET_UID_STATS => {
                let a = ns::GetUidStats::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.uid_stats(call.sender_euid as i32, a.uid) {
                    Ok(entry) => ns::write_get_uid_stats_reply(&mut p, entry.as_ref()),
                    Err(e) => return reply(Err(e)),
                }
            }
            code => {
                let name = ns::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                return reply(Err(self
                    .host
                    .unimplemented_named("netstats", &format!("{name} (#1671)"))));
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
        CacheConfig {
            enabled: false,
            expiry_duration_ms: 1000,
            max_entries: 400,
        }
        .write_to(&mut p);
        let mut expected = Parcel::new();
        for v in [16, 0, 1000, 400] {
            expected.write_i32(v);
        }
        assert_eq!(p.data(), expected.data());
    }

    #[test]
    fn stats_results_parcel_bytes_before_packets() {
        let mut p = Parcel::new();
        Entry {
            rx_packets: 2,
            rx_bytes: 168,
            tx_packets: 3,
            tx_bytes: 200,
        }
        .write_to(&mut p);
        let mut expected = Parcel::new();
        expected.write_i32(36);
        for v in [168, 2, 200, 3] {
            expected.write_i64(v);
        }
        assert_eq!(p.data(), expected.data());
        let value: Vec<u8> = [2u64, 168, 3, 200]
            .iter()
            .flat_map(|v| v.to_ne_bytes())
            .collect();
        assert_eq!(
            Entry::of(&value),
            Some(Entry {
                rx_packets: 2,
                rx_bytes: 168,
                tx_packets: 3,
                tx_bytes: 200
            })
        );
        assert_eq!(Entry::of(&value[..24]), None);
    }

    /// Like LruCacheWithExpiry: fresh values are served, expired ones
    /// recomputed, empty ones never kept, the least recently used evicted.
    #[test]
    fn the_rate_limit_cache_expires_and_evicts() {
        let mut c = RateLimitCache {
            expiry_ms: 1000,
            max: 2,
            entries: VecDeque::new(),
        };
        let e = |n| Entry {
            rx_bytes: n,
            ..Entry::default()
        };
        let get = |c: &mut RateLimitCache, uid, now, v| {
            c.get_or_compute::<()>(uid, now, || Ok(v)).unwrap()
        };
        assert_eq!(get(&mut c, 1, 0, e(1)), e(1));
        assert_eq!(get(&mut c, 1, 1000, e(2)), e(1));
        assert_eq!(get(&mut c, 1, 1001, e(3)), e(3));
        assert_eq!(get(&mut c, 2, 1001, Entry::default()), Entry::default());
        assert_eq!(get(&mut c, 2, 1001, e(4)), e(4));
        // 1 was used last, so 2 goes for 3.
        assert_eq!(get(&mut c, 1, 1002, e(9)), e(3));
        assert_eq!(get(&mut c, 3, 1002, e(5)), e(5));
        assert_eq!(get(&mut c, 2, 1002, e(6)), e(6));
        assert_eq!(get(&mut c, 3, 1002, e(9)), e(5));
        assert_eq!(
            c.get_or_compute(4, 1002, || Err("unreadable")),
            Err("unreadable")
        );
    }
}
