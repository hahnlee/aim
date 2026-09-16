package android.app.job;

import android.content.ClipData;
import android.net.Network;
import android.os.Bundle;
import android.os.IBinder;
import android.os.PersistableBundle;

public final class JobParameters implements android.os.Parcelable {
    public static final android.os.Parcelable.Creator<JobParameters> CREATOR =
            new android.os.Parcelable.Creator<JobParameters>() {
                public JobParameters createFromParcel(android.os.Parcel source) { return null; }
                public JobParameters[] newArray(int size) { return new JobParameters[size]; }
            };
    private final IBinder callback;
    private final int jobId;
    public JobParameters(IBinder callback, String namespace, int jobId,
            PersistableBundle extras, Bundle transientExtras, ClipData clipData,
            int clipGrantFlags, boolean overrideDeadlineExpired, boolean expedited,
            boolean userInitiated, android.net.Uri[] triggeredContentUris,
            String[] triggeredContentAuthorities, Network network) {
        this.callback = callback;
        this.jobId = jobId;
    }
    public IBinder getCallbackForTest() { return callback; }
    public int getJobId() { return jobId; }
}
