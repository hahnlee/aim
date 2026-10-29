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
    void attachNativeStagingBridge(com.android.server.pm.NativeStagingBridge bridge);
    void attachRecoverableKeyStoreKeys(RecoverableKeyStoreKeys keys);
    void attachSystemRouteQueries(com.android.server.media.SystemRouteQueries queries);
    void attachProviderQueries(ProviderQueries queries);
    void attachStatsCompanionUidMap(StatsCompanionUidMap companion);
    void attachStorageCrates(StorageCrates crates);
    void attachPackageIntentParser(PackageIntentParser parser);
    void attachUserSwitcherPolicy(UserSwitcherPolicy policy);
    void initializeCameraPrivacyAllowlist(java.util.List<String> packages);
    void attachAudioPolicyQueries(AudioPolicyQueries queries);
    void attachSyncAdapterQueries(SyncAdapterQueries queries);
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
                public void attachPackageBootstrapJava(IBinder bridge) {
                    if (!(bridge instanceof PackageBootstrapBridge)) throw new AssertionError("missing package Java owner");
                    SystemProcessHostTest.events.add("packageJava");
                }
                public void attachNativeStagingBridge(com.android.server.pm.NativeStagingBridge bridge) {
                    if (bridge == null) throw new AssertionError("missing staging owner");
                    SystemProcessHostTest.events.add("staging");
                }
                public void attachRecoverableKeyStoreKeys(RecoverableKeyStoreKeys keys) {}
                public void attachPackageIntentParser(PackageIntentParser parser) {
                    if (parser == null) throw new AssertionError("missing package Intent parser");
                }
                public void attachUserSwitcherPolicy(UserSwitcherPolicy policy) {
                    if (policy == null) throw new AssertionError("missing user switcher owner");
                }
                public void initializeCameraPrivacyAllowlist(java.util.List<String> packages) {
                    if (packages == null) throw new AssertionError("missing camera privacy allowlist");
                }
                public void attachAudioPolicyQueries(AudioPolicyQueries queries) {
                    if (queries == null) throw new AssertionError("missing audio policy owner");
                }
                public void attachSyncAdapterQueries(SyncAdapterQueries queries) {
                    if (queries == null) throw new AssertionError("missing sync adapter owner");
                }
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
