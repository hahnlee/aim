package android.app.job;

import android.os.Parcel;
import android.os.Parcelable;

public final class JobWorkItem implements Parcelable {
    public static final Parcelable.Creator<JobWorkItem> CREATOR =
            new Parcelable.Creator<JobWorkItem>() {
                public JobWorkItem createFromParcel(Parcel source) { return null; }
                public JobWorkItem[] newArray(int size) { return new JobWorkItem[size]; }
            };
}
