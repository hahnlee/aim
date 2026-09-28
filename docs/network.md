# Network (ADR 0012, #223)

Android sees one **Ethernet** network, `eth0`, which stands for the Mac's
network. The original ConnectivityService, EthernetService, NetworkStack
(IpClient, DhcpClient, NetworkMonitor) and DnsResolver run unmodified and
bring it up the way they would on a device with a wired port and a DHCP
server. No guest packet crosses `eth0`: guest AF_INET/AF_INET6 sockets are
host sockets (`sys/net.rs`), so the host routes every connection. What the
layer adds is the kernel's side of the configuration Android reads and
writes.

| Piece | Where |
| --- | --- |
| Devices `lo` and `eth0`: flags, MTU, addresses, shared by a boot's processes | `crates/aim-linux-abi/src/sys/netif.rs` |
| The Mac's network: default route, address, gateway, MTU, DNS servers; watching it | `sys/uplink.rs` |
| NETLINK_ROUTE: dumps, changes, group announcements | `sys/netlink.rs` |
| AF_PACKET SOCK_RAW and classic BPF filters | `sys/packet.rs` |
| `eth0`'s virtual router: DHCP and ARP | `sys/dhcp.rs` |
| Interface ioctls (`SIOCGIFFLAGS`, `SIOCSIFADDR`, `SIOCGIFINDEX`, ...) | `netif::ioctl` |
| SO_MARK, SO_BINDTODEVICE, ping sockets, UDP connect to port 0 | `sys/net.rs` (`Family::Inet`) |
| INetd interface calls over ioctls and rtnetlink | `daemons/netd/src/interfaces.rs` |
| `android.hardware.ethernet` | `image/vendor/etc/permissions/android.hardware.ethernet.xml` |
| NDK tests | `crates/aim-linux-abi/tests/ndk/t_netif.c`, `t_netwatch.c` |

## Bring-up

```text
system_server                     NetworkStack (IpClient)          netd
EthernetTracker                                                    (INetd)
  getifaddrs -> eth0 (netlink dump)
  interfaceSetCfg(up) ------------------------------------------->  SIOCSIFFLAGS
  <- RTM_NEWLINK, IFF_LOWER_UP (RTNLGRP_LINK) ---------- kernel (netif) <-'
  IpClient.start ----------------> DhcpClient
                                   AF_PACKET: DISCOVER --> virtual router
                                   <-- OFFER (the Mac's address, gateway, DNS)
                                   REQUEST --> <-- ACK (1 h lease)
                                   interfaceSetCfg(addr) ---------> SIOCSIFADDR,
                                                                    SIOCSIFNETMASK
                                   <- RTM_NEWADDR (RTNLGRP_IPV4_IFADDR) <- kernel
                                   provisioned: address, default route, DNS
  NetworkAgent (Ethernet) <------- onProvisioningSuccess
ConnectivityService: networkCreate, addInterface, routes -> netd (bookkeeping)
                     setResolverConfiguration -> DnsResolver (Mac's DNS servers)
NetworkMonitor: DNS through DnsResolver, HTTP and HTTPS generate_204 probes
                through host sockets -> VALIDATED, default network
```

- **Carrier.** `eth0` is up when userspace sets IFF_UP, and has carrier
  (IFF_RUNNING, IFF_LOWER_UP, IF_OPER_UP) while the Mac has an IPv4 default
  route. Changes are announced (see "Following the Mac's network").
- **The lease** is the Mac's primary network as a routing socket and
  `getifaddrs` report it: the address and prefix of the default route's
  interface, its gateway (also the DHCP server identifier) and MTU, and the
  IPv4 nameservers of `/etc/resolv.conf`. The lease time is one hour, so
  DhcpClient renews after 30 minutes and picks up new DNS servers. A
  request for another address is NAKed.
- **Hardware addresses** are fixed and locally administered: `eth0`
  `02:61:69:6d:00:02`, the router `02:61:69:6d:00:01`.
- **The emulator's overlay goes.** Its vendor overlay made `eth0` a
  restricted network (`config_ethernet_interfaces` "eth0;11,12,14;;"),
  because the emulator's `eth0` is its modem link. Ours is an ordinary
  Ethernet network with the default capabilities (INTERNET, NOT_RESTRICTED,
  NOT_METERED, TRUSTED, ...).

## Following the Mac's network (#266)

The Mac joins and leaves networks, sleeps, and moves from Wi-Fi to a
cable. The guest sees this as a device with a cable would:

```text
Mac: default route gone          eth0: carrier lost -> RTM_NEWLINK without
                                   IFF_RUNNING/IFF_LOWER_UP
  EthernetTracker (NetlinkMonitor): link down -> IpClient stopped,
  NetworkAgent unregistered -> ConnectivityService: network lost
Mac: default route back          eth0: carrier back -> RTM_NEWLINK with
                                   IFF_RUNNING/IFF_LOWER_UP
  EthernetTracker: link up -> new IpClient -> DHCP (the new lease) ->
  new NetworkAgent -> NetworkMonitor validates -> default network
Mac: another network (address,   eth0: carrier lost and back at once,
  gateway, MTU or interface)       so the steps above run with a new lease
Mac: new DNS servers only        the next renewal (a DHCPACK with them)
```

- **Watching.** Every process that binds a NETLINK_ROUTE socket to
  RTNLGRP_LINK (system_server's EthernetTracker, NetworkStack) runs a
  thread with a routing socket (`PF_ROUTE`): a change of routes other than
  cloned, host and link-layer entries, or of addresses or interfaces, has
  it look at the Mac's network; it also looks every 2 s, for DNS servers
  and the test hook, which change without a routing message. A routing
  socket needs no entitlement and no run loop, unlike SCDynamicStore or
  `nw_path_monitor`.
- **Once per change.** The network last seen is part of the shared device
  state (`uplink` line of `<runtime>/net/links`). Every process that reads
  or changes the devices compares it with the Mac's current one under the
  state's lock, and the one that finds a change announces it; the other
  watchers then find nothing new.
- **Renewals stay in the guest.** DhcpClient renews by UDP from a socket
  bound to `eth0` and port 68, to the lease's server (RENEWING) or to
  255.255.255.255 (REBINDING). Such a datagram to port 67 becomes the
  frame `eth0` would carry and goes to the virtual router, whose answer
  reaches DhcpClient's packet socket; it never reaches the Mac's network.
  The socket's port 68 is `eth0`'s, not the Mac's: a datagram socket
  bound to a device other than `lo` that binds the DHCP client port is
  bound on the virtual link only (no host port, so the Mac, other guests
  and the NDK tests keep theirs), and nothing from the Mac's network
  arrives on it. Such binds do not conflict with each other.
  Without a network it fails with ENETUNREACH. A renewal for an address
  the Mac no longer has is NAKed, and DhcpClient starts over.
- **Test hook.** `<runtime>/net/simulate` (`<data>/run/net/simulate` for
  `cargo aim boot`) changes what the guest sees without touching the Mac:
  `down` takes the network away; `addr A/P`, `gateway G`, `dns S...` and
  `lease SECONDS` replace those values of the Mac's network. An empty or
  missing file is the Mac's network as it is. Guest sockets still use the
  Mac's real network, so a simulated address is only what Android is told.

```sh
echo down > target/aim/boot/data/run/net/simulate   # outage
: > target/aim/boot/data/run/net/simulate           # back
```

## The kernel side

**State.** `<runtime>/net/links` holds the devices as text (`link` and
`addr` lines), read and rewritten under `flock` by whichever process makes
a change, which then announces it. `lo` starts up with 127.0.0.1/8 and
::1/128, `eth0` down with no address. Changes need CAP_NET_ADMIN.

**Netlink.** A NETLINK_ROUTE socket is a host AF_UNIX datagram socket bound
to `<runtime>/netlink/<portid>-<groups>`, so poll, epoll and plain reads
work on it unchanged. A request is handled in the sending process; each
reply and each announcement is one datagram sent to the listening names
(a dump is several reads, as on Linux). Port ids are the process id, then
negative numbers, as Linux autobinds. Supported: RTM_GETLINK (dump and by
index or name), RTM_GETADDR dumps, RTM_NEWLINK/RTM_SETLINK (flags, MTU),
RTM_NEWADDR, RTM_DELADDR, and the groups RTNLGRP_LINK, RTNLGRP_IPV4_IFADDR
and RTNLGRP_IPV6_IFADDR. RTM_GETROUTE, RTM_GETNEIGH and RTM_GETRULE dump
nothing: there is no routing table (netd keeps routes as bookkeeping, and
the host routes). Other requests are EOPNOTSUPP, other netlink families
EPROTONOSUPPORT.

**Packet sockets.** SOCK_RAW only, with CAP_NET_RAW. A frame sent on
`eth0` goes to the virtual router and to the sending process's ETH_P_ALL
taps; the router's answer to every socket bound to its protocol there.
Classic BPF filters run on each frame, with Linux's ancillary loads and
network- and link-relative offsets (NetworkStack's filters use
SKF_NET_OFF). Packet sockets of other processes do not see these frames.

**Socket options and semantics libcore and bionic rely on:**

- `SO_MARK` is kept per socket (DnsResolver marks its queries; a failure
  failed every lookup); `SO_BINDTODEVICE` binds the host socket to the
  interface behind the device (`IP_BOUND_IF`/`IPV6_BOUND_IF`).
- `SO_PROTOCOL` is the host socket's protocol. libcore exempts UDP
  `connect` from StrictMode's network-on-main-thread check by it.
- An AF_INET6 address on an AF_INET socket is EAFNOSUPPORT (Darwin says
  EINVAL). libcore tries a v4-mapped address first and falls back on that
  errno; every Java `bind`/`connect` of an AF_INET socket failed before.
- A datagram socket connects to port 0 (bionic's and DnsResolver's "have
  IPv4/IPv6" probes, and RFC 6724 source selection); Darwin refuses it, so
  the host socket connects to the discard port and `getpeername` says 0.
- IPv4 ping sockets (SOCK_DGRAM, IPPROTO_ICMP) receive the ICMP message
  without the IP header, as on Linux.
- `SO_RCVBUF`/`SO_SNDBUF` of 0 are raised to Linux's minimum.

**netd.** Our netd sets and reads interfaces through the ioctls and
rtnetlink, as the original's InterfaceController does; networks, routes,
firewall and bandwidth calls stay bookkeeping. fwmarkd answers without
marking: the mark would live in netd's copy of the socket state, not the
client's, and nothing routes by it.

**DNS.** ConnectivityService gives DnsResolver the lease's servers through
LinkProperties; its UDP and TCP queries are host sockets. DNS-over-TLS
validation against those servers runs and fails over to cleartext, as on
a network whose resolver has no DoT.

**Traffic statistics** come from the eBPF maps, which no program fills
(#224); they read zero.

## Verified (2026-09-28)

First boot, `cargo aim boot --exclude bootanim` on an M2 Pro on a Wi-Fi
network (172.30.1.0/24):

- DhcpClient: OFFER and ACK of 172.30.1.46/24, gateway 172.30.1.254, the
  Mac's two DNS servers; IpClient sees the address by RTM_NEWADDR.
- `dumpsys connectivity`: `Active default network: 100`, Ethernet,
  `IS_VALIDATED`; NetworkMonitor's DNS probes resolve, HTTP and HTTPS
  `generate_204` answer 204 (about 0.7 s).
- Shell: `ping -c 3 www.google.com` 3/3 replies; an NDK program resolves
  `example.com` through DnsResolver (`getaddrinfo` with AI_ADDRCONFIG) and
  reads `HTTP/1.1 200 OK`; `ip addr` shows `eth0` with the lease.
- Chrome (`_build/installed-apps`): its browser process holds HTTPS
  connections to example.com and Google; the renderer dies before drawing
  (#260). Chrome's "No such process (3)" warnings (#257) are gone: they
  were DnsResolver's ESRCH for a network with no nameservers.

### Verified: following the Mac's network (2026-09-29)

`cargo aim boot` on the same Mac, with the outage and the other network
simulated through the test hook (the Mac's own network untouched):

- Network available: `Active default network: 100`, Ethernet,
  `IS_VALIDATED`, 172.30.1.49/24; `ping -c 3 www.google.com` 3/3.
- `down`: EthernetTracker `interfaceLinkStateChanged, iface: eth0, up:
  false`; `Active default network: none` within 4 s; ping fails (no statistics).
- Back: network 101, `IS_VALIDATED` within 4 s; ping 3/3.
- Another network (10.77.1.23/24 via 10.77.1.1, DNS 1.1.1.1, 60 s lease):
  link down and up, a new DHCP lease, network 102 `IS_VALIDATED` with
  10.77.1.23/24 within 4 s; ping 3/3; DhcpClient `Renewed lease ... DHCP
  server /10.77.1.1 ... lease 60 seconds` through the virtual router.
- The Mac's network again: network 103 `IS_VALIDATED`, 172.30.1.49/24.
- Booted with the hook at `down`: no default network 20 s after
  `sys.boot_completed`; emptying the hook gave network 100
  `IS_VALIDATED` within 6 s, and ping 3/3.

## Stage 2: presenting it as Wi-Fi (#265)

Most apps and Settings expect Wi-Fi, and a Mac is usually on Wi-Fi. The
network would then be `wlan0`, owned by the original Wi-Fi stack
(`com.android.wifi`: WifiService, ClientModeImpl, WifiScanner), still
provisioned by IpClient and our DHCP router, and still carried by host
sockets. What it takes, below the original stack:

- **Wi-Fi HAL** (`android.hardware.wifi` AIDL: IWifi, IWifiChip,
  IWifiStaIface), a Rust HAL like the others, over a host-call module
  `wifi` backed by CoreWLAN (`CWWiFiClient`, the interface of the Mac's
  default route). It reports one chip with one STA interface named
  `wlan0`, capabilities, and link-layer statistics (RSSI, tx rate) from
  `CWInterface`.
- **Supplicant** (`android.hardware.wifi.supplicant` AIDL: ISupplicant,
  ISupplicantStaIface, ISupplicantStaNetwork), also ours: the Mac owns
  association, so the supplicant mirrors it. It reports COMPLETED for the
  network the Mac is on (SSID, BSSID, frequency, key management) and the
  state changes when the Mac roams or disconnects. A connect to another
  network is refused; the guest does not steer the Mac's Wi-Fi.
  ClientModeImpl only connects to a network it has a WifiConfiguration
  for, so first boot must save one for the Mac's SSID (a network
  suggestion from a setup step, or `cmd wifi connect-network` in the
  device's provisioning), without its secret: the supplicant never needs
  it.
- **wificond over an nl80211 subset.** The original wificond stays. The
  netlink module gains NETLINK_GENERIC with the `nlctrl` family resolution
  and an `nl80211` family: GET_WIPHY (bands and channels), GET_INTERFACE,
  TRIGGER_SCAN and GET_SCAN (results from `CWInterface.scanForNetworks`),
  GET_STATION (signal, tx bitrate) and the `scan`, `mlme` and
  `regulatory` multicast groups; unsupported commands are EOPNOTSUPP.
- **`wlan0` in netif.** The device named after the Mac's default-route
  interface type: `wlan0` (ARPHRD_ETHER) when it is Wi-Fi. `eth0` stays for
  a wired Mac. Carrier follows the supplicant's association, and both
  follow the host's route changes as `eth0` does.
- **Location permission.** macOS gives SSIDs, BSSIDs and scan results
  only to processes with Location Services authorization, so the host side
  must hold it (the process running the `wifi` module, or aim-display
  asking once); without it the HAL reports a hidden network. On the guest
  side the framework already requires ACCESS_FINE_LOCATION for scan
  results.
- **Features and HAL declarations.** The image already declares
  `android.hardware.wifi` (and Direct and Passpoint, which ours would not
  back: they go); the vintf manifest gains the Wi-Fi and supplicant HALs.
  EthernetService keeps `eth0` for wired Macs.

## Open

- #267 no IPv6 on `eth0` (no router advertisements or SLAAC); IPv6 sockets
  still reach the Internet through the host;
- #260 Chrome's renderer: "V8 process OOM (Failed to reserve virtual
  memory for CodeRange)";
- #268 socket options: IP_RECVTTL, IP_RETOPTS, SO_TIMESTAMP (iputils
  ping), TCP_MAXSEG before connect (DnsResolver's DoT), and `setitimer`
  (toybox `nc -w`); `/proc/net/dev` for `ifconfig`;
- #224 traffic statistics.
