package dev.aim.server;

/** Exercise the production publisher against an actual native reservation.
 * The original ART runner checks its bounded log interval for BinderProxy
 * FLAG_ONEWAY warnings; no reflection or process-wide blocking override. */
public final class NativeMutationCapabilitiesOracle {
    private NativeMutationCapabilitiesOracle() {}
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
