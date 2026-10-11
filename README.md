# rp-as-extio

`rp-as-extio` exposes I2C, SPI, UART, and GPIO from a USB-connected Raspberry
Pi Pico 2 through ordinary Linux interfaces. A simulator registers the same
I2C, SPI, and GPIO devices for tests that should not require hardware.

This repository contains four cooperating pieces:

- RP2350 firmware with a vendor-specific USB bulk interface
- a small out-of-tree Linux module that registers I2C, SPI, and GPIO controllers
- `rp-extiod`, which discovers boards and dispatches kernel requests over USB
- a newline-delimited JSON simulator control socket in `rp-extiod`

The development USB identity is `cafe:4010`. It must be replaced before a
distributed product uses an allocated VID/PID.

## Hardware and electrical limits

The startup map enables instance 0 of each peripheral and remembers
conflict-free routes for the disabled instance 1 peripherals:

| Peripheral | Startup state | Pico 2 GPIOs |
| --- | --- | --- |
| UART0 | enabled | TX GP0, RX GP1 |
| UART1 | disabled | TX GP8, RX GP9 |
| I2C0 | enabled | SDA GP4, SCL GP5 |
| I2C1 | disabled | SDA GP10, SCL GP11 |
| SPI0 | enabled | MISO GP16, SCK GP18, MOSI GP19, CS0 GP17, CS1 GP21 |
| SPI1 | disabled | MISO GP12, SCK GP14, MOSI GP15, CS0 GP13, CS1 GP20 |
| GPIO | 17 lines | GP2, GP3, GP6–GP15, GP20, GP22, GP26–GP28 |

Routes and enabled states can be changed at runtime. A disabled peripheral
keeps its remembered routes but reserves no pins. Every unassigned Pico 2
header GPIO becomes a Linux GPIO line in ascending pin order, so the controller
has between 8 lines when all six peripherals are enabled and 26 lines when all
are disabled. The firmware reports the generated mapping to the daemon and
kernel after every change.

All signals are 3.3 V only. Do not connect 5 V logic directly. I2C SDA and SCL
need external pull-ups to 3.3 V; 2.2–4.7 kΩ is a common starting range, but the
correct value depends on bus capacitance and attached devices. Connect a ground
between the Pico 2 and every peripheral.

See [the hardware verification guide](docs/hardware-verification.md) for
breadboard circuits, recommended I2C and SPI peripherals, GPIO LED/button and
loopback tests, Raspberry Pi UART-console wiring, lifecycle testing, and ADC
and PWM stretch fixtures.

I2C initially runs at 100 kHz. SPI accepts 50 kHz through 20 MHz, modes 0–3,
8-bit words, MSB- or LSB-first transfers, active-high or active-low chip select,
and no-CS transfers. USB full-speed latency means this is a functional remote
bus for development and integration tests, not a timing-equivalent substitute
for a local controller.

UART0 and UART1 are two permanent USB CDC ACM functions and appear through
Linux's `cdc_acm` driver as separate `/dev/ttyACM*` devices. Both use 115200
baud, eight data bits, no parity, and one stop bit. Set the host terminal to the
same format; CDC line-coding requests do not reconfigure the RP2350 UART. When
an instance is disabled its ACM function remains enumerated, host output is
discarded, no physical UART traffic is accepted, and its pins become GPIOs.

With the startup map, the GPIO controller exposes 17 lines. GPIO line offsets
map to physical pins as follows; pins assigned to enabled UART, I2C, and SPI
instances are deliberately absent. Re-read `gpioinfo` after every map change
because both the count and offsets can change:

| Line | Pin | Line | Pin | Line | Pin |
| ---: | --- | ---: | --- | ---: | --- |
| 0 | GP2 | 6 | GP10 | 12 | GP20 |
| 1 | GP3 | 7 | GP11 | 13 | GP22 |
| 2 | GP6 | 8 | GP12 | 14 | GP26 |
| 3 | GP7 | 9 | GP13 | 15 | GP27 |
| 4 | GP8 | 10 | GP14 | 16 | GP28 |
| 5 | GP9 | 11 | GP15 | | |

## Build and flash the firmware

Install the Rust target and `picotool`, then build the UF2:

```sh
rustup target add thumbv8m.main-none-eabihf
./scripts/build-uf2.sh
```

To build, flash, verify, and reboot the attached Pico 2 in one command:

```sh
just flash
```

This works immediately when the board is in BOOTSEL mode. This firmware also
exposes Raspberry Pi's standard USB reset interface, so `picotool` can reboot a
running board into BOOTSEL automatically for later flashes.
The firmware returns to BOOTSEL if it panics during startup or operation, so a
bad image remains recoverable over USB.

One firmware image supports every valid route at runtime. Pin environment
variables only change the startup and recovery default. For example, this
builds a recovery image whose default I2C0 route is GP12/GP13:

```sh
RP_EXTIO_I2C0_SDA=12 RP_EXTIO_I2C0_SCL=13 ./scripts/build-uf2.sh
```

Supported variables and defaults are:

| Variable | Default | Valid Pico 2 GPIOs |
| --- | ---: | --- |
| `RP_EXTIO_UART0_TX` | 0 | 0, 2, 12, 14, 16, 18, 28 |
| `RP_EXTIO_UART0_RX` | 1 | 1, 3, 13, 15, 17, 19 |
| `RP_EXTIO_UART1_TX` | 8 | 4, 6, 8, 10, 20, 22, 26 |
| `RP_EXTIO_UART1_RX` | 9 | 5, 7, 9, 11, 21, 27 |
| `RP_EXTIO_I2C0_SDA` | 4 | 0, 4, 8, 12, 16, 20 |
| `RP_EXTIO_I2C0_SCL` | 5 | 1, 5, 9, 13, 17, 21 |
| `RP_EXTIO_I2C1_SDA` | 10 | 2, 6, 10, 14, 18, 22, 26 |
| `RP_EXTIO_I2C1_SCL` | 11 | 3, 7, 11, 15, 19, 27 |
| `RP_EXTIO_SPI0_MISO` | 16 | 0, 4, 16, 20 |
| `RP_EXTIO_SPI0_SCK` | 18 | 2, 6, 18, 22 |
| `RP_EXTIO_SPI0_MOSI` | 19 | 3, 7, 19 |
| `RP_EXTIO_SPI0_CS0` | 17 | any Pico 2 header GPIO |
| `RP_EXTIO_SPI0_CS1` | 21 | any Pico 2 header GPIO |
| `RP_EXTIO_SPI1_MISO` | 12 | 8, 12, 28 |
| `RP_EXTIO_SPI1_SCK` | 14 | 10, 14, 26 |
| `RP_EXTIO_SPI1_MOSI` | 15 | 11, 15, 27 |
| `RP_EXTIO_SPI1_CS0` | 13 | any Pico 2 header GPIO |
| `RP_EXTIO_SPI1_CS1` | 20 | any Pico 2 header GPIO |
| `RP_EXTIO_UART0_ENABLED` | true | boolean |
| `RP_EXTIO_UART1_ENABLED` | false | boolean |
| `RP_EXTIO_I2C0_ENABLED` | true | boolean |
| `RP_EXTIO_I2C1_ENABLED` | false | boolean |
| `RP_EXTIO_SPI0_ENABLED` | true | boolean |
| `RP_EXTIO_SPI1_ENABLED` | false | boolean |
| `RP_EXTIO_SPI0_CS1_ENABLED` | true | boolean |
| `RP_EXTIO_SPI1_CS1_ENABLED` | true | boolean |

The old unqualified instance-0 names, such as `RP_EXTIO_I2C_SDA`, remain
aliases when the corresponding `RP_EXTIO_I2C0_SDA` variable is unset.

Each I2C SDA/SCL route must be a matching pair. Remembered routes must always
be valid for their hardware block, while duplicate pins are permitted only
between disabled peripherals. Invalid enabled conflicts fail the firmware
build with a diagnostic.
The daemon normally replaces this map after USB negotiation and reapplies its
persistent map after every reconnect or firmware reset.

Alternatively, hold BOOTSEL while connecting the Pico 2 and copy
`build/rp-extio-firmware.uf2` to the `RP2350` mass-storage volume. The firmware
uses the RP2350 OTP chip ID as its USB serial number. That serial is the stable
board identity; `/dev/i2c-N`, `/dev/spidevN.*`, and `/dev/gpiochipN` numbers can
change after a daemon or board reconnect.

## Install the Linux module

With DKMS and the running kernel's headers installed, build and install the
module, release binaries, and systemd unit with:

```sh
just install
just enable
```

`just install` does not enable or start the service. `just enable` performs the
same installation and then enables and starts it. To replace an already loaded
development module after rebuilding, use `just restart`; it stops the daemon,
unloads the module without force, and starts the service again.

The recipe refreshes this version's DKMS build for the running kernel. The
module's DKMS configuration enables automatic rebuilds after kernel upgrades.

Install the compiler, make, DKMS, and headers for the running kernel. To stage
version 0.1.0 for DKMS:

```sh
runzero install -d /usr/src/rp-extio-0.1.0
runzero install -m 0644 linux/Makefile linux/dkms.conf \
  linux/rp_extio.c linux/rp_extio_uapi.h /usr/src/rp-extio-0.1.0/
runzero dkms add -m rp-extio -v 0.1.0
runzero dkms build -m rp-extio -v 0.1.0
runzero dkms install -m rp-extio -v 0.1.0
```

The service preparation step below loads the installed module and creates its
control device.

To remove this version:

```sh
systemctl disable --now rp-extiod.service
runzero modprobe -r rp_extio
runzero dkms remove -m rp-extio -v 0.1.0 --all
runzero rm -r /usr/src/rp-extio-0.1.0
```

## Install and run the daemon

Build and install the daemon and service:

```sh
cargo build --release -p rp-extiod
runzero install -m 0755 target/release/rp-extiod /usr/local/bin/rp-extiod
runzero install -m 0755 target/release/rp-extioctl /usr/local/bin/rp-extioctl
runzero install -m 0644 dist/rp-extiod.service \
  /etc/systemd/system/rp-extiod.service
systemctl daemon-reload
systemctl enable --now rp-extiod.service
```

The service runs `rp-extioctl prepare` before starting the daemon, so service
startup loads the module and repairs a missing control-device node without a
separate manual step.
`/dev/rp-extio-control` is owned by root with mode `0600`, and one daemon may
open it. Closing it unregisters every I2C, SPI, and GPIO instance and wakes
outstanding bus operations with `ENODEV`.

For a manual start, or a development build that has not been installed with
DKMS, prepare the control device explicitly:

```sh
runzero rp-extioctl prepare

make -C linux
runzero target/debug/rp-extioctl prepare --module linux/rp_extio.ko
```

The daemon watches for matching USB devices, negotiates protocol version and
capabilities before registering buses, and supports multiple boards. It permits
one physical transaction at a time per board. Independent boards and simulated
devices can operate concurrently.

The installed service runs as root because `/dev/rp-extio-control` is mode
`0600`; this also gives it access to the Pico through Linux usbfs without a
separate udev rule. A direct daemon invocation needs equivalent permissions for
both the control device and USB device. The CDC ACM interface keeps the normal
distribution-provided serial-device permissions.

A disconnect, USB timeout, mismatched response, or uncertain frame tears down
that board's Linux buses. Non-idempotent transactions are never retried. A
reconnected board is negotiated and registered again, potentially under new
Linux bus numbers. The firmware bus deadline is five seconds, the daemon USB
deadline is six seconds, and the kernel callback deadline is eight seconds.

## Configure pin routing

`rp-extiod` listens on `/run/rp-extiod.sock` and persists desired maps in
`/etc/rp-extio/config.json`. The service creates the configuration directory.
Direct invocations can select alternatives with `--socket` and `--config`.
`--strict-config` rejects startup if the configuration names a board that is
not currently connected. Without it, maps for disconnected boards remain
available and are applied when their serial numbers appear.

List boards and inspect the active map:

```sh
rp-extioctl list
rp-extioctl show 125DBCCF0BCA4BEF
```

Change any subset of signals. The client fetches the active complete map,
applies the requested edits, validates it, and submits it with the observed
generation:

```sh
rp-extioctl set 125DBCCF0BCA4BEF --i2c0-sda GP12 --i2c0-scl GP13
rp-extioctl set 125DBCCF0BCA4BEF \
  --spi1-miso GP8 --spi1-sck GP10 --spi1-mosi GP11 \
  --spi1-cs0 GP14 --spi1-cs1 GP15 --enable-spi1
rp-extioctl set 125DBCCF0BCA4BEF --disable-uart0
rp-extioctl set 125DBCCF0BCA4BEF --disable-spi0-cs1
```

Every route option is instance-qualified. The old unqualified options such as
`--i2c-sda` and `--spi-miso` remain aliases for instance 0. Each of `uart0`,
`uart1`, `i2c0`, `i2c1`, `spi0`, and `spi1` has matching `--enable-*` and
`--disable-*` options. Routes can be edited while an instance is disabled;
enabling it later atomically rejects any conflict with the active instances.
Each SPI instance also has `--enable-spiN-cs1` and `--disable-spiN-cs1`.
Disabling CS1 leaves CS0 and the controller active, removes the second spidev
child, and returns the remembered CS1 pin to GPIO.

Every accepted change removes and recreates that board's enabled Linux I2C and
SPI instances and its GPIO controller, so rediscover their numbers and GPIO
offsets afterward. Disabled I2C and SPI instances have no Linux bus. A stale
generation is rejected instead of overwriting a concurrent edit. Firmware
validates the map before touching hardware; an invalid route or conflict leaves
the prior map active.

Use JSON responses in scripts, restore the firmware default, or launch the
interactive editor:

```sh
rp-extioctl --json list
rp-extioctl default 125DBCCF0BCA4BEF
rp-extioctl reload
rp-extioctl tui 125DBCCF0BCA4BEF
```

The TUI presents the physical Pico 2 headers with pin 1 opposite pin 40 at the
USB end. Arrow keys move between physical pins, Tab and Shift-Tab select any of
the 18 peripheral signals, and Enter or Space assigns that signal to the
highlighted GPIO in the draft. Assignments move the selected hardware
instance's UART TX/RX, I2C SDA/SCL, or SPI MISO/SCK/MOSI mux group together by
default. A grouped move that would conflict with another enabled signal or
cannot fit on the exposed header falls back to moving only the selected signal
when that still produces a valid map; otherwise it is rejected without changing
the draft. SPI chip selects remain individually assignable.

Press `g` to switch between grouped and individual assignment. Individual mode
allows unusual valid layouts without a complete exposed group, and can leave a
temporarily invalid draft for manual correction. Valid routes are highlighted;
conflicts, mismatched I2C pairs, changed pins, exported GPIO lines, power pins,
grounds, and current Linux bus assignments remain visible on the board.

`e` toggles the selected signal's entire peripheral instance, except that a
selected SPI CS1 toggles only that optional second chip select. Disabling an
instance or CS1 immediately previews its pins as GPIO; enabling validates all
of its remembered routes before changing the draft. `f` mirrors the diagram
left-to-right for a board mounted belly up, keeping USB at the top while showing
the pin headers as viewed from the back. Arrow movement remains aligned with the
screen.
Orientation is not persisted.

`a` applies the complete draft atomically, `d` loads the firmware defaults into
the draft, and `u` discards edits and restores the active map. `[` and `]`
select a board, `r` refreshes, `l` reloads the persistent file, and `q` exits.
Short terminals scroll the physical pin rows around the cursor, while wide
terminals also show a complete signal legend with enabled-state markers.

The persistent file is JSON and always contains complete maps:

```json
{
  "version": 2,
  "boards": {
    "125DBCCF0BCA4BEF": {
      "uart0_enabled": true, "uart0_tx": 0, "uart0_rx": 1,
      "uart1_enabled": false, "uart1_tx": 8, "uart1_rx": 9,
      "i2c0_enabled": true, "i2c0_sda": 12, "i2c0_scl": 13,
      "i2c1_enabled": false, "i2c1_sda": 10, "i2c1_scl": 11,
      "spi0_enabled": true,
      "spi0_miso": 16, "spi0_sck": 18, "spi0_mosi": 19,
      "spi0_cs0": 17, "spi0_cs1_enabled": false, "spi0_cs1": 21,
      "spi1_enabled": false,
      "spi1_miso": 12, "spi1_sck": 14, "spi1_mosi": 15,
      "spi1_cs0": 13, "spi1_cs1_enabled": true, "spi1_cs1": 20
    }
  }
}
```

Version-1 files are loaded by placing their routes on instance 0 and retaining
the disabled instance-1 defaults. The next successful save writes version 2.
Version-2 maps without the CS1 enable fields load with both second chip selects
enabled.

This release uses USB protocol major 2 and kernel ABI major 2. Rebuild and
install the module and daemon, and flash the matching firmware, as one upgrade;
the daemon rejects a version-1 module or firmware instead of registering an
incompatible device.

The socket is mode `0666`, but access is enforced per request using the Unix
peer credentials supplied by the kernel. Any local process can use `list`,
`show`, and the read-only parts of the TUI. `set`, `default`, `reload`, and TUI
actions that apply a change ask Polkit for `org.rp-extio.configure`; an active
desktop session therefore receives its normal authentication dialog. A session
without a Polkit authentication agent receives an authorization error instead.

Root and members of the group selected with `rp-extiod --operator-group GROUP`
may mutate configuration without a Polkit prompt. Group membership is checked
by the daemon from the connecting process's kernel credentials and
supplementary groups; it does not grant access to usbfs or
`/dev/rp-extio-control`.

Inspect assigned bus numbers with:

```sh
i2cdetect -l
ls -l /dev/spidev*
gpioinfo
journalctl -u rp-extiod.service
```

## Use the buses

Replace `N` with the adapter number shown by `i2cdetect -l`. For example, read
two bytes from register `0x0f` on a device at address `0x50`:

```sh
i2ctransfer -y N w1@0x50 0x0f r2
```

The module creates a `spidev` child for each chip select. Replace `N` with the
SPI controller number:

```sh
spidev_test -D /dev/spidevN.0 -s 1000000 -p 'hello'
```

An administrator can unbind a default `spidev` child and bind a specific kernel
SPI device driver in the usual sysfs manner. The device disappears on bridge
disconnect, so drivers must tolerate removal and reprobe.

Find the gpiochip whose label is `rp-extio virtual GPIO`, then use its line
offsets with libgpiod tools. For example:

```sh
gpioinfo /dev/gpiochipN
gpioget /dev/gpiochipN 0
gpioset /dev/gpiochipN 0=1
gpiomon --edges=both /dev/gpiochipN 0
```

GPIO values and directions are transported synchronously. Edge changes from a
physical Pico 2 are sampled every 10 ms in one USB snapshot, so pulses shorter
than the sampling interval can be missed. This is suitable for functional
development and integration tests, not precise edge timing or pulse counting.

## Simulator

Start the daemon with a Unix control socket in addition to USB discovery:

```sh
runzero /usr/local/bin/rp-extiod --simulate /run/rp-extiod-sim.sock
```

Commands and responses are one JSON object per line. Device state is isolated
by the `device` name. Creating a device returns the allocated Linux bus numbers:

```json
{"command":"create","device":"test-1"}
{"result":"created","i2c_buses":[12,null],"spi_buses":[7,null]}
```

Queue exact expected transactions and their replies before software accesses
the bus:

```json
{"command":"expect","device":"test-1","transaction":{"bus":"i2c","bus_id":0,"messages":[{"address":80,"flags":0,"data":[15],"read_len":0},{"address":80,"flags":1,"data":[],"read_len":2}]},"reply":{"status":0,"completed":2,"data":[18,52]}}
```

Unexpected transactions are recorded and fail with an I/O error. Inspect or
clear a scenario with:

```json
{"command":"observed","device":"test-1"}
{"command":"reset","device":"test-1"}
{"command":"remove","device":"test-1"}
```

The simulated GPIO starts with all lines low and configured as inputs. Inject
an input transition by line offset; this updates reads and emits a Linux edge
event when a consumer has requested the matching edge:

```json
{"command":"set_input","device":"test-1","line":0,"value":1}
```

`set_input` rejects output lines. `reset` also restores every line to a low
input in addition to clearing expected and observed bus transactions.

Reply status values are `0` OK, `1` NACK, `2` arbitration lost, `3` timeout,
`4` disconnected, `5` unsupported, `6` bad frame, `7` I/O error, `8` invalid
route, and `9` pin conflict. SPI
expectations use `chip_select`, `mode`, and a `segments` array; each segment has
`speed_hz`, `flags`, `word_delay_ns`, `delay_ns`, `cs_change_delay_ns`, and
`tx`. A successful SPI reply supplies `completed`, concatenated RX bytes in
`data`, and one `effective_speeds` entry per segment.

## Current interface scope

I2C, SPI, CDC ACM UART, and GPIO are implemented. `/dev/gpiomem` is not
applicable because it exposes local memory-mapped GPIO registers, not GPIO on a
USB-attached controller.
