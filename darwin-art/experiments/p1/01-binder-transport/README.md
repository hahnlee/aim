# P1-1: host transport for the binder driver

## Question

The binder driver's cross-process state lives in darwin-artd (ADR 0012,
decision 7). Every guest `BINDER_WRITE_READ` then becomes a message to the
daemon, and a call with its reply crosses it four times: client → daemon →
server, and server → daemon → client. Can that path stay within Linux binder's
10–30 µs per transaction? Which host primitive should carry it?

## Method

`transport.c` forks the roles and measures 20,000 round trips (after 2,000
warm-up rounds) with 128-byte inline messages:

- **direct**: client ↔ server over one Mach port (two hops). This is the
  floor if the driver ran inside the guests.
- **relayed**: client → daemon → server → daemon → client over Mach. Each
  side blocks on its own reply port with one `mach_msg(SEND|RCV)`, the way a
  thread blocks in `BINDER_WRITE_READ`. The daemon parks the server's read
  (a send-once right) until a call arrives, then parks the caller until the
  reply comes back.
- **socket**: the same relayed path over `AF_UNIX` stream socketpairs.

Ports reach the children through `mach_ports_register`. Each mode runs in its
own process.

## Results (M2 Pro, macOS 27.0)

| path | p50 | p90 | p99 | mean |
| --- | --- | --- | --- | --- |
| direct Mach (2 hops) | 6.2–6.6 µs | 10.1–10.5 µs | 13.8–14.0 µs | 7.3–7.5 µs |
| relayed Mach (4 hops) | 6.5–6.7 µs | 13.8–13.9 µs | 18.8–19.1 µs | 8.5–8.6 µs |
| relayed sockets (4 hops) | 10.3–10.5 µs | 12.2–13.0 µs | 15.6–16.3 µs | 10.4–10.6 µs |

For scale, the driver core alone (`crates/darwin-binder-driver`, in-process,
two OS threads, release build) completes a small call and reply in 9.2 µs
at p50 (11.2 µs p90, 17.3 µs p99). A bare condvar ping-pong between two
threads takes 2.6–2.9 µs.

## Conclusion

**Feasible with Mach messages.** A call and reply through the daemon costs
about 6.5 µs of transport at p50, no more than a direct two-hop exchange.
The likely reason is that the receiver of each hop is already blocked in
`mach_msg`, so the kernel can hand the CPU straight to it. Added to the
core's own work, a daemon-hosted driver should land near 10 µs per
transaction, at the low end of Linux's range. Unix sockets cost about 4 µs
more at p50.

Mechanism: one Mach receive port per guest binder thread (its reply port),
one `mach_msg(SEND|RCV)` per ioctl, and the daemon replying to a parked
send-once right once the ioctl completes. Fds travel as fileports in the
same messages.
