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
| GPIO lines 0–16 | GP2, GP3, GP6–GP15, GP20, GP22, GP26–GP28 |

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

The GPIO controller exposes 17 lines. GPIO line offsets map to physical pins as
follows; pins assigned to UART, I2C, and SPI are deliberately absent:

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

Hold BOOTSEL while connecting the Pico 2, then copy
`build/rp-extio-firmware.uf2` to the `RP2350` mass-storage volume. The firmware
uses the RP2350 OTP chip ID as its USB serial number. That serial is the stable
board identity; `/dev/i2c-N`, `/dev/spidevN.*`, and `/dev/gpiochipN` numbers can
change after a daemon or board reconnect.

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
may open it. Closing the control device unregisters every I2C, SPI, and GPIO
instance and wakes outstanding bus operations with `ENODEV`.

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

The simulated GPIO starts with all lines low and configured as inputs. Inject
an input transition by line offset; this updates reads and emits a Linux edge
event when a consumer has requested the matching edge:

```json
{"command":"set_input","device":"test-1","line":0,"value":1}
```

`set_input` rejects output lines. `reset` also restores every line to a low
input in addition to clearing expected and observed bus transactions.

Reply status values are `0` OK, `1` NACK, `2` arbitration lost, `3` timeout,
`4` disconnected, `5` unsupported, `6` bad frame, and `7` I/O error. SPI
expectations use `chip_select`, `mode`, and a `segments` array; each segment has
`speed_hz`, `flags`, `word_delay_ns`, `delay_ns`, `cs_change_delay_ns`, and
`tx`. A successful SPI reply supplies `completed`, concatenated RX bytes in
`data`, and one `effective_speeds` entry per segment.

## Current interface scope

I2C, SPI, CDC ACM UART, and GPIO are implemented. `/dev/gpiomem` is not
applicable because it exposes local memory-mapped GPIO registers, not GPIO on a
USB-attached controller.
