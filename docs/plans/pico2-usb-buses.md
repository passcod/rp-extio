# Pico 2 USB I2C and SPI bridge

## Goal and scope

Expose the buses of a USB-connected Pico 2 as ordinary Linux `/dev/i2c-*` and
`/dev/spidev*` devices. I2C and SPI are the first required interfaces. UART
through CDC ACM is third; GPIO through `/dev/gpiochip*` is fourth. Other RP2350
peripherals need separate requirements. The initial pin map is Pico 2 GP4/GP5
for I2C0, GP18/GP19/GP16 for SPI0 SCK/MOSI/MISO, and GP17/GP21 for two
software-controlled chip selects. Keep the protocol and bus logic independent
of this board map so other RP2350 boards and later MCU families can supply
board-specific implementations.

Rust owns firmware, wire handling, USB discovery, the host daemon, and the
simulated backend. The daemon is a normal runtime component. A simulated
backend must register the same Linux bus nodes as the hardware backend and
provide scripted peripheral responses for Raspberry Pi software tests. The
premature C prototype already in this repository is not the implementation
baseline; remove or replace it when implementation begins.

## Completed design work: kernel boundary

The kernel component will be a small C module. On 2026-10-09, upstream Linux
Rust sources had a miscdevice abstraction, but their I2C abstraction registered
I2C *client drivers*, and no Rust SPI controller abstraction existed. A minimal
out-of-tree Rust module prototype also failed to link against the installed
7.2.6 kernel headers: the available Rust compiler could not read their
prebuilt Rust crate metadata, despite a matching version number. A mixed
Rust/C module would retain the C bus registration code and add this exact
compiler dependency. Revisit this conclusion if kernel APIs and packaging
change, but do not make the first bridge depend on them.

The C module owns only Linux I2C/SPI registration, SPI child devices, a
miscdevice request channel, bounded copying between kernel and daemon,
callback wait/cancellation, and teardown. It does not speak USB, parse board
capabilities, implement mock behavior, or contain firmware protocol logic.
`io_uring` and eBPF cannot replace registration of `i2c_adapter` and
`spi_controller` objects. The daemon may use ordinary async I/O for its own
work; the kernel boundary is a miscdevice because its read/write/poll/ioctl
semantics fit synchronous bus callbacks and daemon lifecycle.

Target Linux 6.12 LTS through the current 7.2 series, including current
Raspberry Pi OS kernels in that range. Keep version-specific C compatibility
at the kernel API boundary. The module uses DKMS and standard running-kernel
headers, without a Rust compiler requirement. The daemon is a Rust binary and
may run on x86-64 or Raspberry Pi Linux hosts.

### Kernel-to-daemon ABI

Register `/dev/rp-extio-control` as a single-owner miscdevice. The daemon
opens it once. Fixed-layout ioctls with explicit-width fields obtain ABI
version 1, create an I2C or SPI bus, and destroy a bus. Each create call
returns an instance ID and the allocated Linux bus number. It also supplies
validated capabilities: I2C functionality and speed, or SPI chip-select count,
mode bits, and speed range. Registration is tied to the open file; closing it
marks every instance offline, wakes all callers with `-ENODEV`, and unregisters
its controllers. A reconnect creates new instances and may receive new Linux
bus numbers.

The daemon reads one complete request per `read()` and writes one complete
reply per `write()`. A short user buffer returns `-EMSGSIZE` without removing
the request; partial frames are never accepted. `poll()` indicates queued
requests. Each frame has a 32-byte little-endian header: four-byte magic
`RPKD`, `u16` major, `u16` operation, `u32` instance ID, `u32` payload length,
`u64` request ID, `i32` status, and `u32` reserved. Major is 1, reserved is 0,
and request status is 0. The payload is at most 4096 bytes. Fixed ioctl
structures use explicit-width and aligned 64-bit fields so 32-bit daemon
processes have the same layout. The request ID is unique within one open
session; a response with the wrong ID or length is rejected and cannot satisfy
another callback. Reply status uses the bounded bus-error classes defined
below; the kernel maps those classes to Linux errnos and rejects arbitrary
status values from userspace.

The I2C and SPI cores serialize work per controller; allow one outstanding
request per bus and concurrent requests on different buses. The kernel wait
limit is eight seconds. A timed-out request is removed, and a later reply for
that ID is discarded. A timeout cannot undo an operation already started on
the remote bus. The daemon owns one file descriptor; a second opener gets
`-EBUSY`. The control device is root-owned with mode 0600 by default. Test
processes configure simulations through the daemon rather than opening the
kernel control device.

## Completed design work: bus semantics and USB protocol

Use two separately versioned protocols. The kernel ABI carries Linux bus
requests to the daemon; the USB protocol carries board-neutral transactions
to firmware. The daemon translates between them. Both are little-endian,
bounded, and explicitly framed. Neither assumes that one USB packet equals one
message. Unknown major versions fail before bus registration; unknown minor
extensions are ignored only when their length and reserved bits permit it.

### USB framing and capabilities

The Pico 2 presents one vendor-specific USB interface with one bulk OUT and
one bulk IN endpoint. A USB frame has a 24-byte header: four-byte magic
`RPXB`, `u8` major, `u8` minor, `u8` operation, `u8` flags, `u64` request ID,
`u32` payload length, and `i32` status. Version starts at 1.0. Requests have
status 0; replies echo operation and ID. Flags distinguish request from reply;
all other flag bits are reserved. Payload length is at most 4096 bytes. The
receiver accumulates endpoint packets until the declared length is present
and rejects extra bytes, malformed fields, and unknown operations. USB already
provides packet integrity, so this framing adds no second checksum.

Operations are `HELLO` (1), `I2C_TRANSFER` (2), and `SPI_MESSAGE` (3). Reply
frames set flag bit 0; all other bits are zero. Status values are `OK` (0),
`NACK` (1), `ARBITRATION_LOST` (2), `TIMEOUT` (3), `DISCONNECTED` (4),
`UNSUPPORTED` (5), `BAD_FRAME` (6), and `IO_ERROR` (7). `HELLO` returns
version range, protocol limits, and capability records encoded as `u16 type`,
`u16 length`, and value bytes. The general record contains maximum payload;
each bus record contains bus ID, supported flags, speed range, and
chip-select count where relevant. Unknown record types can be skipped. The USB
serial string is derived from the Pico 2's stable board/flash ID. The daemon
registers Linux buses only after matching the protocol version and capabilities.
Use a local development USB ID until an allocated VID/PID is available.

At most one request is active per USB device. The daemon serializes physical
operations on that device, although separate Linux buses can have pending
requests. A physical disconnect or uncertain USB framing tears down its Linux
bus instances; no non-idempotent transaction is retried automatically. Every
error response names a protocol error class and, where meaningful, the first
failed I2C message or completed SPI byte count. The daemon maps these to
`-EREMOTEIO` for NACK, `-EAGAIN` for arbitration loss, `-ETIMEDOUT` for bus
or transport timeout, `-ENODEV` for disconnect, `-EOPNOTSUPP` for an
unsupported operation, and `-EIO`/`-EPROTO` for other hardware/protocol
failures. The firmware operation deadline is five seconds, the daemon USB
deadline six seconds, and the kernel callback deadline eight seconds.

### I2C contract

An I2C request contains `u8` bus ID, `u8` message count, `u16` reserved,
then up to 16 message descriptors. Each descriptor contains `u16` address,
`u16` flags, `u16` length, `u16` reserved, and write bytes when applicable.
A successful reply concatenates read bytes in message order and reports the
number of completed messages.
The sum of data bytes in either direction is at most 3584. The firmware must
execute one Linux transfer atomically on the physical bus, with START before
each message, repeated START between messages, and one STOP at the end. It
supports 7-bit and 10-bit addresses, both reads and writes, zero-length
messages, clock stretching, and arbitration/error reporting. It accepts
`I2C_M_RD` and `I2C_M_TEN`; `I2C_M_DMA_SAFE` is a kernel buffer hint and is
not sent over USB. `RECV_LEN`, `NOSTART`, `STOP`, and other protocol-mangling
flags are rejected, not approximated. Advertise `I2C_FUNC_I2C`,
`I2C_FUNC_10BIT_ADDR`, and `I2C_FUNC_SMBUS_EMUL`; do not advertise
`I2C_FUNC_SMBUS_READ_BLOCK_DATA` or other modes needing `RECV_LEN`.

The Pico 2 bus runs at 100 kHz initially. Embassy's public RP2350 I2C API
covers single messages and same-address write-then-read. The RP2350 I2C target
address register cannot change while its controller is enabled; that API
cannot preserve arbitrary cross-address repeated starts. The firmware must
therefore have a Rust open-drain software I2C executor for general message
sequences and use the hardware controller only for cases whose exact START
and STOP behavior is confirmed. Select the executor before each transaction,
and switch pin function only while the bus is idle. Both executors use the
same advertised 100 kHz rate and bus deadline. The software executor waits
for SCL to rise to honor clock stretching, detects arbitration loss when
releasing SDA, and performs bounded bus recovery. No transfer is silently
split by a STOP.

### SPI contract

An SPI request contains `u8` bus ID, `u8` chip select, `u16` mode,
`u8` segment count, and three reserved bytes, then up to 16 segments. Each
segment contains `u32` requested speed, `u16` byte length, `u16` flags
(including `cs_change`), three `u32` delays in nanoseconds for word,
post-transfer, and chip-select-change timing, and TX bytes (zero fill when
Linux provided no TX buffer). RX bytes for every segment, the completed byte
count, and effective speed per segment come back in the reply. Total TX and RX data each
have a 3584-byte limit, leaving room for metadata inside the 4096-byte frame.
The kernel advertises these limits through controller callbacks.

The Pico 2 implementation offers two software GPIO chip selects, SPI modes
0–3, 8-bit words, MSB or software-reversed LSB order, full duplex, 50 kHz to
20 MHz requested clocks, active-high or active-low select, and no-select mode.
Firmware holds select across segments, toggles it for an intermediate
`cs_change`, and applies requested delays, rounding up to its timer
resolution. Cap cumulative requested delays at two seconds per message.
Final-segment `cs_change` is a Linux hint; firmware deasserts select at
message completion. Reject three-wire, multi-lane, non-eight-bit words,
loopback, and other unsupported flags before shifting any bytes. Do not split
a Linux SPI message across USB requests because doing so would weaken the
chip-select and atomic-message contract.

## Implementation handoff

### 3. Rust firmware

Create a `no_std` wire crate with the USB frame parser, capability records,
error classes, and bounded I2C/SPI payload types. Create a separate Pico 2
firmware crate using verified published versions of Embassy RP and Embassy
USB, targeting RP2350 Arm and producing a UF2. Keep pins, bus setup, and
hardware-specific I2C/SPI executors in the board layer; the wire dispatcher
uses board-neutral transaction traits. The firmware owns static request and
response buffers, the vendor bulk endpoints, a unique USB serial, I2C bus
recovery, SPI chip-select lifetime, and cancellation on disconnect. Implement
the open-drain I2C path for full Linux message sequences and use Embassy's
hardware drivers only where their public behavior matches this contract.

### 4. C kernel endpoint

Replace the premature USB-facing module with a module containing the minimal
control miscdevice, I2C adapter callbacks, SPI controller callbacks, SPI child
device registration, request queue, and lifecycle code. Put the ABI constants
and fixed-layout ioctl structures in one shared UAPI header consumed by the
Rust daemon. Provide the DKMS manifest and only the compatibility code needed
for the supported kernel range. Create `spidev` children for the Pico 2 chip
selects by default; document how an administrator can unbind one and attach a
specific SPI driver. The module never opens USB or reads simulation files.

### 5. Rust daemon and backends

Create one Rust daemon with a dispatcher over a `Backend` interface for
capabilities, I2C transfers, SPI messages, and disconnect. Its USB backend
uses `nusb` for Linux bulk transfers and hotplug discovery, matches the
versioned `HELLO` response, and registers/removes kernel bus instances. Its
simulated backend uses the same dispatcher and kernel instances, with a
Unix-domain control socket that accepts JSON scenario commands to create
buses, attach I2C addresses or SPI chip selects, enqueue exact transaction
expectations and responses, inject bus errors, inspect observed requests,
and reset state. Unexpected simulated transactions fail explicitly and are
recorded. Keep scenario state per virtual device so independent tests do not
share peripheral state. The daemon handles request IDs, USB timeouts,
firmware errors, hot unplug, and orderly kernel instance teardown.

## Later interfaces and integration

Document stable device identity, USB permissions, service startup, reconnect
behavior, DKMS install/removal, Pico 2 flashing, pin wiring, 3.3 V limits,
external I2C pull-ups, and examples using `i2c-dev` and `spidev`. Add UART
through USB CDC ACM so Linux's existing driver provides `/dev/ttyACM*`;
report it as an optional board capability. Add GPIO through a Linux GPIO
controller and `/dev/gpiochipN`, including direction, values, edge events,
and simulated input changes. GPIO line ownership must exclude pins assigned
to I2C, SPI, or UART. `/dev/gpiomem` exposes Raspberry Pi memory-mapped
registers and cannot directly represent GPIO on a USB-attached board.

USB full-speed and request latency make this a functional development bridge,
not a timing-equivalent replacement for a local Raspberry Pi controller.
