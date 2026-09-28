package android.net;

import java.net.InetAddress;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Collections;
import java.util.List;

/** Minimal API-29 link snapshot exposed by the process-local host network. */
public final class LinkProperties {
    private List<InetAddress> dnsServers = Collections.emptyList();
    private String domains;
    private String interfaceName;

    public LinkProperties() {}

    public LinkProperties(LinkProperties source) {
        if (source == null) return;
        dnsServers = source.dnsServers;
        domains = source.domains;
        interfaceName = source.interfaceName;
    }

    public List<InetAddress> getDnsServers() { return dnsServers; }
    public String getDomains() { return domains; }
    public boolean isPrivateDnsActive() { return false; }
    public String getPrivateDnsServerName() { return null; }
    public String getInterfaceName() { return interfaceName; }

    public void setDnsServers(Collection<InetAddress> servers) {
        dnsServers = servers == null || servers.isEmpty()
                ? Collections.emptyList()
                : Collections.unmodifiableList(new ArrayList<>(servers));
    }

    public void setDomains(String value) { domains = value; }
    public void setInterfaceName(String value) { interfaceName = value; }
}
