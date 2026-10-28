package dev.aim.server;
import android.os.IBinder;
interface ISystemProcess {
    IBinder getInternalServiceRepository();
    void attachDomainVerification(IBinder frontend);
    void attachSystemProcess(IBinder thread, IForegroundServicePolicy foreground, SafeLabels labels, BatteryHealthStats battery);
    void attachNetworkActivityBridge(IBinder bridge);
    void attachWifiBridge(WifiBridge bridge);
    void attachDevicePolicyDefaults(DevicePolicyDefaults defaults);
    void attachSettingsValues(SettingsValues values);
    void attachPackageBootstrapJava(IBinder bridge);
    void attachRecoverableKeyStoreKeys(RecoverableKeyStoreKeys keys);
    void attachSystemRouteQueries(com.android.server.media.SystemRouteQueries queries);
    void attachProviderQueries(ProviderQueries queries);
    void attachSettingsValues(SettingsValues values);
    void attachStatsCompanionUidMap(StatsCompanionUidMap companion);
    void attachStorageCrates(StorageCrates crates);
    abstract class Stub {
        public static ISystemProcess asInterface(IBinder binder) {
            return new ISystemProcess() {
                public IBinder getInternalServiceRepository() { throw new AssertionError(); }
                public void attachDomainVerification(IBinder frontend) {
                    if (frontend == null) throw new AssertionError("missing domain front end");
                    SystemProcessHostTest.events.add("attachDomain");
                }
                public void attachWifiBridge(WifiBridge bridge) {}
                public void attachSettingsValues(SettingsValues values) {}
                public void attachProviderQueries(ProviderQueries queries) {
                    ProviderQueriesTest.check(queries);
                }
                public void attachDevicePolicyDefaults(DevicePolicyDefaults defaults) {}
                public void attachSettingsValues(SettingsValues values) {}
                public void attachPackageBootstrapJava(IBinder bridge) {
                    if (!(bridge instanceof PackageBootstrapBridge)) throw new AssertionError("missing package Java owner");
                    SystemProcessHostTest.events.add("packageJava");
                }
                public void attachRecoverableKeyStoreKeys(RecoverableKeyStoreKeys keys) {}
                public void attachStorageCrates(StorageCrates crates) {
                    if (crates == null) throw new AssertionError("missing storage crates");
                    SystemProcessHostTest.events.add("crates");
                }
                public void attachSystemRouteQueries(com.android.server.media.SystemRouteQueries queries) {}
                public void attachNetworkActivityBridge(IBinder bridge) {
                    SystemProcessHostTest.events.add("network");
                }
                public void attachStatsCompanionUidMap(StatsCompanionUidMap companion) {
                    SystemProcessHostTest.events.add("companion");
                }
                public void attachSystemProcess(IBinder thread, IForegroundServicePolicy policy, SafeLabels labels, BatteryHealthStats battery) {
                    if (thread != SystemProcessHostTest.applicationThread
                            || policy != SystemProcessHostTest.foregroundPolicy) {
                        throw new AssertionError("lost application thread or foreground policy");
                    }
                    SystemProcessHostTest.events.add("attachSystem");
                }
            };
        }
    }
}
