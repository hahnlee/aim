# Bluetooth (ADR 0012, P5)

The original Android Bluetooth stack (the `com.android.bt` APEX: the
Bluetooth app and `libbluetooth_jni.so`, gd and the Rust stack) runs
unmodified. It talks HCI to `android.hardware.bluetooth.IBluetoothHci/default`
as it would to a chip. Our HAL answers as a **virtual LE controller over
CoreBluetooth**: the Wine-style split of a guest contract (HCI) and a host
provider (CoreBluetooth).

| Piece | Where |
| --- | --- |
| HAL service (AIDL V1) | `hal/bluetooth`, `/vendor/bin/hw/android.hardware.bluetooth-service.darwin` |
| Host-call module `bluetooth` (id 8) | `crates/darwin-host-bluetooth`, ABI in `darwin_hostcall::bluetooth` |
| Virtual controller (HCI commands, events, ACL) | `darwin-host-bluetooth/src/controller.rs`, `hci.rs` |
| L2CAP fixed channels | `l2cap.rs` |
| ATT server over a device's GATT tree | `att.rs`, `gatt.rs` |
| Advertisement data and addresses | `adv.rs` |
| The radio | `corebluetooth/` (`CBCentralManager`, `CBPeripheralManager`) |
| Test client (not in the image) | `hal/tests/bluetooth-hci-client` |
| End-to-end test | `crates/darwin-linux-abi/tests/bluetooth.rs` |

## Data path

```text
Bluetooth app (gd)  --binder-->  HAL: sendHciCommand / sendAclData
                                   | host call FN_SEND
                                   v
                     controller (host, in the HAL's process)  <-- CoreBluetooth
                                   | queue; wake pipe readable       delegate
                                   v                                 callbacks on
HAL reader thread: poll(fd) -> FN_RECV* -> hciEventReceived /        its own
                                          aclDataReceived            dispatch queue
```

- Packets carry no H4 type byte, as in `IBluetoothHci`.
- `FN_OPEN` returns the read end of a pipe. The host writes one byte when
  its queue goes from empty to non-empty, and drains the pipe (through its
  own duplicate of the read end) when `FN_RECV` empties the queue, under the
  same lock. So the fd is readable exactly while packets wait.
- Host code never calls guest code. CoreBluetooth runs on a serial dispatch
  queue of its own; each request is queued there (`dispatch_async_f`) and
  never blocks the guest thread.
- The framework is loaded with `dlopen` on the first `FN_OPEN`, so other
  guest processes never load it.
- `close`, or the death of the stack's callback object, closes the
  controller. The host then closes its end of the pipe, and the reader
  thread sees the hang-up.

## The controller

It reports HCI and LMP 5.3, manufacturer `0xffff`, and the LMP features
*LE supported* and *BR/EDR not supported*. The stack therefore runs LE
only; CoreBluetooth has no BR/EDR. They also claim *Secure Simple
Pairing*: the Android stack asserts it at start-up (`btm_sec_dev_reset`,
"only controllers with SSP is supported") and aborts without it, LE only
or not.

- **Address.** Derived from the Mac's hardware UUID (`gethostuuid`), marked
  locally administered. It is stable per Mac and never a real device's
  address.
- **Start-up.** Every command the stack sends unconditionally at start-up
  (`Controller::impl::Start` in `system/gd/hci/controller.cc`) succeeds,
  including the BR/EDR settings commands. A dual-mode controller with an
  idle BR/EDR radio would store those too.
  - The Android vendor command `LE_Get_Vendor_Capabilities` is an unknown
    command, so the stack takes no vendor capabilities.
  - The Supported Commands bitmap lists exactly the implemented commands
    (`hci::cmd::SUPPORTED`). Everything else, such as Inquiry or classic
    Create Connection, is an unknown command.
- **LE features.** Only *LE Extended Advertising*. With it, one extended
  report carries a device's whole advertisement, which CoreBluetooth hands
  over merged from the advertising PDU and the scan response. There is no
  LE encryption bit: macOS pairs and encrypts on its own.
- **Event masks** are honoured, with the spec's defaults after reset. The
  default mask lacks LE Meta, which the stack enables.

### Scanning

`LE Set (Extended) Scan Enable` scans with `scanForPeripherals`. Each
advertisement becomes a report:

- after the legacy commands, LE Advertising Reports: the rebuilt data split
  into an `ADV_IND`/`ADV_NONCONN_IND` and a `SCAN_RSP`, 31 bytes each;
- after the extended commands, LE Extended Advertising Reports: a legacy PDU
  when the data fits 31 bytes, otherwise an extended one, in fragments of
  229 bytes.

The AD structures are rebuilt from CoreBluetooth's dictionary: service
UUIDs (16, 32 and 128 bits), local name, TX power, solicited UUIDs, service
data and manufacturer data. Flags are lost. The address is a random static
address derived from the peripheral's `NSUUID`. Duplicate filtering is the
controller's, as the host asked.

### Connections (central)

`LE (Extended) Create Connection` connects to the peer address, or to any
device on the Filter Accept List, as the Android stack's LE connections
always do.

- A device already seen in a scan is connected at once
  (`connectPeripheral`).
- Otherwise the controller scans until it advertises.
- Once connected, the device's whole GATT tree is discovered (services,
  characteristics, descriptors), and only then is the LE (Enhanced)
  Connection Complete sent. So the ATT server can answer from the first
  request.
- The reported interval, latency and timeout are the host's requested
  maximum; macOS actually chooses them.
- Other commands:
  - `LE Connection Update` gets *Unacceptable Connection Parameters*.
  - `LE Set PHY` reports LE 1M.
  - `Read Remote Version Information` completes with *Unsupported Remote
    Feature*.
  - `LE Read Remote Features` reports no feature.
  - `Disconnect` cancels the connection.
- Every ACL packet from the host is acknowledged at once with Number Of
  Completed Packets. Toward the host, frames are split into 251-byte ACL
  packets.

### ATT over GATT

CoreBluetooth offers GATT, not ATT. `att::Bearer` plays the device's ATT
server over its GATT tree (`gatt::Db`).

- **Handles.** Each service is a declaration followed by its
  characteristics: declaration, value, then descriptors. The numbering
  depends only on the discovered tree, so it is stable across
  reconnections, which the stack's GATT cache relies on.
- **GAP and GATT services.** CoreBluetooth hides Generic Access and Generic
  Attribute, so both are rebuilt.
  - The Device Name comes from `CBPeripheral.name`, which macOS reads from
    the device's own Device Name characteristic.
  - Service Changed is indicated when CoreBluetooth reports modified
    services; the tree is then discovered again and the database replaced.
- **Discovery** (Read By Group Type, Find By Type Value, Read By Type of
  declarations, Find Information) is answered locally.
- **Remote operations.**
  - Reads, Read By Type of a value, writes, Write Commands and long writes
    (Prepare/Execute, assembled into one write) become `readValue` and
    `writeValue` operations.
  - Read Blob continues from the value the last read returned, since
    CoreBluetooth reads whole values.
  - One request is outstanding at a time, as ATT requires; later requests
    wait.
- **CCCDs.** A CCCD is held in the bearer. Writing it calls
  `setNotifyValue`, because CoreBluetooth forbids writing CCCDs.
  Notifications and indications go out as the client configured them;
  indications wait for their confirmation.
- **Errors.** ATT errors from the device (`CBATTErrorDomain`) pass through;
  any other error is *Unlikely Error*.
- **MTU.** Exchange MTU reports the MTU macOS negotiated
  (`maximumWriteValueLength` + 3).
- **Other channels.**
  - The Security Manager answers Pairing Request with *Pairing Not
    Supported*: macOS pairs when a protected attribute needs it.
  - The LE signaling channel refuses credit-based channels (*SPSM not
    supported*) and parameter update requests.

### Advertising (peripheral role)

There is one advertising set, legacy or extended. When it is enabled, its
data and scan response are parsed and `CBPeripheralManager` advertises
what it can: the local name and the service UUIDs. The advertising
parameters, other AD types and connections to the Mac as a peripheral are
not mapped.

## Permission

CoreBluetooth asks for the Bluetooth permission (TCC) on first use,
attributed to the app responsible for the process (the terminal, for
`linux-run` started from one). Until the user answers, CoreBluetooth
reports no state and the controller sees no devices; the HCI dialogue still
works. When access is denied, the host module logs it once.

## The stack's needs from the syscall layer

The Bluetooth stack is `libbluetooth_jni.so`, loaded by the Bluetooth app,
so it needs ART and system_server (P2/P3). What it needs from the kernel,
from its imports:

- sockets: AF_UNIX stream, seqpacket and socketpairs for the app sockets;
  no `AF_BLUETOOTH`, no netlink;
- epoll, eventfd, timerfd, pipes;
- POSIX timers (`timer_create` and related), used by `osi/alarm`, are
  still missing: the syscall layer answers `ENOSYS`;
- `sched_setscheduler`, `prctl`, `getrandom`;
- no `/dev` node besides binder: the HCI goes through the AIDL HAL.

The library is built with the shadow call stack. It loads through the
original linker with its dependencies' constructors run; the load-time
rewrite turns 34,541 SCS instructions. Loading it needs hwservicemanager,
because constructors of its HIDL dependencies wait for
`hwservicemanager.ready`.

## Verified (2026-09-28)

- **Unit tests.** `cargo test -p darwin-host-bluetooth`: 27 tests.
  - HCI encodings and the Supported Commands bitmap.
  - The controller against a fake CoreBluetooth backend: the stack's
    start-up dialogue, legacy and extended scanning, direct and accept-list
    connections, cancel, reset, disconnect, advertising.
  - ATT to GATT translation: discovery, reads, blobs, writes, long writes,
    CCCDs, notifications, indications, Service Changed and errors.
  - L2CAP fragmentation, reassembly, signaling and the Security Manager.
- **End to end.** `cargo test -p darwin-linux-abi --test bluetooth`:
  - on the derived image, the original servicemanager, our HAL, and the
    test client over the AIDL interface: initialize, reset, version (HCI
    5.3), address, LE-only features, an extended scan;
  - then `libbluetooth_jni.so` loads with `JNI_OnLoad`, under the original
    hwservicemanager.

  Real advertisements and the GATT Device Name read need the Bluetooth
  permission granted for the terminal. Without it the scan reports no
  devices, which the test reports but does not fail on.

## Not done

- BR/EDR (classic audio needs a USB controller path, ADR 0012).
- LE credit-based channels (`CBPeripheral.openL2CAPChannel`).
- Connections to the Mac as a peripheral (a GATT client toward the stack's
  GATT server over `CBPeripheralManager`).
- LE Audio and ISO.
- Scan duration timeouts: `LE Set Extended Scan Enable` with a duration
  scans until it is disabled.
- The Android stack itself, which waits for P3.
