package android.app.job;

import android.content.ComponentName;
import android.os.Parcel;
import android.os.Parcelable;

public final class JobInfo implements Parcelable {
    public static final int NETWORK_TYPE_NONE = 0;
    public static final int NETWORK_TYPE_ANY = 1;
    public static final int NETWORK_TYPE_UNMETERED = 2;
    public static final int BACKOFF_POLICY_LINEAR = 0;
    public static final int BACKOFF_POLICY_EXPONENTIAL = 1;
    public static final long MAX_BACKOFF_DELAY_MILLIS = 18 * 60 * 60 * 1000L;
    public static final Parcelable.Creator<JobInfo> CREATOR =
            new Parcelable.Creator<JobInfo>() {
                public JobInfo createFromParcel(Parcel source) { return null; }
                public JobInfo[] newArray(int size) { return new JobInfo[size]; }
            };
    private final ComponentName service;
    private final int id;
    private final int networkType;
    private final boolean periodic;
    private final boolean persisted;
    private final boolean expedited;
    private final boolean userInitiated;
    private final boolean requireCharging;
    private final boolean requireDeviceIdle;
    private final boolean requireBatteryNotLow;
    private final boolean requireStorageNotLow;
    private final long minLatencyMillis;
    private final long maxExecutionDelayMillis;
    private final long initialBackoffMillis;
    private final int backoffPolicy;
    public JobInfo(ComponentName value) { this(new Builder(value, 1)); }
    private JobInfo(Builder value) {
        service = value.service;
        id = value.id;
        networkType = value.networkType;
        periodic = value.periodic;
        persisted = value.persisted;
        expedited = value.expedited;
        userInitiated = value.userInitiated;
        requireCharging = value.requireCharging;
        requireDeviceIdle = value.requireDeviceIdle;
        requireBatteryNotLow = value.requireBatteryNotLow;
        requireStorageNotLow = value.requireStorageNotLow;
        minLatencyMillis = value.minLatencyMillis;
        maxExecutionDelayMillis = value.maxExecutionDelayMillis;
        initialBackoffMillis = value.initialBackoffMillis;
        backoffPolicy = value.backoffPolicy;
    }
    public ComponentName getService() { return service; }
    public int getId() { return id; }
    public int getNetworkType() { return networkType; }
    public boolean isPeriodic() { return periodic; }
    public boolean isPersisted() { return persisted; }
    public boolean isExpedited() { return expedited; }
    public boolean isUserInitiated() { return userInitiated; }
    public boolean isRequireCharging() { return requireCharging; }
    public boolean isRequireDeviceIdle() { return requireDeviceIdle; }
    public boolean isRequireBatteryNotLow() { return requireBatteryNotLow; }
    public boolean isRequireStorageNotLow() { return requireStorageNotLow; }
    public Object getTriggerContentUris() { return null; }
    public long getMinLatencyMillis() { return minLatencyMillis; }
    public long getMaxExecutionDelayMillis() { return maxExecutionDelayMillis; }
    public long getInitialBackoffMillis() { return initialBackoffMillis; }
    public int getBackoffPolicy() { return backoffPolicy; }
    public android.os.PersistableBundle getExtras() { return new android.os.PersistableBundle(); }
    public android.os.Bundle getTransientExtras() { return new android.os.Bundle(); }
    public android.content.ClipData getClipData() { return null; }
    public int getClipGrantFlags() { return 0; }

    public static final class Builder {
        private final ComponentName service;
        private final int id;
        private int networkType = NETWORK_TYPE_NONE;
        private boolean periodic;
        private boolean persisted;
        private boolean expedited;
        private boolean userInitiated;
        private boolean requireCharging;
        private boolean requireDeviceIdle;
        private boolean requireBatteryNotLow;
        private boolean requireStorageNotLow;
        private long minLatencyMillis;
        private long maxExecutionDelayMillis;
        private long initialBackoffMillis = 30_000L;
        private int backoffPolicy = BACKOFF_POLICY_EXPONENTIAL;
        public Builder(int jobId, ComponentName value) { id = jobId; service = value; }
        private Builder(ComponentName value, int jobId) { this(jobId, value); }
        public Builder setRequiredNetworkType(int value) { networkType = value; return this; }
        public Builder setPeriodic(long value) { periodic = true; return this; }
        public Builder setPersisted(boolean value) { persisted = value; return this; }
        public Builder setExpedited(boolean value) { expedited = value; return this; }
        public Builder setUserInitiated(boolean value) { userInitiated = value; return this; }
        public Builder setRequiresCharging(boolean value) { requireCharging = value; return this; }
        public Builder setRequiresDeviceIdle(boolean value) { requireDeviceIdle = value; return this; }
        public Builder setRequiresBatteryNotLow(boolean value) { requireBatteryNotLow = value; return this; }
        public Builder setRequiresStorageNotLow(boolean value) { requireStorageNotLow = value; return this; }
        public Builder setMinimumLatency(long value) { minLatencyMillis = value; return this; }
        public Builder setOverrideDeadline(long value) { maxExecutionDelayMillis = value; return this; }
        public Builder setBackoffCriteria(long delay, int policy) {
            initialBackoffMillis = delay; backoffPolicy = policy; return this;
        }
        public JobInfo build() { return new JobInfo(this); }
    }
}
