// Compile-only pinned framework API; checked by the device-services linkage gate.
package android.net;
import android.os.Parcel;
import android.os.Parcelable;
public final class NetworkRequest implements Parcelable {
    public NetworkRequest(NetworkRequest source) { throw new RuntimeException("stub"); }
    public static final Creator<NetworkRequest> CREATOR = null;
    public int describeContents() { throw new RuntimeException("stub"); }
    public void writeToParcel(Parcel p, int flags) { throw new RuntimeException("stub"); }
    public final int requestId = 0;
    public final Type type = null;
    public enum Type { NONE, LISTEN, TRACK_DEFAULT, REQUEST, BACKGROUND_REQUEST, TRACK_SYSTEM_DEFAULT, LISTEN_FOR_BEST, RESERVATION }
}
