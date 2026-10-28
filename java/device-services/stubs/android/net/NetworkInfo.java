// Compile-only pinned framework API; checked by the device-services linkage gate.
package android.net;
import android.os.Parcel;
import android.os.Parcelable;
public final class NetworkInfo implements Parcelable {
    public NetworkInfo(int type, int subtype, String typeName, String subtypeName) { throw new RuntimeException("stub"); }
    public static final Creator<NetworkInfo> CREATOR = null;
    public int describeContents() { throw new RuntimeException("stub"); }
    public void writeToParcel(Parcel p, int flags) { throw new RuntimeException("stub"); }
    public String toShortString() { throw new RuntimeException("stub"); }
}
