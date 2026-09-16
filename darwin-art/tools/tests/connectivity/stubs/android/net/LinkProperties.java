package android.net;

import android.os.Parcel;
import android.os.Parcelable;
import java.net.InetAddress;
import java.util.ArrayList;
import java.util.Collection;
import java.util.List;

public final class LinkProperties implements Parcelable {
    public static final Creator<LinkProperties> CREATOR = new Creator<LinkProperties>() {
        public LinkProperties createFromParcel(Parcel source) { source.readInt(); return new LinkProperties(); }
        public LinkProperties[] newArray(int size) { return new LinkProperties[size]; }
    };
    private String interfaceName;
    private final List<InetAddress> dnsServers = new ArrayList<InetAddress>();

    public LinkProperties() {}
    public void setInterfaceName(String value) { interfaceName = value; }
    public String getInterfaceName() { return interfaceName; }
    public boolean addDnsServer(InetAddress value) { return dnsServers.add(value); }
    public void setDnsServers(Collection<InetAddress> values) {
        dnsServers.clear();
        dnsServers.addAll(values);
    }
    public List<InetAddress> getDnsServers() { return new ArrayList<InetAddress>(dnsServers); }
    public void writeToParcel(Parcel dest, int flags) { dest.writeInt(1); }
}
