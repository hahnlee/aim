package dev.aim.server;

/** Exercise the production publisher against an actual native reservation.
 * The original ART runner checks its bounded log interval for BinderProxy
 * FLAG_ONEWAY warnings; no reflection or process-wide blocking override. */
public final class NativeMutationCapabilitiesOracle {
    private NativeMutationCapabilitiesOracle() {}
    public static void main(String[] args) {
        try {
            // Match SystemServer's diagnostic policy before creating any proxies.
            android.os.Binder.setWarnOnBlocking(true);
            android.util.Log.i("NativeMutationOracle", "BEGIN " + args[0]);
            var host = IServiceHost.Stub.asInterface(android.os.Binder.allowBlocking(
                    java.util.Objects.requireNonNull(android.os.ServiceManager.checkService("aim.service_host"))));
            var internal = IPackageInternalHost.Stub.asInterface(android.os.Binder.allowBlocking(
                    java.util.Objects.requireNonNull(host.getPackageInternalHost())));
            var owner = new PackageSnapshots.Owner() {
                public String getFilteredPackageName(long version, String name, int uid, int user) {
                    throw new AssertionError("unexpected filtered mutation scope");
                }
                public boolean shouldFilter(long version, com.android.server.pm.pkg.PackageState state, int uid, int user) {
                    throw new AssertionError("unexpected filtered mutation scope");
                }
            };
            try (var store = new PackageSnapshots.Store(host::capturePackageScan, owner, true)) {
                exercise(new NativePackageMutationPublisher(internal, owner, store, true));
            }
            android.util.Log.i("NativeMutationOracle", "END " + args[0]);
            System.out.println("NATIVE_MUTATION_CAPABILITIES reservation snapshot sequence scopes installed close pid="
                    + android.os.Process.myPid());
            System.exit(0);
        } catch (Throwable failure) {
            failure.printStackTrace(System.out);
            System.exit(1);
        }
    }
    public static void exercise(NativePackageMutationPublisher publisher) {
        NativePackageStateMutation.Reservation reservation = publisher.reserve();
        PackageSnapshots.ComputerSnapshot snapshot = reservation.snapshot();
        long version = snapshot.getVersion();
        if (version <= 0) throw new AssertionError("reservation snapshot has no native version");
        int sequence = reservation.changedPackagesSequence();
        if (reservation.changedPackagesSequence() != sequence)
            throw new AssertionError("retained reservation changed its captured sequence");
        if (snapshot.getPackageStates() == null || snapshot.getDisabledSystemPackageStates() == null)
            throw new AssertionError("native reservation lacks actual package scopes");
        if (snapshot.getInstalledApplications(0, 0, 1000, false).isEmpty())
            throw new AssertionError("native reservation lacks installed applications");
        if (snapshot.getVersion() != version || reservation.changedPackagesSequence() != sequence)
            throw new AssertionError("retained reservation changed after reads");
        reservation.close();
        reservation.close();
        try {
            reservation.snapshot();
            throw new AssertionError("closed reservation remained readable");
        } catch (IllegalStateException expected) {
            // Exact production owner lifetime, not a synthetic Binder response.
        }
    }
}
