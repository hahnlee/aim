package dev.aim.server;
import dev.aim.server.IPackageScanSnapshot;
/** One native mutation reservation retains its captured graph until publish/close. */
interface IPackageMutationReservation {
    IPackageScanSnapshot getSnapshot();
    int getChangedPackagesSequence();
    int[] getDisabledUserAliases(String packageName);
    void publish(long expectedVersion, in byte[] mutationRecord);
    void close();
}
