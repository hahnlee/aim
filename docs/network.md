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
| The devices' IPv6 configuration, `/proc/sys/net/ipv6/conf/*` | `sys/ipv6conf.rs` |
| IPv6 autoconfiguration, the IPv6 routes, the virtual router's advertisements | `sys/addrconf.rs` |
| The guest's IPv6 addresses standing for the Mac's (NAT66) | `sys/nat66.rs` |
| The Mac's network: default routes, addresses, gateways, MTU, DNS servers; watching it | `crates/aim-uplink` (also the app host's `connectivity`); the test hook, the lease and the advertisement: `sys/uplink.rs` |
| NETLINK_ROUTE: dumps, changes, group announcements | `sys/netlink.rs` |
| AF_PACKET SOCK_RAW and classic BPF filters | `sys/packet.rs` |
| `eth0`'s virtual router: DHCP and ARP | `sys/dhcp.rs` |
| Interface ioctls (`SIOCGIFFLAGS`, `SIOCSIFADDR`, `SIOCGIFINDEX`, ...) | `netif::ioctl` |
| SO_MARK, SO_BINDTODEVICE, IP_MULTICAST_ALL, ping sockets, UDP connect to port 0 | `sys/net.rs` (`Family::Inet`) |
| INetd interface calls over ioctls and rtnetlink; `/proc/sys/net` as InterfaceController writes it | `daemons/netd/src/interfaces.rs`, `sysctl.rs` |
| `android.hardware.ethernet` | `image/vendor/etc/permissions/android.hardware.ethernet.xml` |
| NDK tests | `crates/aim-linux-abi/tests/ndk/t_netif.c`, `t_netwatch.c`, `t_ipv6.c` |

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
                                   REQUEST --> <-- ACK (60 s lease)
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
  IPv4 nameservers of `/etc/resolv.conf`. The lease time is 60 seconds, so
  DhcpClient normally renews after 30 seconds and picks up new DNS servers
  without dropping carrier (#307; RFC 2131 section 4.4.5). A
  request for another address is NAKed.
- **Hardware addresses** are fixed and locally administered: `eth0`
  `02:61:69:6d:00:02`, the router `02:61:69:6d:00:01`.
- **IPv6.** netd sets the devices' IPv6 configuration at start as
  InterfaceController's `initializeAll` does (`accept_ra` 2, RA routes in
  per-interface tables, optimistic DAD), and IpClient's `startIPv6` sets
  `accept_ra`, `use_tempaddr` 2, stable privacy addresses (the persisted
  `persist.netd.stable_secret`) and `disable_ipv6` 0. eth0 then gets its
  link-local address; when its DAD completes it solicits routers, and
  the virtual router (`fe80::61:69ff:fe6d:1`) answers with the Mac's IPv6
  network if the Mac has one: its prefix (SLAAC and temporary addresses,
  the prefix's route), a default route and its DNS servers (RDNSS).
  IpClient provisions IPv6 from the netlink messages of these as on a
  device (RTM_NEWADDR, RTM_NEWROUTE, RTM_NEWNDUSEROPT). See "IPv6".
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
Mac: new DNS servers only        the next renewal (normally within 30 s),
                                 a DHCPACK with them; carrier stays up
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
- **Test hook.** `<runtime>/net/simulate` (`<data>.run/net/simulate` for
  `cargo aim boot`) changes what the guest sees without touching the Mac:
  `down` takes the network away; `addr A/P`, `gateway G`, `dns S...` and
  `lease SECONDS` replace those values of the Mac's network, and
  `interface NAME` puts host interface NAME behind it, as the Mac moving
  from Wi-Fi to cable does. For IPv6, `down6` takes the Mac's IPv6 away,
  `addr6 A/P` gives it global address A in prefix P (an IPv6 network on
  the IPv4 one's interface, router `fe80::1`, when it has none), and
  `temp6 A`, `gateway6 G` and `dns6 S...` its temporary address, router
  and DNS servers. An empty or
  missing file is the Mac's network as it is. The app host's
  `connectivity` reads the same hook (`aim_uplink::hook`), so a simulated
  outage or change is what both the guest's `eth0` and the framework's
  default network see. Guest sockets still use the Mac's real network, so
  a simulated address is only what Android is told.

```sh
echo down > target/aim/boot/data.run/net/simulate   # outage
: > target/aim/boot/data.run/net/simulate           # back
```

## The kernel side

**State.** `<runtime>/net/links` holds the devices as text (`link`,
`addr`, `conf6` and `route6` lines, and what the virtual router last
advertised), read and rewritten under `flock` by whichever process makes
a change, which then announces it. `lo` starts up with 127.0.0.1/8 and
::1/128, `eth0` down with no address. Changes need CAP_NET_ADMIN.

**Netlink.** A NETLINK_ROUTE socket is a host AF_UNIX datagram socket bound
to `<runtime>/netlink/<portid>-<groups>`, so poll, epoll and plain reads
work on it unchanged. A request is handled in the sending process; each
reply and each announcement is one datagram sent to the listening names
(a dump is several reads, as on Linux). Port ids are the process id, then
negative numbers, as Linux autobinds. Supported: RTM_GETLINK (dump and by
index or name), RTM_GETADDR dumps, RTM_NEWLINK/RTM_SETLINK (flags, MTU),
RTM_NEWADDR, RTM_DELADDR, RTM_GETROUTE dumps of the IPv6 table, and the
groups RTNLGRP_LINK, RTNLGRP_IPV4_IFADDR, RTNLGRP_IPV6_IFADDR,
RTNLGRP_IPV6_ROUTE, RTNLGRP_IPV6_PREFIX and RTNLGRP_ND_USEROPT. An address
keeps the lifetimes RTM_NEWADDR gives
it (IFA_CACHEINFO, as IpClient sets a lease's): it is reported with the
seconds left, deprecated when its preferred lifetime runs out and removed
when its valid one does, both announced; they are checked whenever a
process looks at the devices, which a process following the Mac's network
does every 2 s. There is no IPv4 routing table (netd keeps those routes as
bookkeeping, and the host routes); RTM_GETNEIGH and RTM_GETRULE dump
nothing. Other requests are EOPNOTSUPP, other netlink families
EPROTONOSUPPORT.

**IPv6 configuration.** `/proc/sys/net/ipv6/conf/{all,default,lo,eth0}`
hold Linux 6.6's per-device IPv6 configuration (`ipv6_devconf`, and
Android's `accept_ra_rt_table`), kept with the devices: reads and writes
as `proc_dointvec` with the handlers' ranges and modes (`stable_secret`
0600, `mc_forwarding` read-only), `all`'s `disable_ipv6`, `forwarding`
and `addr_gen_mode` going to `default` and every device, `stable_secret`
taking an address and turning on stable privacy (EIO for `all`, a bad
address, or a read before one is set). `ipv4/conf` and the neighbour
directories list the devices; their values are kept as written, not
emulated.

**Packet sockets.** SOCK_RAW only, with CAP_NET_RAW. A frame sent on
`eth0` goes to the virtual router and to the sending process's ETH_P_ALL
taps; the router's answer to every socket bound to its protocol there.
Classic BPF filters run on each frame, with Linux's ancillary loads and
network- and link-relative offsets (NetworkStack's filters use
SKF_NET_OFF). Packet sockets of other processes do not see these frames.

**Socket options and semantics libcore and bionic rely on:**

- `SO_MARK` is kept per socket (DnsResolver marks its queries; a failure
  failed every lookup); `SO_BINDTODEVICE` binds the host socket to the
  interface behind the device (`IP_BOUND_IF`/`IPV6_BOUND_IF`). The
  process then follows the Mac's network, and when another of the Mac's
  interfaces comes behind the device its bound sockets move to it.
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
- `IP_MULTICAST_ALL` (an IPv4-level option Linux takes on IPv6 sockets
  too) is kept per socket. libcore clears it on every `DatagramSocket` and
  failed on ENOPROTOOPT, so no Java UDP socket could be made (SntpClient's
  network time among them, #515). Darwin delivers multicast only to
  sockets that joined the group, as Linux does with the flag cleared; a
  socket that keeps it set does not get groups other sockets joined
  (#524).

**netd.** Our netd sets and reads interfaces through the ioctls and
rtnetlink, as the original's InterfaceController does; networks, routes,
firewall and bandwidth calls stay bookkeeping. fwmarkd answers without
marking: the mark would live in netd's copy of the socket state, not the
client's, and nothing routes by it.

**DNS.** ConnectivityService gives DnsResolver the lease's servers through
LinkProperties; its UDP and TCP queries are host sockets. DNS-over-TLS
validation against those servers runs and fails over to cleartext, as on
a network whose resolver has no DoT.

**Socket options and interval timers (#268).** IP_RECVTTL/IP_RECVTOS and
SO_TIMESTAMP/SO_TIMESTAMPNS translate Darwin's receive control messages,
including ping replies; IP_RETOPTS is Darwin's IP_RECVRETOPTS (#2152).
TCP_MAXSEG keeps Linux's user MSS before connect and applies it when
Darwin permits. The three interval timers deliver SIGALRM, SIGVTALRM and
SIGPROF through the signal owner. The NDK option and timer tests pass
(2026-10-28); ping and toybox nc have not been rechecked in a boot.

**Traffic statistics** come from the eBPF maps. The socket layer counts
each AF_INET/AF_INET6 send and receive into netd's per-uid maps
(`stats_map_A`/`B`, `app_uid_stats_map`) as its cgroup skb programs
would, with the socket's uid, tag, counter set and interface (`lo` for
loopback addresses, `eth0` for the rest). One call counts as one packet
behind its headers, split at the MTU as the programs split a GSO skb;
TCP's control packets are not counted. The same hooks fill `iface_stats_map`
independently of UID tags and counter sets, excluding system-owned CLAT egress
as the original xt_bpf program does. `iface_index_name_map` registers the guest
devices when the maps are discovered. Host checks cover the map selection,
CLAT exclusion and real loopback UDP send/receive through pinned maps, without
a guest boot (#224, #1720). `/proc/net/dev` lists `netif`'s devices in Linux's
format, with Darwin's 64-bit interface totals (`NET_RT_IFLIST2`): `lo0` for `lo`, the
current uplink for `eth0`. These include host traffic and Darwin's rounding
of byte totals. Unsupported fields and a device without a backing host
interface have zero counters. NDK tests cover the proc aliases, headers,
fields and fresh counters after loopback UDP traffic; the original toybox
`ifconfig -a` lists both devices without a boot (2026-10-28).

## IPv6 (#267)

The guest gets IPv6 as a Linux host behind a router does; the router is
the Mac's IPv6 network (`aim-uplink`: the interface of its IPv6 default
route, its usable global address and the temporary one by their
`IN6_IFF_*` flags, its router, its global IPv6 nameservers).

- **Autoconfiguration** (`sys/addrconf.rs`, Linux's `addrconf.c`). A
  device whose link becomes ready (up, with carrier) with IPv6 enabled
  gets a link-local address by its `addr_gen_mode`: EUI-64 from its
  hardware address, or RFC 7217 stable privacy from `stable_secret`
  (`ipv6_generate_stable_address`, with the one-block `sha1_transform`).
  Addresses are tentative while DAD runs (`dad_transmits` seconds),
  optimistic where `optimistic_dad` allows. Taking a device down or
  disabling IPv6 clears its IPv6 addresses and routes.
- **The router.** When `eth0`'s link-local address completes DAD it
  solicits routers, and the virtual router (`fe80::61:69ff:fe6d:1`)
  answers with the Mac's prefix (valid 30 days, preferred 7), its MTU and
  DNS servers (RDNSS, 1800 s), router lifetime 1800 s. It advertises
  again every 600 s and at once when the Mac's IPv6 changes: an old
  prefix and servers with zero lifetimes, and a zero router lifetime when
  the Mac has none. Receiving an advertisement is `ndisc_router_discovery`
  as the device's configuration allows: the default route (proto ra,
  `ra_defrtr_metric`, not below `accept_ra_min_lft`), the hop limit and
  MTU, the prefix's route, the SLAAC address with RFC 4862's two-hour
  rule, RFC 8981 temporary addresses with `use_tempaddr` (regenerated
  before they deprecate), RTM_NEWPREFIX, and RDNSS as RTM_NEWNDUSEROPT.
- **Routes.** The kernel's IPv6 table holds prefix routes, fe80::/64, the
  default route, the local table's routes of usable addresses and
  ff00::/8, in the table `accept_ra_rt_table` names (netd's -1000: table
  1002 for `eth0`). Nothing routes by it: the host routes every socket.
- **NAT66** (`sys/nat66.rs`). SLAAC and temporary addresses are not the
  Mac's, and guest sockets are host sockets. As a NAT66 router does (RFC
  6296), the socket layer gives the host the Mac's address where the
  guest names one of its own in the advertised prefix (bind, connect,
  sendto) and the guest its own where the host reports the Mac's
  (getsockname, getpeername, recvfrom, accept): a temporary address
  stands for the Mac's temporary one, the stable one for its stable one.
  Back, the Mac's temporary address is the newest temporary one; with no
  temporary address on the Mac, every guest address stands for its one
  address, which comes back as the stable one.

Not done: the router's frames on packet sockets (the advertisement is
received by the kernel side only; nothing in the pinned NetworkStack
reads it there but ApfFilter, which needs APF the device does not
offer), DHCPv6, and IPv6 control messages (`IPV6_PKTINFO`). A boot on a
Mac with IPv6 has not been verified yet: this Mac has none, and the
ndk test (`t_ipv6.c`) drives the Mac's IPv6 through the test hook.

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
  netlink module's NETLINK_GENERIC (`nlctrl` family resolution, served)
  gains an `nl80211` family: GET_WIPHY (bands and channels), GET_INTERFACE,
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
- **Features and HAL declarations.** The image declares
  `android.hardware.wifi`; Direct and Passpoint, which ours would not
  back, are removed (`image/overlay.toml`); the vintf manifest gains the
  Wi-Fi and supplicant HALs.
  EthernetService keeps `eth0` for wired Macs.

## Open

- #267 IPv6 on `eth0`: verify on a Mac with IPv6 (`dumpsys connectivity`
  with an IPv6 address, default route and DNS on eth0, `ping6`);
- #260 Chrome's renderer: "V8 process OOM (Failed to reserve virtual
  memory for CodeRange)";
- #224 traffic statistics.
