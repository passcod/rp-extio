# Use your devkit (RP Pico, ESP32) as a Linux serial peripheral

Let's say you're working on Linux software that controls an I2C or SPI device
for an embedded system. You can connect to the system in question over SSH and
compile there, or you can cross-compile, ship the binary, and test over there,
or other such arrangements. But that gets tedious. Or maybe you have a fun
sensor you just want to connect to your computer permanently, without writing
a whole firmware for it.

You already have a panoply of various TTL to USB adapters that more or less work.
You consider getting something like a [DLN-2](https://diolan.com/dln2). It's only
100 united states dollars, plus 60 for shipping to your location.

But you do have a whole drawer full of various devkits, devboards, etc. These are
here, now, and shipping only requires walking to your workshop, and perhaps
soldering on some headers. What if they could be used as a USB to Whatever adapter?

Surely someone has even made the firmware already.

Yes. Yes I have.

## Overview

`rp-extio` is so named because the first target was a Raspberry Pico 2. However,
we've since grown support for more boards. There are several components:

- A small kernel module, `rp-extio.ko`, creates the /dev/ interfaces, and exposes an ABI to...
- the `rp-extiod` userspace daemon, which connects over USB to...
- a devboard flashed with the `rp-extio-firmware`.
- The `rp-extioctl` CLI and TUI provides the management surface.

Your software can then use the Linux I²C, SPI, GPIO, IIO, PWM, and tty
interfaces as normal.

Using the `rp-extioctl tui`, you can enable, disable, map, and configure peripherals
to whatever pin sets they support on your devboard, at runtime (configuration does not
require recompiling/reflashing the firmware). There's also some convenience features,
such as being able to set GPIO outputs high/low, see the current level of GPIO inputs,
and set the duty cycle of PWMs and frequency of clock outputs, if available.

Peripheral configurations are remembered on the host device (your computer) per device
serial, and are not saved to the devboard itself.

## Setup

Right now only building and setting up from source is supported. Pre-builds may come later.

Requirements:

- A Rust toolchain.
- The Rust target for your hardware, and its flashing software (this will be specified for each board).
- The [`just` command runner](https://just.systems).

Build and install:

```sh
just install
just enable
```

The installed service runs as root so it can open supported USB devices and
serial bridges without a separate dialout-group setup. A custom unprivileged
service needs read/write access to those device nodes as well as the kernel
control device.

## Supported boards

### Raspberry Pico 2

- Peripherals: I²C, SPI, UART, GPIO, ADC inputs, PWM outputs, clock outputs.
- Core: RP2350.
- Signal voltage: 3.3V only.

Serial buses may need pull-ups.

| Peripheral | Startup state | Default GPIOs / notes |
| --- | --- | --- |
| UART0 | enabled | TX GP0, RX GP1 |
| UART1 | disabled | TX GP8, RX GP9 |
| I2C0 | enabled | SDA GP4, SCL GP5 |
| I2C1 | disabled | SDA GP10, SCL GP11 |
| SPI0 | enabled | MISO GP16, SCK GP18, MOSI GP19, CS0 GP17, CS1 GP21 |
| SPI1 | disabled | MISO GP12, SCK GP14, MOSI GP15, CS0 GP13, CS1 GP20 |
| ADC0–2 | disabled | GP26, GP27, GP28 |
| PWM0A–7B | disabled | one selected RP2350A mux route per channel |
| CLOCK0 | disabled | GP13 |
| CLOCK1 | disabled | GP15 |
| GPIO | 17 lines | GP2, GP3, GP6–GP15, GP20, GP22, GP26–GP28 |

Notes:

- GPIO inputs support none, pull-up, and pull-down bias using internal resistors.
- The onboard GP25 LED can be controlled from the CLI and TUI.
- The three ADC inputs are 12-bit conversions referenced to 3.3 V.
- I2C initially runs at 100 kHz.
- SPI accepts 50 kHz through 20 MHz, modes 0–3, 8-bit words, MSB- or LSB-first
  transfers, active-high or active-low chip select, and no-CS transfers.

#### Firmware

- Rust target: `thumbv8m.main-none-eabihf`
- Flasher: `picotool`

Put your device in BOOTSEL mode. Either:
- remove power, hold BOOTSEL, apply power, release BOOTSEL
- hold BOOTSEL, short RUN pin to ground, release BOOTSEL
- hold TP6 to ground, short RUN pin to ground, release TP6

Then flash:

```sh
just flash pico2
```

Subsequent flashes don't need BOOTSEL mode.

### ESP32-C3-DevKitM-1 v1.6

- Peripherals: I²C, SPI, UART, GPIO, ADC inputs, PWM outputs.
- Core: ESP32-C3.
- Signal voltage: 3.3V only.

GPIO20 and GPIO21 are reserved for rp-extio. GPIO2 and GPIO9 are strapping pins;
GPIO8 is a strapping pin connected to the onboard RGB LED.
The onboard bridge carries the rp-extio control protocol at 115200 baud.

| Peripheral | Startup state | Default GPIOs / notes |
| --- | --- | --- |
| UART1 | disabled | TX GPIO6, RX GPIO7 |
| I2C0 | enabled | SDA GPIO4, SCL GPIO5 |
| SPI0 | enabled | MISO GPIO3, SCK GPIO6, MOSI GPIO7, CS0 GPIO10, CS1 GPIO1 |
| ADC0–4 | disabled | GPIO0–GPIO4 |
| PWM0–5 | disabled | any available GPIO |

Notes:

- GPIO inputs support none, pull-up, and pull-down bias using internal resistors.
- ADC0–4 are 12-bit conversions over the ESP32-C3 ADC's 0–2.5 V measurement range.
- PWM channels can route independently. Channels 0/4 and 1/5 share LEDC timers,
  so each pair must use the same period while both outputs are active.
- I2C initially runs at 100 kHz.
- SPI accepts 50 kHz through 20 MHz, modes 0–3, 8-bit words, MSB- or LSB-first
  transfers, active-high or active-low chip select, and no-CS transfers.
- UART1 appears as `/dev/ttyRPE*` when enabled and follows standard termios
  settings up to 5 Mbaud.
- The onboard GPIO8 RGB LED can be controlled from the CLI and TUI while GPIO8
  is not assigned to another signal.

#### Firmware

- Rust target: `riscv32imc-unknown-none-elf`
- Flasher: [`espflash`](https://github.com/esp-rs/espflash)

With one supported board attached:

```sh
just flash esp32c3
```

If several supported bridges are attached, pass the selected stable path as a
second argument.

The daemon automatically discovers every attached board using a supported
USB-to-UART bridge and confirms its firmware before registering it. Repeatable
`rp-extiod --serial PATH` options add compatible bridge models that are not in
the discovery table.

## Simulator

In addition to running against real devices, the `rp-extiod` supports simulation mode.
Using it, you can emulate I2C and SPI devices for testing. Create devices, queue expected
transactions to the daemon's unix socket, and devices will respond to software accesses
via the usual /dev/ interfaces accordingly.

To enable, add the `--simulate` option to the daemon.

Commands and responses are one JSON object per line. Device state is isolated
by the `device` name. Creating a device returns the allocated Linux bus numbers:

```json
{"command":"create","device":"test-1"}
{"result":"created","i2c_buses":[12,null],"spi_buses":[7,null]}
```

Queue exact expected transactions and their replies before software accesses the bus:

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
route, `9` pin conflict, and `10` busy.

SPI expectations use `chip_select`, `mode`, and a `segments` array; each segment
has `speed_hz`, `flags`, `word_delay_ns`, `delay_ns`, `cs_change_delay_ns`, and
`tx`. A successful SPI reply supplies `completed`, concatenated RX bytes in
`data`, and one `effective_speeds` entry per segment.

