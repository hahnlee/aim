package android.net;

import android.os.Parcel;
import android.os.Parcelable;

public final class NetworkInfo implements Parcelable {
    public enum DetailedState { IDLE, CONNECTED, DISCONNECTED }
    public static final Creator<NetworkInfo> CREATOR = new Creator<NetworkInfo>() {
        public NetworkInfo createFromParcel(Parcel source) {
            return new NetworkInfo(source.readInt(), source.readInt(), source.readString(),
                    source.readString(), source.readInt() != 0);
        }
        public NetworkInfo[] newArray(int size) { return new NetworkInfo[size]; }
    };

    private final int type;
    private final int subtype;
    private final String typeName;
    private final String subtypeName;
    private boolean available;
    private DetailedState detailedState = DetailedState.IDLE;

    public NetworkInfo(int type, int subtype, String typeName, String subtypeName) {
        this(type, subtype, typeName, subtypeName, false);
    }
    private NetworkInfo(int type, int subtype, String typeName, String subtypeName,
            boolean connected) {
        this.type = type; this.subtype = subtype; this.typeName = typeName;
        this.subtypeName = subtypeName; detailedState = connected
                ? DetailedState.CONNECTED : DetailedState.IDLE;
    }
    public void setDetailedState(DetailedState value, String reason, String extraInfo) {
        detailedState = value;
    }
    public boolean isConnected() { return detailedState == DetailedState.CONNECTED; }
    public int getType() { return type; }
    public void writeToParcel(Parcel dest, int flags) {
        dest.writeInt(type); dest.writeInt(subtype); dest.writeString(typeName);
        dest.writeString(subtypeName); dest.writeInt(isConnected() ? 1 : 0);
    }
}
