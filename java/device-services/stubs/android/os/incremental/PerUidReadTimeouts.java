package android.os.incremental;
public class PerUidReadTimeouts implements android.os.Parcelable {
    public int uid;public long minTimeUs,minPendingTimeUs,maxPendingTimeUs;
    public static final android.os.Parcelable.Creator<PerUidReadTimeouts> CREATOR=null;
    public void writeToParcel(android.os.Parcel p,int flags){throw new RuntimeException("stub");}
    public int describeContents(){throw new RuntimeException("stub");}
}
