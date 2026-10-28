// Host-only Android boundary double; production policy is compiled unchanged.
package dev.aim.server;
public interface IStatsCompanionUidMap {
    void start(android.os.IBinder binder) throws android.os.RemoteException;
    abstract class Stub implements IStatsCompanionUidMap {}
}
