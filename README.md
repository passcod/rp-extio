# rp-as-extio

`rp-as-extio` exposes I2C and SPI buses on a USB-connected Raspberry Pi Pico 2
as ordinary Linux I2C adapters and SPI controllers. A simulator registers the
same Linux bus devices for tests that should not require hardware.

This repository contains four cooperating pieces:

- RP2350 firmware with a vendor-specific USB bulk interface
- a small out-of-tree Linux module that registers I2C and SPI controllers
- `rp-extiod`, which discovers boards and dispatches kernel requests over USB
- a newline-delimited JSON simulator control socket in `rp-extiod`

The development USB identity is `cafe:4010`. It must be replaced before a
distributed product uses an allocated VID/PID.

## Hardware and electrical limits

The Pico 2 pin map is:

| Function | Pico 2 pin |
| --- | --- |
| I2C0 SDA | GP4 |
| I2C0 SCL | GP5 |
| SPI0 SCK | GP18 |
| SPI0 MOSI | GP19 |
| SPI0 MISO | GP16 |
| SPI0 CS0 | GP17 |
| SPI0 CS1 | GP21 |
| UART0 TX | GP0 |
| UART0 RX | GP1 |

All signals are 3.3 V only. Do not connect 5 V logic directly. I2C SDA and SCL
need external pull-ups to 3.3 V; 2.2–4.7 kΩ is a common starting range, but the
correct value depends on bus capacitance and attached devices. Connect a ground
between the Pico 2 and every peripheral.

I2C initially runs at 100 kHz. SPI accepts 50 kHz through 20 MHz, modes 0–3,
8-bit words, MSB- or LSB-first transfers, active-high or active-low chip select,
and no-CS transfers. USB full-speed latency means this is a functional remote
bus for development and integration tests, not a timing-equivalent substitute
for a local controller.

UART0 appears through Linux's `cdc_acm` driver as `/dev/ttyACM*`. It uses
115200 baud, eight data bits, no parity, and one stop bit. Set the host terminal
to the same format; CDC line-coding requests do not reconfigure the RP2350 UART.

## Build and flash the firmware

Install the Rust target and `picotool`, then build the UF2:

```sh
rustup target add thumbv8m.main-none-eabihf
./scripts/build-uf2.sh
```

Hold BOOTSEL while connecting the Pico 2, then copy
`build/rp-extio-firmware.uf2` to the `RP2350` mass-storage volume. The firmware
uses the RP2350 OTP chip ID as its USB serial number. That serial is the stable
board identity; `/dev/i2c-N` and `/dev/spidevN.*` numbers can change after a
daemon or board reconnect.

## Install the Linux module

Install the compiler, make, DKMS, and headers for the running kernel. To stage
version 0.1.0 for DKMS:

```sh
sudo install -d /usr/src/rp-extio-0.1.0
sudo install -m 0644 linux/Makefile linux/dkms.conf \
  linux/rp_extio.c linux/rp_extio_uapi.h /usr/src/rp-extio-0.1.0/
sudo dkms add -m rp-extio -v 0.1.0
sudo dkms build -m rp-extio -v 0.1.0
sudo dkms install -m rp-extio -v 0.1.0
sudo modprobe rp_extio
```

This creates `/dev/rp-extio-control`, owned by root with mode `0600`. One daemon
may open it. Closing the control device unregisters every I2C/SPI instance and
wakes outstanding bus operations with `ENODEV`.

To remove this version:

```sh
sudo systemctl disable --now rp-extiod.service
sudo modprobe -r rp_extio
sudo dkms remove -m rp-extio -v 0.1.0 --all
sudo rm -r /usr/src/rp-extio-0.1.0
```

## Install and run the daemon

Build and install the daemon and service:

```sh
cargo build --release -p rp-extiod
sudo install -m 0755 target/release/rp-extiod /usr/local/bin/rp-extiod
sudo install -m 0644 dist/rp-extiod.service \
  /etc/systemd/system/rp-extiod.service
sudo systemctl daemon-reload
sudo systemctl enable --now rp-extiod.service
```

The daemon watches for matching USB devices, negotiates protocol version and
capabilities before registering buses, and supports multiple boards. It permits
one physical transaction at a time per board. Independent boards and simulated
devices can operate concurrently.

A disconnect, USB timeout, mismatched response, or uncertain frame tears down
that board's Linux buses. Non-idempotent transactions are never retried. A
reconnected board is negotiated and registered again, potentially under new
Linux bus numbers. The firmware bus deadline is five seconds, the daemon USB
deadline is six seconds, and the kernel callback deadline is eight seconds.

Inspect assigned bus numbers with:

```sh
i2cdetect -l
ls -l /dev/spidev*
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

## Simulator

Start the daemon with a Unix control socket in addition to USB discovery:

```sh
sudo /usr/local/bin/rp-extiod --simulate /run/rp-extiod-sim.sock
```

Commands and responses are one JSON object per line. Device state is isolated
by the `device` name. Creating a device returns the allocated Linux bus numbers:

```json
{"command":"create","device":"test-1"}
{"result":"created","i2c_bus":12,"spi_bus":7}
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

Reply status values are `0` OK, `1` NACK, `2` arbitration lost, `3` timeout,
`4` disconnected, `5` unsupported, `6` bad frame, and `7` I/O error. SPI
expectations use `chip_select`, `mode`, and a `segments` array; each segment has
`speed_hz`, `flags`, `word_delay_ns`, `delay_ns`, `cs_change_delay_ns`, and
`tx`. A successful SPI reply supplies `completed`, concatenated RX bytes in
`data`, and one `effective_speeds` entry per segment.

## Current interface scope

I2C, SPI, and CDC ACM UART are implemented. The remote GPIO controller is a
specified follow-on interface and is not exposed by the current firmware.
