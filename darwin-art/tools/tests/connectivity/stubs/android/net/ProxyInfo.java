package android.net;

import android.os.Parcel;
import android.os.Parcelable;

public final class ProxyInfo implements Parcelable {
    public static final Creator<ProxyInfo> CREATOR = new Creator<ProxyInfo>() {
        public ProxyInfo createFromParcel(Parcel source) {
            return new ProxyInfo(source.readString(), source.readInt());
        }
        public ProxyInfo[] newArray(int size) { return new ProxyInfo[size]; }
    };

    private final String host;
    private final int port;

    public ProxyInfo(String host, int port) {
        this.host = host;
        this.port = port;
    }

    public String getHost() { return host; }
    public int getPort() { return port; }

    public int describeContents() { return 0; }
    public void writeToParcel(Parcel dest, int flags) {
        dest.writeString(host);
        dest.writeInt(port);
    }
}
