/* Test boundaries for unchanged Apache-2.0 AOSP android-16.0.0_r1 SyncManager bodies. */
import java.io.*;
import java.util.*;
public class Cleanup {
    static final String TAG = "fixture", SYNC_WAKE_LOCK_PREFIX = "*sync*/";
    static final int SYNC_ADAPTER_CONNECTION_FLAGS = 0;
    static final BufferedReader input = new BufferedReader(new InputStreamReader(System.in));
    static int request(String s) {
        System.out.println(s); System.out.flush();
        try { return Integer.parseInt(input.readLine()); } catch (Exception e) { throw new AssertionError(e); }
    }
    static void require(boolean b) { if (!b) throw new AssertionError(); }
    static class RemoteException extends Exception {}
    interface IBinder {
        interface DeathRecipient { void binderDied(); }
        void linkToDeath(DeathRecipient r, int flags) throws RemoteException;
        boolean unlinkToDeath(DeathRecipient r, int flags);
    }
    interface ServiceConnection { void onServiceConnected(ComponentName n, IBinder b); void onServiceDisconnected(ComponentName n); }
    static class ISyncContext { static class Stub {} }
    static class ISyncAdapter {
        static class Stub { static ISyncAdapter asInterface(IBinder b) { return (ISyncAdapter)b; } }
        IBinder asBinder() { return (IBinder)this; }
        void startSync(ActiveSyncContext c, String p, Object a, Bundle b) throws RemoteException {}
        void cancelSync(ActiveSyncContext c) throws RemoteException {}
    }
    static class Adapter extends ISyncAdapter implements IBinder {
        DeathRecipient death; boolean failStart, failStop, dead; int cancels;
        public void linkToDeath(DeathRecipient r, int f) { death = r; }
        public boolean unlinkToDeath(DeathRecipient r, int f) { require(death == r); death = null; return true; }
        void startSync(ActiveSyncContext c, String p, Object a, Bundle b) throws RemoteException { if (failStart) throw new RemoteException(); }
        void cancelSync(ActiveSyncContext c) throws RemoteException { cancels++; if (failStop || dead) throw new RemoteException(); }
    }
    static class Log {
        static final int VERBOSE = 2;
        static boolean isLoggable(String s, int n) { return false; }
        static void d(Object... o) {} static void w(Object... o) {}
        static String getStackTraceString(Exception e) { return e.toString(); }
    }
    static class Slog { static void v(Object... o) {} static void wtf(Object... o) {} static void e(Object... o) {} }
    static class Logger { void log(Object... o) {} boolean enabled() { return false; } }
    static class SystemClock { static long elapsedRealtime() { return System.nanoTime()/1000000; } }
    static class Bundle {} static class UserHandle { UserHandle(int u) {} }
    static class ComponentName {} static class Intent {} static class SyncInfo {}
    static class SyncStorageEngine {
        static final String MESG_SUCCESS = "success", MESG_CANCELED = "cancel";
        static class EndPoint { int userId; String provider = "authority"; Object account = "account"; }
        Set<SyncInfo> active = new HashSet<>();
        void removeActiveSync(SyncInfo i, int u) { require(active.remove(i)); }
    }
    static class SyncOperation {
        SyncStorageEngine.EndPoint target = new SyncStorageEngine.EndPoint();
        int jobId, reason, retries, syncExemptionFlag, owningUid, syncSource; String owningPackage; boolean isPeriodic, allowParallelSyncs;
        final String key;
        SyncOperation(String k) { key = k; }
        SyncOperation(Object a, int u, int uid, String p, int r, int s, String provider, Bundle b, boolean parallel, int exemption) { throw new AssertionError(); }
        String wakeLockName() { return key; } Object getExtrasAsString() { return ""; }
        Bundle getClonedExtras() { return new Bundle(); }
        boolean isDerivedFromFailedPeriodicSync() { return false; }
        SyncOperation createOneTimeSyncOperation() { throw new AssertionError(); }
        static Object reasonToString(Object x, int r) { return ""; }
    }
    static class SyncResult {
        boolean tooManyDeletions, fullSyncRequested; long delayUntil;
        static class Stats { long numDeletes, numIoExceptions; } Stats stats = new Stats();
        boolean hasError() { return stats.numIoExceptions != 0; }
    }
    static class ContentResolver { static final int SYNC_EXEMPTION_NONE = 0; static String syncErrorToString(int n) { return "io"; } }
    static class SyncJobService { static int finishes; static void callJobFinished(Object... o) { finishes++; } }
    static class SystemMessage { static final int NOTE_SYNC_ERROR = 1; }
    static class Notifications { void cancelAsUser(String s, int id, UserHandle u) {} }
    static class Constants { int getMaxRetriesWithAppStandbyExemption() { return 3; } }
    static class WorkSource { int uid; WorkSource(int u) { uid = u; } }
    static class PowerManager {
        static final int PARTIAL_WAKE_LOCK = 1; int next;
        class WakeLock {
            int id = next++; String name; WorkSource ws; boolean held;
            WakeLock(String n) { name = n; }
            void setReferenceCounted(boolean b) { require(!b); }
            void setWorkSource(WorkSource w) { ws = w; if (held) request("source " + id + " " + (w == null ? -1 : w.uid)); }
            void acquire() { request("acquire " + id + " " + name + " " + (ws == null ? -1 : ws.uid)); held = true; }
            void release() { if (held) { request("release " + id); held = false; } }
        }
        WakeLock newWakeLock(int level, String name) { require(level == PARTIAL_WAKE_LOCK); return new WakeLock(name); }
    }
    static class Context {
        boolean bindResult = true, failUnbind;
        Map<ActiveSyncContext,Integer> connections = new IdentityHashMap<>(); int next;
        boolean bindServiceAsUser(Intent i, ActiveSyncContext c, int flags, UserHandle u) { int id=next++; boolean bound=request("bind " + id + " " + (bindResult ? 1 : 0)) != 0; if (bound) connections.put(c,id); return bound; }
        void unbindService(ActiveSyncContext c) { request("unbind " + connections.remove(c)); if (failUnbind) throw new NoSuchElementException(); }
    }
    static Intent getAdapterBindIntent(Context c, ComponentName n, int u) { return new Intent(); }
    static class Battery {
        void noteSyncStart(String n, int u) throws RemoteException { request("start " + n + " " + u); }
        void noteSyncFinish(String n, int u) throws RemoteException { request("finish " + n + " " + u); }
    }
    static class Message { int what; Object obj; Runnable body; }
    class ServiceConnectionData { ActiveSyncContext activeSyncContext; IBinder adapter; ServiceConnectionData(ActiveSyncContext c, IBinder b) { activeSyncContext=c; adapter=b; } }
    final PowerManager mPowerManager = new PowerManager();
    final PowerManager.WakeLock mSyncManagerWakeLock = mPowerManager.newWakeLock(1, "*SyncLoop*");
    final Context mContext = new Context(); final Logger mLogger = new Logger(); final Battery mBatteryStats = new Battery();
    final SyncStorageEngine mSyncStorageEngine = new SyncStorageEngine(); final List<ActiveSyncContext> mActiveSyncContexts = new ArrayList<>();
    final Notifications mNotificationMgr = new Notifications(); final Constants mConstants = new Constants();
    final SyncHandler mSyncHandler = new SyncHandler();
    String logSafe(Object o) { return "safe"; }
    void sendSyncFinishedOrCanceledMessage(ActiveSyncContext c, SyncResult result) { mSyncHandler.queue.add(() -> { if (isSyncStillActiveH(c)) mSyncHandler.runSyncFinishedOrCanceledH(result, c); }); }
@ACTIVE@
@STILL@
    class SyncHandler {
        static final int MESSAGE_SERVICE_CONNECTED = 4, MESSAGE_SERVICE_DISCONNECTED = 5, MESSAGE_MONITOR_SYNC = 8;
        HashMap<String, PowerManager.WakeLock> mWakeLocks = new HashMap<>();
        ArrayDeque<Runnable> queue = new ArrayDeque<>(); Set<ActiveSyncContext> monitors = new HashSet<>();
        Message obtainMessage() { return new Message(); }
        void sendMessage(Message m) { queue.add(() -> handleServiceMessage(m)); }
        void handleServiceMessage(Message msg) {
            boolean isLoggable=false;
            switch (msg.what) {
                case MESSAGE_SERVICE_CONNECTED: ServiceConnectionData d=(ServiceConnectionData)msg.obj; if (isSyncStillActiveH(d.activeSyncContext)) runBoundToAdapterH(d.activeSyncContext,d.adapter); break;
@DISCONNECT@
                default: throw new AssertionError();
            }
        }
        void removeMessages(int what, ActiveSyncContext c) { require(what == MESSAGE_MONITOR_SYNC); monitors.remove(c); }
        void handleSyncMessage(Message m) { m.body.run(); }
        void dispatch(Runnable r) { Message m = new Message(); m.body=r; handleMessage(m); }
        void drain() { while (!queue.isEmpty()) dispatch(queue.remove()); }
@HANDLE@
@LOCK@
@BOUND@
@FINISH@
@CLOSE@
        void increaseBackoffSetting(Object o) {} void scheduleSyncOperationH(SyncOperation o) {}
        void cancelJob(Object... o) {} void clearBackoffSetting(Object... o) {} void setDelayUntilTime(Object... o) {}
        void stopSyncEvent(Object... o) {} void reschedulePeriodicSyncH(Object... o) { throw new AssertionError(); }
        int reschedules; void maybeRescheduleSync(Object... o) { reschedules++; } void postScheduleSyncMessage(Object... o) { throw new AssertionError(); }
        int syncResultToErrorNumber(Object o) { return 8; }
        void installHandleTooManyDeletesNotification(Object... o) { throw new AssertionError(); }
    }
    ActiveSyncContext open(String key, int uid, boolean bind) {
        ActiveSyncContext c = new ActiveSyncContext(new SyncOperation(key), 1, uid);
        c.mSyncInfo = new SyncInfo(); mSyncStorageEngine.active.add(c.mSyncInfo); mActiveSyncContexts.add(c); mSyncHandler.monitors.add(c);
        mContext.bindResult=bind;
        if (!c.bindToSyncAdapter(new ComponentName(),0)) mSyncHandler.closeActiveSyncContext(c);
        return c;
    }
    void idle() { mSyncHandler.drain(); require(mActiveSyncContexts.isEmpty() && mSyncStorageEngine.active.isEmpty() && mSyncHandler.monitors.isEmpty() && mContext.connections.isEmpty() && !mSyncManagerWakeLock.held); for (PowerManager.WakeLock w : mSyncHandler.mWakeLocks.values()) require(!w.held && w.ws == null); request("idle"); }
    void run() {
        mSyncHandler.dispatch(() -> open("a",10100,false)); idle(); // Failed bind must stop the impending lock without sync notes.
        for (int mode=0; mode<8; mode++) {
            final int scenario=mode;
            ActiveSyncContext c=open("a",10100,true); Adapter a=new Adapter(); a.failStart=scenario==4; a.failStop=scenario==1;
            c.onServiceConnected(new ComponentName(),a); mSyncHandler.drain();
            if (scenario==4) { idle(); continue; }
            mSyncHandler.dispatch(() -> {
                request("check 2 10100 *sync*/a 1");
                if (scenario==5) { c.mSyncWakeLock.setWorkSource(null); request("check 2 10100 *sync*/a 0"); request("check 2 1000 *sync*/a 1"); }
                if (scenario==6) { ActiveSyncContext other=open("b",10101,true); request("bindings 2"); other.onFinished(new SyncResult()); mSyncHandler.queue.add(() -> request("bindings 1")); }
                if (scenario==2) { a.dead=true; require(a.death!=null); a.death.binderDied(); }
                else if (scenario==3) { request("process-death"); a.dead=true; c.onServiceDisconnected(new ComponentName()); }
                else if (scenario==0 || scenario==5 || scenario==6) c.onFinished(new SyncResult());
                else { mContext.failUnbind=scenario==1; sendSyncFinishedOrCanceledMessage(c,null); }
                c.onFinished(new SyncResult());
                c.onServiceDisconnected(new ComponentName());
            });
            mSyncHandler.drain(); mContext.failUnbind=false;
            require(!c.mIsLinkedToDeath && a.death==null);
            if (scenario==1 || scenario==2 || scenario==3 || scenario==7) require(a.cancels==1);
            if (scenario==3) require(SyncJobService.finishes==1 && mSyncHandler.reschedules==1);
            idle();
        }
        try { mSyncHandler.dispatch(() -> { throw new IllegalStateException("handler failure"); }); throw new AssertionError(); } catch (IllegalStateException expected) {} idle();
        // Death of the retained process itself removes all its native wake tokens.
        mSyncManagerWakeLock.acquire(); mSyncHandler.getSyncWakeLock(new SyncOperation("a")).acquire(); request("owner-death"); request("owner-death"); request("idle");
    }
    public static void main(String[] args) { new Cleanup().run(); }
}
