# rp-as-extio

`rp-as-extio` exposes I2C, SPI, UART, GPIO, ADC, PWM, and clock outputs from a
USB-connected Raspberry Pi Pico 2. I2C, SPI, GPIO, ADC, and PWM use standard
Linux interfaces; the daemon CLI adds scripting conveniences and dynamic pin
routing. A simulator provides deterministic peripheral state for tests that
should not require hardware.

This repository contains four cooperating pieces:

- RP2350 firmware with a vendor-specific USB bulk interface
- a small out-of-tree Linux module that registers I2C, SPI, GPIO, IIO ADC, and
  PWM controllers
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
| ADC0–2 | disabled | GP26, GP27, GP28 |
| PWM0A–7B | disabled | one selected RP2350A mux route per channel |
| CLOCK0 / CLOCK1 | disabled | GP13 or GP21 / GP15 |
| GPIO | 17 lines | GP2, GP3, GP6–GP15, GP20, GP22, GP26–GP28 |

Routes and enabled states can be changed at runtime. A disabled peripheral
keeps its remembered routes but reserves no pins. Every unassigned Pico 2
header GPIO becomes a Linux GPIO line in ascending pin order. Enabled ADC,
PWM, and clock signals reserve their selected pins just like serial buses.
The firmware reports the generated mapping to the daemon and kernel after
every change.

All signals are 3.3 V only. Do not connect 5 V logic directly. I2C SDA and SCL
need external pull-ups to 3.3 V; 2.2–4.7 kΩ is a common starting range, but the
correct value depends on bus capacitance and attached devices. Connect a ground
between the Pico 2 and every peripheral.

GPIO inputs support disabled, pull-up, and pull-down bias. The three ADC inputs
are 12-bit conversions referenced to 3.3 V. PWM routes follow the RP2350A mux:
slice 0 through 2 A/B use GP0/1 through GP5 (with GP16–GP21 aliases), slice 3
uses GP6/GP7 (A also GP22), slice 4 uses GP8/GP9, slice 5 uses GP10/GP11 (with
GP26/GP27 aliases), slice 6 uses GP12/GP13 (A also GP28), and slice 7 uses
GP14/GP15. Channels A and B on one slice share a period. CLOCK0 routes to GP13
or GP21 and CLOCK1 routes to GP15.

See [the hardware verification guide](docs/hardware-verification.md) for
breadboard circuits, recommended I2C and SPI peripherals, GPIO LED/button and
loopback tests, Raspberry Pi UART-console wiring, lifecycle testing, ADC,
PWM, servo, and clock-output fixtures.

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
open it. Closing it unregisters every I2C, SPI, GPIO, IIO, and PWM instance and
wakes outstanding operations with `ENODEV`.

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
rp-extioctl set 125DBCCF0BCA4BEF \
  --route adc0=GP26 --enable adc0 \
  --route pwm1a=GP18 --enable pwm1a
rp-extioctl set 125DBCCF0BCA4BEF \
  --route clock0=GP13 --enable clock0 \
  --clock-source 0=system --clock-hz 0=1000000
```

Every route option is instance-qualified. The old unqualified options such as
`--i2c-sda` and `--spi-miso` remain aliases for instance 0. Each of `uart0`,
`uart1`, `i2c0`, `i2c1`, `spi0`, and `spi1` has matching `--enable-*` and
`--disable-*` options. Routes can be edited while an instance is disabled;
enabling it later atomically rejects any conflict with the active instances.
Each SPI instance also has `--enable-spiN-cs1` and `--disable-spiN-cs1`.
Disabling CS1 leaves CS0 and the controller active, removes the second spidev
child, and returns the remembered CS1 pin to GPIO.
The repeatable `--route SIGNAL=PIN`, `--enable SIGNAL`, and `--disable SIGNAL`
forms cover every serial, ADC, PWM, and clock signal. Clock source names are
`system`, `reference`, `usb`, `adc`, `crystal`, `ring-oscillator`, and
`low-power-oscillator`.

Every accepted change removes and recreates that board's Linux devices, so
rediscover their numbers and GPIO offsets afterward. Disabled I2C and SPI
instances have no Linux bus; unrouted ADC and PWM channels return unsupported.
A stale
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
the 39 peripheral signals, and Enter or Space assigns that signal to the
highlighted GPIO in the draft. Assignments move the selected hardware
instance's UART TX/RX, I2C SDA/SCL, or SPI MISO/SCK/MOSI mux group together by
default. A grouped move that would conflict with another enabled signal or
cannot fit on the exposed header falls back to moving only the selected signal
when that still produces a valid map; otherwise it is rejected without changing
the draft. SPI chip selects, ADC inputs, PWM channels, and clock outputs remain
individually assignable.

Press `g` to switch between grouped and individual assignment. Individual mode
allows unusual valid layouts without a complete exposed group, and can leave a
temporarily invalid draft for manual correction. Valid routes are highlighted;
conflicts, mismatched I2C pairs, changed pins, exported GPIO lines, power pins,
grounds, and current Linux bus assignments remain visible on the board.

Assigning a disabled peripheral with Enter or Space also enables it when the
resulting map is valid. If enabling it would create a conflict, the TUI leaves
the draft unchanged and reports the conflicting route.

`e` toggles the selected serial peripheral instance; SPI CS1 and every ADC,
PWM, or clock signal toggle individually. Disabling a function immediately
previews its pins as GPIO; enabling validates all remembered routes before
changing the draft. `f` mirrors the diagram
left-to-right for a board mounted belly up, keeping USB at the top while showing
the pin headers as viewed from the back. Arrow movement remains aligned with the
screen.
Orientation is not persisted.
With CLOCK0 or CLOCK1 selected, `s` cycles its source and `+`/`-` doubles or
halves its target frequency. Firmware reports the effective frequency after
the RP2350 16.16 divider is quantized.

`a` applies the complete draft atomically, `d` loads the firmware defaults into
the draft, and `u` discards edits and restores the active map. `[` and `]`
select a board, `r` refreshes, `l` reloads the persistent file, and `q` exits.
Short terminals scroll the physical pin rows around the cursor, while wide
terminals also show a complete signal legend with enabled-state markers.

Press `v` to switch from route mode to operate mode. The pinout then refreshes
live state while idle and annotates every exported GPIO as input or output,
high or low, and pull-up or pull-down. The status panel expands the highlighted
pin's state and interprets it as a button; `b` switches between active-low and
active-high button wiring. `0` and `1` drive the highlighted GPIO low or high,
`t`, Enter, or Space toggles it as a persistent LED/output, and `i` returns it
to input while cycling through pull-up, pull-down, and no internal bias. These
actions use the daemon's normal Polkit authorization.

Selecting an enabled ADC signal in operate mode continuously shows its raw and
millivolt reading. Selecting an enabled PWM signal shows its runtime enable,
effective frequency, duty cycle, and polarity. Press `p`, Enter, or Space to
open the PWM editor: type the frequency and duty percentage, use Tab to switch
fields, `e` to toggle output, `i` to invert polarity, Enter to apply, and Escape
to cancel. Only the selected ADC or PWM is polled; GPIO state still arrives in
one bulk snapshot. Clock selections show their configured source, target
frequency, and enabled state. Press `v` again to return to route editing.

The persistent file is JSON and always contains complete maps:

```json
{
  "version": 3,
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
      "spi1_cs0": 13, "spi1_cs1_enabled": true, "spi1_cs1": 20,
      "adc_enabled": [false, false, false],
      "adc_pins": [26, 27, 28],
      "pwm_enabled": [false, false, false, false, false, false, false, false,
                      false, false, false, false, false, false, false, false],
      "pwm_pins": [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
      "clock_enabled": [false, false],
      "clock_pins": [13, 15],
      "clock_sources": ["system", "system"],
      "clock_hz": [1000000, 1000000]
    }
  }
}
```

Version-1 files are loaded by placing their routes on instance 0 and retaining
the disabled instance-1 defaults. Version-2 maps gain disabled ADC, PWM, and
clock functions. The next successful save writes version 3. Version-2 maps
without the CS1 enable fields load with both second chip selects enabled.

This release uses USB protocol major 3 and kernel ABI major 3. Rebuild and
install the module and daemon, and flash the matching firmware as one upgrade;
the daemon rejects older incompatible modules and firmware.

The socket is mode `0666`, but access is enforced per request using the Unix
peer credentials supplied by the kernel. Any local process can use `list`,
`show`, ADC reads, PWM queries, GPIO snapshots, and the read-only parts of the
TUI. Routing changes, GPIO direction/bias/output changes, PWM application, and
TUI actions that apply a change ask Polkit for `org.rp-extio.configure`; an
active desktop session therefore receives its normal authentication dialog.

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
ls -d /sys/bus/iio/devices/iio:device* /sys/class/pwm/pwmchip*
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

Internal pulls, persistent outputs, LED polarity, bulk snapshots, and button
waits are also available without finding dynamic GPIO line offsets:

```sh
rp-extioctl gpio snapshot 125DBCCF0BCA4BEF
rp-extioctl gpio input 125DBCCF0BCA4BEF GP22 --pull pull-up
rp-extioctl gpio set 125DBCCF0BCA4BEF GP22 high
rp-extioctl led 125DBCCF0BCA4BEF GP22 on --active-low
rp-extioctl button 125DBCCF0BCA4BEF GP22 --active-low --wait pressed --timeout-ms 10000
```

Daemon-owned output state persists until another command changes it, the pin
map is reconfigured, or the device disconnects.

Enable an ADC route before reading it. The direct CLI reports raw counts and
millivolts; standard IIO exposes the same 12-bit conversion and 3.3 V scale:

```sh
rp-extioctl adc 125DBCCF0BCA4BEF 0
cat /sys/bus/iio/devices/iio:deviceN/in_voltage0_raw
cat /sys/bus/iio/devices/iio:deviceN/in_voltage_scale
```

Each PWM channel is a hardware slice/channel pair from PWM0A through PWM7B.
The A and B channels on a slice share one period; applying a different period
while the sibling is active fails with `busy`. Query and configure channels
through the CLI, or use the standard `/sys/class/pwm/pwmchipN` interface:

```sh
rp-extioctl pwm get 125DBCCF0BCA4BEF 2
rp-extioctl pwm set 125DBCCF0BCA4BEF 2 --frequency-hz 1000 --duty-percent 25
rp-extioctl pwm disable 125DBCCF0BCA4BEF 2

echo 2 > /sys/class/pwm/pwmchipN/export
echo 1000000 > /sys/class/pwm/pwmchipN/pwm2/period
echo 250000 > /sys/class/pwm/pwmchipN/pwm2/duty_cycle
echo 1 > /sys/class/pwm/pwmchipN/pwm2/enable
```

Writes to the PWM sysfs interface normally require the host's standard device
permissions. The `rp-extioctl` mutation path instead requests Polkit
authorization from the active session.

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

`set_input` rejects output lines. ADC channels return deterministic values of
512, 2048, and 3584 counts. PWM state is retained, and different active sibling
periods reproduce the hardware `busy` result. GPIO snapshots include levels,
directions, pull-ups, and pull-downs. `reset` restores GPIO and PWM state in
addition to clearing expected and observed bus transactions.

Reply status values are `0` OK, `1` NACK, `2` arbitration lost, `3` timeout,
`4` disconnected, `5` unsupported, `6` bad frame, `7` I/O error, `8` invalid
route, `9` pin conflict, and `10` busy. SPI
expectations use `chip_select`, `mode`, and a `segments` array; each segment has
`speed_hz`, `flags`, `word_delay_ns`, `delay_ns`, `cs_change_delay_ns`, and
`tx`. A successful SPI reply supplies `completed`, concatenated RX bytes in
`data`, and one `effective_speeds` entry per segment.

## Current interface scope

I2C, SPI, CDC ACM UART, GPIO with internal bias, three ADC inputs, sixteen PWM
channels, and two clock outputs are implemented. `/dev/gpiomem` is not
applicable because it exposes local memory-mapped GPIO registers, not GPIO on a
USB-attached controller. ADC capture buffers, PWM capture, and precise GPIO
edge timing are outside the current interface.
