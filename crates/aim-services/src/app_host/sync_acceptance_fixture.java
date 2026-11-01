/* Android API boundaries for the retained scheduling and reconnect bodies. */
    static class Pair<A,B> { A first; B second; Pair(A a, B b) { first=a; second=b; } }
    static final int TEMPORARY_ALLOWLIST_TYPE_FOREGROUND_SERVICE_NOT_ALLOWED=1, REASON_SYNC_MANAGER=306;
    static class Process { static final int SYSTEM_UID=1000; }
    static abstract class BroadcastReceiver { abstract void onReceive(Context c, Intent i); }
    static class EndPoint { static final Object USER_ALL_PROVIDER_ALL_ACCOUNTS_ALL=new Object(); }
    static class NetworkInfo { boolean isConnected() { return true; } }
    static class ConnectivityManager { NetworkInfo getActiveNetworkInfo() { return request("network-read")!=0 ? new NetworkInfo() : null; } }
    ConnectivityManager getConnectivityManager() { return new ConnectivityManager(); }
    boolean mDataConnectionIsConnected;
    final List<SyncOperation> scheduled=new ArrayList<>(); int nextJob=42, submissions, reschedules; JobInfo last;
    List<SyncOperation> getAllPendingSyncs() {
        for (SyncOperation op:scheduled) require(request("pending " + op.jobId)==1);
        return new ArrayList<>(scheduled);
    }
    int getUnusedJobIdH() { return nextJob++; }
    SyncStorageEngine getSyncStorageEngine() { return mSyncStorageEngine; }
    void cancelJob(SyncOperation op, String why) { request("cancel-job " + op.jobId); require(scheduled.remove(op)); }
    void rescheduleSyncs(Object endpoint, String why) {
        require(endpoint==EndPoint.USER_ALL_PROVIDER_ALL_ACCOUNTS_ALL && why.equals("network reconnect"));
        reschedules++;
        for (SyncOperation op:getAllPendingSyncs()) scheduleSyncOperationH(op);
    }
    void reportStart(SyncOperation op) {
@EXEMPTED_START@
    }
    static class UsageStatsManagerInternal {
        void reportExemptedSyncStart(String p, int u) { require(p.equals("p") && u==0); request("exempted-start"); }
        void reportSyncScheduled(String p, int u, boolean e) { require(p.equals("p") && u==0); request("scheduled " + (e?1:0)); }
    }
    static class DeviceIdleInternal {
        void addPowerSaveTempWhitelistApp(int caller, String p, long d, int type, int user, boolean sync, int reason, String text) {
            require(caller==1000 && p.equals("p") && d==30000 && user==0 && !sync && reason==306 && text.equals("sync by top app"));
            if (request("allowlist " + type)<0) throw new UnsupportedOperationException("#2016");
        }
    }
    static class LocalServices {
        static <T> T getService(Class<T> k) { return k.cast(k==DeviceIdleInternal.class ? new DeviceIdleInternal() : new UsageStatsManagerInternal()); }
    }
    static class JobInfo {
        static final int NETWORK_TYPE_ANY=1, NETWORK_TYPE_UNMETERED=2, FLAG_EXEMPT_FROM_APP_STANDBY=8;
        int id, network, flags, bias; long latency, period, flex; boolean storage, persisted, charging, expedited; SyncOperation op;
        boolean isExpedited() { return expedited; }
        static class Builder {
            JobInfo j=new JobInfo(); Builder(int id, ComponentName c) { j.id=id; }
            Builder setExtras(SyncOperation o) { j.op=o; return this; }
            Builder setRequiredNetworkType(int n) { j.network=n; return this; }
            Builder setRequiresStorageNotLow(boolean b) { j.storage=b; return this; }
            Builder setPersisted(boolean b) { j.persisted=b; return this; }
            Builder setBias(int b) { j.bias=b; return this; } Builder setFlags(int f) { j.flags=f; return this; }
            Builder setMinimumLatency(long l) { j.latency=l; return this; }
            Builder setPeriodic(long p, long f) { j.period=p; j.flex=f; return this; }
            Builder setRequiresCharging(boolean b) { j.charging=b; return this; }
            Builder setExpedited(boolean b) { j.expedited=b; return this; }
            JobInfo build() { return j; }
        }
    }
    static class JobScheduler { static final int RESULT_FAILURE=0; }
    class Scheduler {
        int scheduleAsPackage(JobInfo j, String p, int user, String tag) {
            require(p.equals("p") && user==0 && tag.equals(j.op.key) && j.storage && j.persisted && j.bias==0);
            int result=request("job " + j.id + " " + j.latency + " " + j.flags + " " + (j.expedited?1:0) + " " + (j.charging?1:0) + " " + j.network + " " + j.period + " " + j.flex + " " + j.op.syncExemptionFlag + " " + j.op.expectedRuntime + " " + (j.op.scheduleEjAsRegularJob?1:0) + " " + j.op.key);
            require(result==1); last=j; submissions++; scheduled.remove(j.op); scheduled.add(j.op); return result;
        }
    }
    Scheduler getJobScheduler() { return new Scheduler(); }
@SCHEDULE@
@RECONNECT@
    SyncOperation operation(String key, int exemption) { SyncOperation o=new SyncOperation(key); o.syncExemptionFlag=exemption; return o; }
    void acceptance() {
        require(request("bucket")==50);
        SyncOperation plain=operation("plain",0);
        mSyncHandler.dispatch(() -> scheduleSyncOperationH(plain)); require(request("bucket")==20);
        require(mSyncStorageEngine.pending); cancelJob(plain,"fixture");
        SyncOperation top=operation("a",2); top.expedited=true;
        mSyncStorageEngine.backoff=SystemClock.elapsedRealtime()+60000;
        mSyncStorageEngine.delayUntil=SystemClock.elapsedRealtime()+120000;
        mSyncHandler.dispatch(() -> scheduleSyncOperationH(top));
        require(last.latency>90000 && !last.expedited && last.flags==8);
        require(top.scheduleEjAsRegularJob && request("bucket")==10 && request("membership")==1);
        require(submissions==2);
        // Real broadcasts trigger the original receiver, which rereads the native query.
        require(request("network 2")==1); mConnectivityIntentReceiver.onReceive(mContext,new Intent());
        require(mDataConnectionIsConnected && mSyncStorageEngine.clears==1 && reschedules==1 && submissions==3);
        // Replacement sends loss/connected payloads while both active reads stay online.
        require(request("network 3")==2);
        mConnectivityIntentReceiver.onReceive(mContext,new Intent()); mConnectivityIntentReceiver.onReceive(mContext,new Intent());
        require(mSyncStorageEngine.clears==1 && submissions==3);
        require(request("network 3")==0); // Duplicate snapshot is silent.
        mSyncStorageEngine.backoff=SystemClock.elapsedRealtime()+60000;
        require(request("network 0")==1); mConnectivityIntentReceiver.onReceive(mContext,new Intent());
        require(!mDataConnectionIsConnected && mSyncStorageEngine.backoff!=-1);
        mSyncStorageEngine.delayUntil=0;
        require(request("network 4")==1); mConnectivityIntentReceiver.onReceive(mContext,new Intent());
        require(mSyncStorageEngine.backoff==-1 && reschedules==2 && submissions==4);
        // Pending duplicates and active duplicates must neither resubmit nor re-promote.
        mSyncHandler.dispatch(() -> scheduleSyncOperationH(top)); require(submissions==5);
        SyncOperation later=operation("a",0);
        mSyncHandler.dispatch(() -> scheduleSyncOperationH(later,60000)); require(submissions==5);
        reportStart(top);
        ActiveSyncContext active=open("a",10100,true);
        mSyncHandler.dispatch(() -> scheduleSyncOperationH(top)); require(submissions==5);
        active.onFinished(new SyncResult()); idle(); cancelJob(top,"fixture");
        // Immediate EJ and periodic constraints reach the native persisted job owner too.
        SyncOperation ej=operation("ej",1); ej.expedited=true;
        mSyncHandler.dispatch(() -> scheduleSyncOperationH(ej)); require(!ej.scheduleEjAsRegularJob && last.expedited && last.latency==0); cancelJob(ej,"fixture");
        SyncOperation periodic=operation("periodic",0); periodic.isPeriodic=true; periodic.charging=true; periodic.unmetered=true;
        mSyncHandler.dispatch(() -> scheduleSyncOperationH(periodic)); require(last.period==3600000 && last.flex==900000 && last.charging && last.network==2); cancelJob(periodic,"fixture");
        // Owner exceptions escape the handler but its original finally releases the loop lock.
        try { mSyncHandler.dispatch(() -> new DeviceIdleInternal().addPowerSaveTempWhitelistApp(1000,"p",30000,2,0,false,306,"sync by top app")); throw new AssertionError(); }
        catch (UnsupportedOperationException expected) { require(expected.getMessage().equals("#2016")); } idle();
        request("unsupported"); request("persist");
    }
