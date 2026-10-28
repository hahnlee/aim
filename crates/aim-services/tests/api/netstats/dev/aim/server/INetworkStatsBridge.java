// Host JVM test double; production links the retained image classes.
package dev.aim.server;
public interface INetworkStatsBridge {
 android.os.IBinder openSessionForUsageStats(int flags,String pkg);
 void incrementOperationCount(int uid,int tag,int count);
 abstract class Stub extends android.os.Binder implements INetworkStatsBridge {}
}
