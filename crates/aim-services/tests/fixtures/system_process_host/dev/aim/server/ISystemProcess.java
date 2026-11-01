package dev.aim.server;
import android.os.IBinder;
interface ISystemProcess {
    Object getSyncAccounts();
    long getSyncOwnerGeneration();
    void attachSyncOwner(long generation, IBinder owner);
    boolean isSyncOwnerAttached(long generation);
    IBinder getInternalServiceRepository();
    IBinder getPackageInternalHost();
    void attachRoleService(IBinder service);
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
    void attachPackageShellRead(AppHostPackageShellRead owner);
    void attachStatsCompanionUidMap(StatsCompanionUidMap companion);
    void attachStorageCrates(StorageCrates crates);
    void attachActivityAnimations(ActivityAnimations animations);
    void attachPackageIntentParser(PackageIntentParser parser);
    void attachUserSwitcherPolicy(UserSwitcherPolicy policy);
    void initializeCameraPrivacyAllowlist(java.util.List<String> packages);
    void attachAudioPolicyQueries(AudioPolicyQueries queries);
    void attachSyncAdapterQueries(SyncAdapterQueries queries);
    void accessibilityReady(IBinder service) throws android.os.RemoteException;
    Object getDexUsePackages();
    void attachDexUseBridge(com.android.server.art.AppHostDexUse bridge);
    abstract class Stub {
        public static ISystemProcess asInterface(IBinder binder) {
            return new ISystemProcess() {
                public Object getSyncAccounts() { return this; }
                public long getSyncOwnerGeneration() { return 7; }
                public void attachSyncOwner(long generation, IBinder owner) {
                    if (generation != 7 || owner == null) throw new AssertionError();
                    try { owner.transact(4, null, null, 0); throw new AssertionError("sync forwarded"); }
                    catch (IllegalStateException expected) {
                        if (!expected.getMessage().contains("forwarding inactive")) throw expected;
                    } catch (android.os.RemoteException e) { throw new AssertionError(e); }
                    SystemProcessHostTest.events.add("syncAttached");
                }
                public boolean isSyncOwnerAttached(long generation) { return generation == 7; }
                public IBinder getInternalServiceRepository() { throw new AssertionError(); }
                public IBinder getPackageInternalHost() { throw new AssertionError(); }
                public void attachRoleService(IBinder service) {
                    if (service != android.os.ServiceManager.getService("role") || service == null)
                        throw new AssertionError("wrong role owner");
                    SystemProcessHostTest.events.add("attachRole");
                }
                public void attachDomainVerification(IBinder frontend) {
                    if (frontend == null) throw new AssertionError("missing domain front end");
                    SystemProcessHostTest.events.add("attachDomain");
                }
                public void accessibilityReady(IBinder service) { SystemProcessHostTest.events.add("accessibilityReady"); }
                public Object getDexUsePackages() { return this; }
                public void attachDexUseBridge(com.android.server.art.AppHostDexUse bridge) { SystemProcessHostTest.events.add("dex"); }
                public void attachWifiBridge(WifiBridge bridge) {}
                public void attachSettingsValues(SettingsValues values) {
                    SystemProcessHostTest.events.add("settings");
                }
                public void attachPackageShellRead(AppHostPackageShellRead owner) { if(owner==null)throw new AssertionError("missing package shell read owner"); }
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
                public void attachActivityAnimations(ActivityAnimations animations) {
                    if (animations == null) throw new AssertionError("missing activity animation owner");
                }
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
