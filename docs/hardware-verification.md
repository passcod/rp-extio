# Hardware verification

This guide verifies `rp-extio` against physical circuits and peripherals.
It complements the automated simulator and protocol tests by exercising the
complete Linux kernel, daemon, USB, firmware, and electrical path.

Record the commit, firmware build, host kernel, Pico serial number, wiring, and
peripheral part numbers for each run. Linux bus numbers can change after any
reconnect, so discover them again rather than carrying numbers between steps.

## Safety

- Use 3.3 V logic only. Never drive either board's GPIO above 3.3 V.
- Connect ground between the Pico and every attached board.
- Do not connect the 3.3 V or 5 V supply rails of two independently powered
  computers. The Raspberry Pi UART test needs signal and ground wires only.
- Power down before changing breadboard wiring.
- Put a 1 kΩ resistor in every GPIO loopback. This limits current if both pins
  are accidentally configured as outputs with opposite levels.
- Put a 330 Ω to 1 kΩ resistor in series with every LED.
- Use dedicated test memories for destructive EEPROM and flash tests.
- Check each breakout's data sheet and schematic. A board advertised for 5 V
  systems may contain pull-ups or level shifting unsuitable for this fixture.

## Fixture and equipment

### Minimum fixture

- Raspberry Pi Pico 2 with soldered headers and current `rp-as-extio` firmware
- solderless breadboard and jumper wires
- 1 kΩ resistors for GPIO loopbacks
- one LED and a 330 Ω to 1 kΩ series resistor
- one momentary button and a 10 kΩ pull-up resistor
- two 2.2 kΩ to 4.7 kΩ I2C pull-up resistors
- multimeter
- Linux host running the module and daemon

A 3.3 V-capable logic analyser is strongly recommended. It makes chip-select,
clock mode, I2C repeated starts, UART framing, and hot-unplug failures much
easier to distinguish from software failures.

### Peripheral set

The following categories provide broader coverage than several devices with
the same transaction pattern:

- An I2C register device that uses a combined write and repeated-start read,
  such as a BME280-compatible sensor.
- A 3.3 V I2C EEPROM, such as a 24xx-family part, dedicated to destructive
  write, page-boundary, persistence, and temporary-NACK tests.
- An I2C device with a documented clock-stretching mode, or a programmable
  microcontroller acting as a slave.
- A 3.3 V SPI NOR flash, such as a W25Q-family part, dedicated to destructive
  erase, program, and readback tests.
- A second SPI device with an independent chip select, such as an ADC, sensor,
  or second flash.
- A second programmable microcontroller when full I2C and SPI edge-case
  coverage is required. Ordinary peripherals rarely expose 10-bit I2C,
  arbitration loss, every SPI mode, LSB-first operation, and deliberately
  malformed timing in one fixture.
- A Raspberry Pi or other 3.3 V UART system for the serial-console test.

The programmable-slave fixture is optional for initial verification, but it is
the practical route to exhaustive protocol coverage.

## Default Pico wiring reference

These are firmware startup defaults, not fixed pins. Runtime configuration can
replace them without rebuilding. Always wire the fixture to the active map from
`rp-extioctl show SERIAL`.

| Interface | State | GPIO and Pico header pin |
| --- | --- | --- |
| UART0 TX / RX | enabled | GP0 pin 1 / GP1 pin 2 |
| UART1 TX / RX | disabled | GP8 pin 11 / GP9 pin 12 |
| I2C0 SDA / SCL | enabled | GP4 pin 6 / GP5 pin 7 |
| I2C1 SDA / SCL | disabled | GP10 pin 14 / GP11 pin 15 |
| SPI0 MISO / SCK / MOSI | enabled | GP16 pin 21 / GP18 pin 24 / GP19 pin 25 |
| SPI0 CS0 / CS1 | enabled | GP17 pin 22 / GP21 pin 27 |
| SPI1 MISO / SCK / MOSI | disabled | GP12 pin 16 / GP14 pin 19 / GP15 pin 20 |
| SPI1 CS0 / CS1 | disabled | GP13 pin 17 / GP20 pin 26 |
| 3V3 OUT | — | 36 |

Convenient ground pins include header pins 3, 8, 13, 18, 23, 28, and 33.

## Default ESP32-C3-DevKitM-1 v1.6 wiring reference

These routes are runtime configurable. GPIO20 and GPIO21 belong to the control
transport and cannot be reassigned. GPIO2 and GPIO9 are boot-strapping pins;
GPIO8 is also a strapping pin and drives the onboard RGB LED. Avoid external
pulls on those pins until ordinary routes have been verified.
The onboard CP2102N bridge carries the control protocol at 115200 baud.

| Interface | State | GPIO and header |
| --- | --- | --- |
| UART1 TX / RX | disabled | GPIO6 J3-9 / GPIO7 J3-8 |
| I2C0 SDA / SCL | enabled | GPIO4 J3-11 / GPIO5 J3-10 |
| SPI0 MISO / SCK / MOSI | enabled | GPIO3 J1-5 / GPIO6 J3-9 / GPIO7 J3-8 |
| SPI0 CS0 / CS1 | enabled | GPIO10 J1-11 / GPIO1 J1-10 |
| ADC0–4 | disabled | GPIO0 J1-9, GPIO1 J1-10, GPIO2 J1-4, GPIO3 J1-5, GPIO4 J3-11 |
| PWM0–5 | disabled | any available exposed GPIO |
| RGB LED | available | onboard GPIO8, J3-6 |
| 3V3 / GND | — | J1-2 or J1-3 / J1-1 |

GPIO0–GPIO10, GPIO18, and GPIO19 may be used as GPIO when not assigned to a
peripheral. The ADC input measurement range is 0–2.5 V even though digital I/O
uses 3.3 V logic. Never apply 3.3 V directly to an ADC test input.

## Record the test environment

Before wiring peripherals, record:

```text
Date:
Repository commit:
Firmware UF2 checksum:
Host distribution:
Host kernel:
Kernel module build:
Daemon build:
Board kind and revision:
Firmware identity:
USB bridge path:
I2C0 / I2C1 adapters:
SPI0 / SPI1 controllers and children:
GPIO chip:
IIO device:
PWM chip:
UART0 / UART1 CDC ACM devices:
Daemon-backed tty devices:
Peripheral parts and board revisions:
Logic-analyser capture names:
```

Confirm the Pico identity and rediscover every interface:

```sh
lsusb -d cafe:4010
ls -l /dev/serial/by-id/
i2cdetect -l
ls -l /dev/spidev*
gpioinfo
ls -d /sys/bus/iio/devices/iio:device*
ls -d /sys/class/pwm/pwmchip*
systemctl status rp-extiod.service
rp-extioctl list
```

With the startup map, the board must have a stable USB serial, two CDC ACM
devices, one I2C0 adapter, two SPI0 `spidev` children, one 17-line GPIO chip,
one three-channel IIO device, and one sixteen-channel PWM chip. UART1 remains
enumerated over USB while disabled; disabled I2C/SPI instances have no Linux
buses, while disabled ADC/PWM channels reject operations. Run the ignored USB
hardware test before attaching a peripheral:

```sh
cargo test -p rp-extiod negotiates_connected_bridge_and_reads_gpio_snapshot \
  -- --ignored --nocapture
```

## Runtime routing

Perform this section with one stock UF2; do not rebuild between routes.

1. Record the startup map with `rp-extioctl show SERIAL`.
2. Move I2C0 to GP12/GP13 with `rp-extioctl set SERIAL --i2c0-sda GP12
   --i2c0-scl GP13`, rediscover the adapter, and read the attached I2C device.
3. Move I2C0 to a second valid pair, move the wires, and repeat the read.
4. Move SPI0 MISO/SCK/MOSI and both chip selects to another conflict-free map,
   rediscover both spidev children, and repeat loopback and device-ID reads.
5. Disable SPI0 CS1 while leaving SPI0 enabled. Confirm the second spidev child
   disappears, CS0 traffic continues, GP21 returns to the GPIO controller, and
   requests for chip select 1 are rejected. Re-enable CS1 and confirm the child
   and route return.
6. Move UART0 TX/RX, reconnect the signal wires without reconnecting USB, and
   verify both CDC-to-UART and UART-to-CDC traffic. Restore the startup map
   before enabling all remembered instance-1 routes below.
7. Enable I2C1 on its remembered GP10/GP11 route. Confirm a second I2C adapter
   appears, each adapter reaches only its wired bus, and the GPIO count falls
   from 17 to 15.
8. Enable SPI1 on its remembered GP12/GP14/GP15, CS GP13/GP20 route. Confirm a
   second controller with two children appears, exercise both SPI instances,
   and confirm the GPIO count falls to 10.
9. Enable UART1 on GP8/GP9, exercise the second ACM path in both directions,
   and confirm the GPIO count falls to 8. The two ACM devices must have remained
   enumerated throughout.
10. Disable each second instance again. Its remembered routes must stay visible,
   its I2C or SPI Linux bus must disappear, and its pins must return to the GPIO
   list in ascending GPIO order.
11. Disable all six peripherals. Confirm there are no I2C or SPI buses for the
    board, both ACM devices remain, and the GPIO controller exposes all 26
    header GPIOs. Restore the startup state with `rp-extioctl default SERIAL`.
12. Attempt an invalid mux route and a duplicate enabled assignment. The
    client or firmware must reject each one while the previous buses and active
    map remain usable.
13. Change remembered routes while an instance is disabled, including a route
    overlapping an enabled peripheral. The edit may be saved, but enabling the
    conflicting instance must fail atomically without changing the active map.
14. Start two `rp-extioctl set` operations from the same observed generation.
   One may apply; the other must report a generation conflict.
15. Restart the daemon and then reset the Pico. The persistent map must be
   reapplied in both cases before Linux buses appear.
16. Run `just flash` while the current firmware is running. Confirm `picotool`
    resets the board into BOOTSEL, updates it, and executes the new image without
    touching BOOTSEL or disconnecting the cable.
17. Run `rp-extioctl default SERIAL`; the startup map must return, its
    persistent entry must disappear, and Linux buses must be rediscovered.
18. Run `rp-extioctl tui SERIAL`, exercise every signal selector, grouped moves
    within both hardware instances, the `e` enable toggle, conflict display,
    apply, refresh, reload, and default action. Press `f` and confirm the board
    mirrors left-to-right as viewed belly up, without exchanging the USB and
    SWD ends, and that arrow navigation remains screen-relative. Exit with both
    `q` and an interrupt and confirm the terminal is restored.
19. With UART0 on GP0/GP1, assign SPI0 SCK to GP2 in grouped mode. Confirm the
    TUI falls back to changing SCK alone, retains the existing MISO/MOSI routes,
    and leaves the draft valid.

## GPIO fixture

The line offsets below are for the startup map only. After any peripheral
enable, disable, or route change, derive the current offset-to-pin mapping from
`gpioinfo`; offsets are reassigned in ascending Pico GPIO order. Repeat the
input/output smoke test on every pin released by a disabled peripheral, and
repeat it with all peripherals disabled so all 26 header GPIOs are covered.

### Protected loopback pairs

Connect each pair through a 1 kΩ resistor:

| GPIO line | Pico pin | GPIO line | Pico pin |
| ---: | --- | ---: | --- |
| 0 | GP2, header 4 | 1 | GP3, header 5 |
| 2 | GP6, header 9 | 3 | GP7, header 10 |
| 4 | GP8, header 11 | 5 | GP9, header 12 |
| 6 | GP10, header 14 | 7 | GP11, header 15 |
| 8 | GP12, header 16 | 9 | GP13, header 17 |
| 10 | GP14, header 19 | 11 | GP15, header 20 |
| 12 | GP20, header 26 | 13 | GP22, header 29 |
| 14 | GP26, header 31 | 15 | GP27, header 32 |

Leave line 16, GP28 on header 34, available for the button circuit. Always
configure the receiving line as an input before driving its paired output.

For every pair:

1. Configure both lines as inputs and confirm neither operation blocks.
2. Drive the first line low and confirm the second reads low.
3. Drive the first line high and confirm the second reads high.
4. Reverse the direction and repeat both levels.
5. Repeat transitions while monitoring both edges on the input.
6. Hold each level and confirm there are no duplicate edge events.
7. Repeat with unrelated I2C and SPI traffic running concurrently.

The firmware samples GPIO every 10 ms. Hold test pulses for at least 100 ms;
short pulses are explicitly outside the interface guarantee.

### LED output circuit

Wire one exposed GPIO to a series resistor, then to the LED anode. Connect the
LED cathode to ground:

```text
GPIO ---- 330 Ω to 1 kΩ ----|>|---- GND
```

Verify that output-low turns the LED off, output-high turns it on, repeated
toggles remain responsive, then explicitly reconfigure the line as an input
and confirm the LED is no longer driven.

Exercise the daemon-owned persistent output and LED polarity paths as well as
libgpiod, replacing `SERIAL` and the free pin:

```sh
rp-extioctl gpio set SERIAL GP22 high
rp-extioctl gpio set SERIAL GP22 high --persist
rp-extioctl led SERIAL GP22 off
rp-extioctl led SERIAL GP22 on --active-low
```

### Button input circuit

Use an active-low button with an external pull-up:

```text
3V3 ---- 10 kΩ ----+---- GPIO
                   |
                 button
                   |
                  GND
```

Verify an unpressed high level, a pressed low level, and both edge directions.
A mechanical button can bounce, so it may produce several transitions. Use a
loopback output for deterministic edge counts and the button for a physical
input smoke test.

Remove the external 10 kΩ pull-up and repeat with the RP2350 internal pull:

```sh
rp-extioctl gpio input SERIAL GP28 --pull pull-up
rp-extioctl gpio input SERIAL GP28 --pull pull-up --persist
rp-extioctl gpio snapshot SERIAL
rp-extioctl button SERIAL GP28 --active-low --pull pull-up --wait pressed \
  --timeout-ms 10000
```

Repeat with a button to 3.3 V and `--pull pull-down`. Confirm `gpioinfo` reports
the requested bias and that a pin-map change clears daemon-owned direction and
bias state.

### Persisted GPIO startup state

Only persist output levels on the resistor-protected LED fixture or another
load known to tolerate them. Saved state is keyed by the Pico USB serial and is
applied by the host daemon, not by firmware. Expect a brief input/floating
interval after power-up, reset, or while the daemon is stopped; add an external
pull when the circuit needs a defined level throughout that interval.

1. Save the LED pin high with `rp-extioctl gpio set SERIAL GP22 high
   --persist`. Confirm `rp-extioctl show SERIAL` lists it as a saved output.
2. Restart `rp-extiod.service`, rediscover the GPIO device, and confirm the LED
   returns high without another GPIO command.
3. Reset or reconnect the Pico and confirm the same result after it enumerates.
4. Save the button pin using `rp-extioctl gpio input SERIAL GP28 --pull
   pull-up --persist`. Repeat the service restart and Pico reset, then confirm
   its direction and pull-up in `gpioinfo` and `rp-extioctl gpio snapshot`.
5. Change the live direction, bias, or level without `--persist`, run
   `rp-extioctl reload`, and confirm the saved state is restored.
6. Attempt to route an enabled peripheral onto GP22 or GP28. Confirm the map is
   rejected until `rp-extioctl gpio forget SERIAL PIN` removes that pin's
   startup entry.
7. In TUI operate mode, confirm saved pins have a `•` marker and a startup
   description. Use `w` to replace the highlighted pin's saved state with its
   live state and `x` to forget it without changing the live pin.
8. With the input highlighted, press `b` until it is an active-low button. Run
   `rp-extioctl button SERIAL GP28 --wait pressed`, press the button, and confirm
   the CLI reports `pressed (low)`. Cycle it to active-high and confirm
   `rp-extioctl show SERIAL` reports the changed role. Cycle once more to clear
   the role and confirm the button command asks for a saved role or an explicit
   `--active-low`/`--active-high` override.
9. Run `rp-extioctl default SERIAL` and confirm the custom map and all GPIO
   startup entries disappear. Recreate any map needed for later tests.

### GPIO acceptance criteria

- Every exposed line can become an input and an output.
- Every protected pair reads both driven levels in both directions.
- Deterministic 100 ms transitions produce the requested rising and falling
  events without missing or inventing stable transitions.
- The LED and button circuits behave according to their schematics.
- Invalid offsets and attempts to claim an already-owned line fail promptly.
- Duplicate saved states and maps that claim saved GPIO pins are rejected.
- Saved input and output states return after daemon restart, Pico reset, and
  explicit configuration reload.
- Stopping the daemon releases open GPIO operations with an error rather than
  leaving a new unkillable task.

## I2C fixture

Wire SDA and SCL to all I2C peripherals in parallel. Add one pull-up from SDA
to 3.3 V and one from SCL to 3.3 V; do not add a separate pair for every
breakout if the boards already contain suitable pull-ups.

```text
Pico GP4 SDA ----+---- peripheral SDA
                 +---- 2.2–4.7 kΩ ---- 3V3

Pico GP5 SCL ----+---- peripheral SCL
                 +---- 2.2–4.7 kΩ ---- 3V3

Pico GND  -------+---- peripheral GND
Pico 3V3 --------+---- 3.3 V peripheral VCC, when suitable
```

Measure the idle bus before starting: SDA and SCL should both be near 3.3 V.
Use `i2cdetect` only when the attached parts permit arbitrary probe commands;
some devices are disturbed by address scans.

### SCD41 demo

With an SCD41 at its fixed address `0x62`, run the demo script:

```sh
./scripts/scd41.sh
```

The script builds and starts `rp-extiod`, discovers the adapter created by that
daemon session, starts periodic measurement, waits for the first sample, and
prints CO2, temperature, and relative humidity. It then stops periodic
measurement and the daemon. The privileged phase uses `runzero` and prompts
through polkit.

### Register-device tests

1. Read the documented identity register.
2. Capture and confirm START, address-write, register byte, repeated START,
   address-read, data, NACK, and STOP.
3. Read a multi-byte value and compare it with the device's documented format.
4. Repeat the read enough times to expose intermittent framing failures.
5. Query an unused address and confirm Linux receives a NACK-class error.

### EEPROM tests

Use a reserved test region that contains no valuable data.

1. Save the original bytes from the region.
2. Write and read back walking-one, walking-zero, `0x00`, `0xff`, and seeded
   pseudo-random patterns.
3. Cross a page boundary using writes split according to the EEPROM data sheet
   rather than relying on undocumented wrap behaviour.
4. Exercise acknowledge polling while the EEPROM completes an internal write.
5. Power-cycle the peripheral and verify persistence.
6. Restore the original bytes.

### Extended I2C coverage

A programmable slave should test zero-length messages, 10-bit addressing,
clock stretching near the five-second firmware deadline, multi-message
transfers across different addresses, forced NACK at each byte position, and
arbitration loss. Confirm that one Linux transfer has repeated starts between
messages and exactly one final STOP.

### I2C acceptance criteria

- Known register values and EEPROM patterns round-trip correctly.
- The same fixture works through I2C0 and I2C1, including simultaneous adapters
  on independent pin pairs.
- Combined transactions retain repeated-start semantics.
- Unknown addresses and forced NACKs map to the documented Linux error.
- Clock stretching within the deadline succeeds; exceeding it times out and
  leaves the bus usable for the next request.
- A disconnected or held-low peripheral produces a bounded error and recovery,
  not a daemon or kernel hang.

## SPI fixture

Wire both SPI devices to the shared clock, MOSI, and MISO signals, with one
chip select per device:

```text
Pico GP18 SCK  ----+---- device 0 SCK
                   +---- device 1 SCK
Pico GP19 MOSI ----+---- device 0 MOSI
                   +---- device 1 MOSI
Pico GP16 MISO ----+---- device 0 MISO
                   +---- device 1 MISO
Pico GP17 CS0  --------- device 0 CS
Pico GP21 CS1  --------- device 1 CS
Pico GND       ----+---- both device grounds
Pico 3V3       ----+---- 3.3 V device supplies, when suitable
```

Confirm that neither device drives MISO while its chip select is inactive.
For a basic full-duplex loopback, disconnect the peripherals and connect GP19
MOSI to GP16 MISO through 1 kΩ.

### Loopback tests

1. Transfer fixed patterns, walking bits, and seeded pseudo-random buffers.
2. Test lengths around USB and protocol boundaries, including one-byte and
   maximum-size messages and the first rejected oversize message.
3. Sweep representative speeds from 50 kHz through 20 MHz.
4. Exercise modes 0 through 3 and MSB- and LSB-first operation.
5. Verify multi-segment messages, delays, and `cs_change` with a logic analyser.

Loopback validates shifted data but cannot validate chip-select polarity or a
peripheral's interpretation of clock phase. Use the device tests as well.

### SPI flash tests

1. Read and validate the JEDEC identifier on CS0.
2. Read the status registers and confirm expected power-on state.
3. Save one dedicated erase sector.
4. Erase it, program fixed and seeded pseudo-random patterns, and read them
   back at several supported speeds.
5. Exercise a read whose command and data are separate segments while keeping
   chip select asserted.
6. Restore the saved sector if it contained meaningful data.

### Second chip-select tests

Read a stable identity or conversion value from the device on CS1, then
alternate CS0 and CS1 requests. A logic-analyser capture must show only the
selected line asserted for each transaction. Run traffic to both children
from separate processes to cover controller serialization.

### Second controller tests

Enable SPI1 on a pin group separate from SPI0 and wire either another device or
a second loopback. Run transfers on both Linux controllers concurrently and
confirm logic-analyser captures associate each request with the correct SCK,
MOSI, MISO, and chip-select pins. Disable SPI0 while SPI1 traffic continues,
verify the SPI0 children disappear and their five pins become GPIO lines, then
re-enable SPI0 and repeat in the opposite direction.

### SPI acceptance criteria

- Loopback returns every transmitted byte at representative lengths and
  speeds.
- Both chip selects address only their intended peripheral.
- Disabling CS1 leaves CS0 operational and releases only the CS1 pin to GPIO.
- SPI0 and SPI1 remain isolated and can operate concurrently on separate pins.
- Flash erase, program, and readback succeed in the dedicated test region.
- Multi-segment chip-select lifetime and delay fields match the request.
- Unsupported modes and sizes fail before partial device operations occur.
- A peripheral removal or Pico disconnect produces a bounded Linux error.

## UART fixture using a Raspberry Pi console

Use a disposable or development Raspberry Pi, not a production console. Both
systems use 3.3 V UART levels. Cross TX and RX and connect ground only:

| Pico 2 | Raspberry Pi 40-pin header |
| --- | --- |
| GP0 TX, header 1 | GPIO15 RXD, physical pin 10 |
| GP1 RX, header 2 | GPIO14 TXD, physical pin 8 |
| GND | GND, physical pin 6 |

Do not connect either power rail. Configure the Raspberry Pi UART for 115200
baud, eight data bits, no parity, and one stop bit. Enable a serial console or
`serial-getty` on the selected UART and ensure Bluetooth or another overlay has
not claimed it. `systemctl` should be invoked directly; it handles polkit
authentication itself.

### UART tests

1. Open the Pico's stable `/dev/serial/by-id/` link on the host.
2. Reboot the test Raspberry Pi and capture its firmware and kernel console
   output through the Pico CDC ACM device.
3. Wait for the login prompt, transmit input through CDC ACM, and confirm the
   Raspberry Pi receives it.
4. Log in to the disposable test system and run a command whose output includes
   a unique marker; capture the marker on the host.
5. Transfer multiline text and binary byte patterns outside the terminal path
   to check both directions without terminal echo or newline translation.
6. Run UART traffic concurrently with I2C, SPI, and GPIO activity.
7. Change the host CDC line coding and confirm the fixed physical UART remains
   at the documented 115200 8N1 setting.
8. Move the fixture to UART1's configured pins, enable UART1, and repeat the
   bidirectional raw and console tests through its distinct ACM device.
9. Disable each UART in turn. Its ACM device must remain enumerated, host writes
   must produce no pin activity, and its TX/RX pins must appear as GPIO lines.
   Re-enable it and confirm traffic resumes on the remembered route.

### UART acceptance criteria

- Boot and login-console output reaches the host without corruption.
- Host input reaches the Raspberry Pi console and produces the expected output.
- Raw bidirectional transfers preserve bytes when terminal processing is off.
- UART0 and UART1 map to distinct, stable ACM interfaces and physical pins.
- Concurrent bus activity does not stall or corrupt UART traffic.
- Disconnect and reconnect restore the same stable CDC identity.

## ESP32-C3-DevKitM-1 verification

Use this section instead of Pico-specific instance and CDC checks. Flash with
`just flash esp32c3`; it selects the sole supported CP2102N bridge, or requires
an explicit `/dev/serial/by-id/…` path when several are attached. Start the
daemon without `--serial`. `rp-extioctl list` must discover every compatible
attached board, and the identity must remain the eFuse MAC across resets and
host USB paths.

With no external wiring attached, run `scripts/verify-esp32c3-smoke.sh SERIAL`
for a reversible RGB, GPIO, ADC, PWM, and UART tty smoke test. It restores the
default map and turns the RGB LED off before exiting.

### GPIO and onboard RGB LED

Wire GPIO0 through a 1 kΩ resistor to an LED and 330 Ω series resistor to
ground. Verify output high, output low, input, pull-up, and pull-down through
both the CLI and TUI. Move the button and loopback fixtures from the Pico
section across GPIO1–GPIO7, GPIO10, GPIO18, and GPIO19. Test GPIO2 and GPIO9
last, with no external pull that can change their reset strap level.

Set the onboard LED to red, green, blue, white, and off with `rp-extioctl
onboard-led` and the TUI. Assign another signal to GPIO8 and confirm LED
operations report busy without changing that signal. Release GPIO8 and confirm
colour control returns.

### I²C and SPI

For I²C, substitute GPIO4 SDA and GPIO5 SCL in the I²C fixture. Retain the same
2.2–4.7 kΩ pull-ups to 3.3 V. Read a repeated-start register device, exercise
NACK recovery, then move the bus to at least two other GPIO pairs without
reflashing.

For SPI, substitute GPIO6 SCK, GPIO7 MOSI, GPIO3 MISO, GPIO10 CS0, and GPIO1
CS1. Run loopback and peripheral tests, disable CS1 while CS0 remains active,
then move every SPI signal to a second conflict-free map. The supported range
is 78.125 kHz through 20 MHz. Induce a rejected speed or malformed segment
while monitoring both chip selects; neither chip select may remain asserted
after the error.

The C3 profile exposes one I²C and one SPI controller. Do not run the Pico
second-controller cases for this board.

### UART1 tty

Enable UART1 on GPIO6 TX and GPIO7 RX. For local loopback, connect them through
1 kΩ, locate the `ttyRPE` path reported by `rp-extioctl show`, disable terminal
echo, and verify bidirectional byte patterns at 9600, 115200, and 921600 baud.
Exercise 7E1, 8N1, and 8N2. Then replace the loopback with the Raspberry Pi
console fixture: C3 TX to Pi RX, C3 RX to Pi TX, and ground to ground only.

Move UART1 to another pair without restarting the daemon and repeat. Disable
UART1 and confirm its tty disappears and the pins return to GPIO; re-enable it
and confirm a tty returns. Reset and unplug the C3 while the tty is open: the
old descriptor must fail, automatic discovery must reconnect the board, and a
new tty must work without adding `--serial` or restarting the daemon.

### ADC and LEDC PWM

Enable ADC0–ADC4 one at a time on GPIO0–GPIO4. Drive each from the Pico ADC
divider fixture, but keep the measured node between 0 and 2.5 V. Compare raw
IIO and CLI readings at ground, an intermediate voltage, and near 2.5 V; never
use the Pico section's 3.3 V endpoint. Verify enabling an ADC removes its pin
from GPIO and disabling it restores the line.

Route PWM0–PWM5 to six LEDs or logic-analyser channels. Verify independent
routing, enable, duty, polarity, and frequency control. Channels 0/4 share one
LEDC timer and channels 1/5 share another: equal periods must run concurrently,
while a conflicting period must return busy without changing the active
sibling. Channels 2 and 3 must accept independent periods. Repeat saved-state
restoration after daemon restart and C3 reset.

### C3 completion criteria

- Automatic discovery registers all attached supported C3 boards and ignores
  unrelated serial devices.
- Dynamic routing, GPIO bias, ADC, PWM, RGB LED, I²C, SPI, and UART1 work
  without reflashing.
- Strap warnings are visible but do not prohibit deliberate GPIO2/GPIO8/GPIO9
  routes.
- Linux I²C, SPI, GPIO, IIO, PWM, and tty identifiers appear in the CLI and
  TUI while active.
- Reset, unplug, reflash, and daemon restart recover by stable firmware identity
  and restore that board's persisted map and output state.

## Lifecycle and failure tests

Run these after individual buses work:

1. Generate continuous non-destructive traffic on every attached interface.
2. Unplug the Pico. Existing operations must fail within their documented
   deadlines, and its I2C, SPI, and GPIO nodes must disappear.
3. Reconnect it without restarting the daemon. The same USB serial must return,
   new bus numbers may be assigned, and fresh operations must succeed.
4. Reset the Pico during each bus type's operation and confirm the same teardown
   and recovery behaviour.
5. Stop `rp-extiod.service` during active operations. No process may remain
   blocked in uninterruptible sleep due to a module mutex.
6. Start the service again and confirm clean registration.
7. Reload the module while the daemon is stopped and repeat registration.
8. If a second Pico becomes available, run both simultaneously and confirm
   independent identity, device nodes, traffic, disconnect, and reconnect.

Do not use forced module removal during normal verification. A forced removal
can leave callers executing unloaded module code and can require a reboot.

## Installation and kernel coverage

In addition to the development-host run, verify the documented DKMS and service
installation flow on a disposable system. Use `runzero` for commands requiring
privilege and invoke `systemctl` directly.

Build the module against Linux 6.12 LTS and each newer kernel API boundary used
by the compatibility code, as well as the current development kernel. Loading
and exercising the module on at least 6.12 and the current kernel is required;
a headers-only build does not cover registration and teardown behaviour.

Verify DKMS rebuild after a kernel upgrade, boot-time module loading, daemon
startup before and after the Pico is connected, service restart, uninstall,
and removal of all installed files and device nodes.

Also remove `/dev/rp-extio-control` while the daemon is stopped, then start the
service. Its `rp-extioctl prepare` pre-start must load `rp_extio` when needed,
trigger and settle the misc-device udev event, recreate the character device,
and allow the daemon to start. For an uninstalled development module, exercise
the equivalent manual path:

```sh
runzero target/debug/rp-extioctl prepare --module linux/rp_extio.ko
```

## ADC fixture

ADC0, ADC1, and ADC2 use GP26, GP27, and GP28 respectively. Enable each signal
before reading it; an enabled ADC signal owns its pin and removes it from the
GPIO controller.

### ADC potentiometer fixture

Use a 10 kΩ linear potentiometer with its ends connected to 3.3 V and ground.
Connect the wiper through 1 kΩ to GP26, GP27, or GP28:

```text
3V3 ----/\/\/\/\/---- GND
             |
           wiper ---- 1 kΩ ---- ADC input
```

Never let the wiper exceed 3.3 V. For each channel:

1. Route and enable it, for example `rp-extioctl set SERIAL --route adc0=GP26
   --enable adc0`.
2. Discover the board's `iio:deviceN` with `rp-extioctl show SERIAL`.
3. Tie the input through 1 kΩ to ground. Read with `rp-extioctl adc SERIAL 0`
   and `cat /sys/bus/iio/devices/iio:deviceN/in_voltage0_raw`; expect a value
   near zero from both paths.
4. Tie it through 1 kΩ to 3.3 V and expect a value near 4095.
5. Restore the potentiometer and compare the CLI millivolt result with a
   multimeter at low, midpoint, and high settings.
6. Sweep slowly and confirm monotonic readings, then hold several points and
   record the observed noise range.
7. Hold the other two inputs at fixed distinct voltages and alternate reads to
   check channel isolation.
8. Disable the channel. Both direct CLI and IIO raw reads must fail as
   unsupported, and the pin must return to the GPIO list.

The IIO scale is expressed in millivolts per 12-bit count using the documented
3.3 V reference. ADC acceptance requires agreement between CLI and IIO raw
values, monotonic conversion across all three channels, bounded fixed-input
noise, and clean pin release after disable.

## PWM fixtures

The exposed channels are PWM0A, PWM0B, through PWM7A, PWM7B. Each channel can
select one of its valid header aliases, but A and B on the same hardware slice
share a period. Use `rp-extioctl show SERIAL` to discover `pwmchipN` and the
effective routed map.

### PWM LED fixture

The GPIO LED circuit can become a PWM fixture. Route and enable a channel on
that pin, then sweep 0%, 10%, 50%, 90%, and 100% duty at 1 kHz:

```sh
rp-extioctl pwm set SERIAL CHANNEL --frequency-hz 1000 --duty-percent 50
rp-extioctl pwm get SERIAL CHANNEL
rp-extioctl pwm disable SERIAL CHANNEL
```

Confirm the visible brightness trend and measure effective frequency and duty
with a logic analyser. Repeat with `--inverted`, several frequencies across
the usable divider range, and an alias pin for a channel that has two routes.
The disabled output must stop and the route remains owned until the signal is
disabled in the pin map.

Also export the channel under `/sys/class/pwm/pwmchipN`, set period,
`duty_cycle`, polarity, and enable, and confirm the standard Linux path produces
the same waveform. Use a privileged shell or appropriately delegated sysfs
permissions; prefixing `echo` with an elevation command does not elevate shell
redirection.

Enable both channels of one slice with the same period and different duties;
both must run concurrently. While A is active, request a different period on B
and require a `busy` error without disturbing A. Disable A, change B's period,
and confirm the change now succeeds.

On the resistor-protected LED fixture, save a 1 kHz 25% waveform and verify
daemon-owned startup restoration:

```sh
rp-extioctl pwm set SERIAL CHANNEL --frequency-hz 1000 \
  --duty-percent 25 --persist
systemctl restart rp-extiod.service
rp-extioctl pwm get SERIAL CHANNEL
rp-extioctl pwm forget SERIAL CHANNEL
```

Confirm the waveform returns after service restart, Pico reset/reconnect, pin
map reapplication, and `rp-extioctl reload`. In TUI operate mode, select the
enabled PWM signal, press `w`, and confirm its saved settings and `•` marker;
change the live waveform and reload to restore it, then press `x` to forget it.
Attempting to disable or reroute a channel with saved PWM state must fail until
the state is forgotten. `rp-extioctl default SERIAL` must clear both GPIO and
PWM startup entries. Do not use a servo or another mechanically active load for
automatic-start testing.

### PWM servo fixture

A small servo provides a pulse-width integration test, but power it from a
separate suitable supply. Connect the supply ground to Pico ground and connect
only the PWM signal to the Pico. Do not power a servo from the Pico 3.3 V pin.
Test 50 Hz with 1.0 ms, 1.5 ms, and 2.0 ms pulses, staying within the servo's
documented range. Verify endpoint/centre movement, live updates, and that USB
disconnect, daemon stop, pin-map reconfiguration, and PWM disable stop the
signal.

### PWM acceptance criteria

- CLI and standard Linux PWM control agree on enabled state and effective
  period, duty, and polarity.
- Logic-analyser measurements match the reported quantized values within one
  PWM counter step.
- All sixteen logical channels operate on at least one valid route; every alias
  route muxes correctly.
- Same-slice channels share a period and reject conflicting live changes
  without altering the sibling.
- Different slices run independently and survive concurrent ADC, serial-bus,
  and GPIO activity.
- Disable, reconfiguration, daemon stop, and disconnect leave no waveform.
- Saved PWM state returns after daemon restart, Pico reconnect, map
  reapplication, and configuration reload, and can be forgotten independently.

## Clock-output fixture

Route CLOCK0 to GP13 or GP21 and CLOCK1 to GP15. Start with a 1 MHz system-clock
source and measure it with a logic analyser or frequency counter:

```sh
rp-extioctl set SERIAL --route clock0=GP13 --enable clock0 \
  --clock-source 0=system --clock-hz 0=1000000
```

Clock output enable, route, source, and frequency are persisted directly in
the pin map; there is no separate runtime startup record. Restart the daemon
and reset/reconnect the Pico, confirming the clock returns after each. In TUI
operate mode, selecting the clock and pressing `w` must report that it is
already persisted. Disable it in route mode and apply to remove the startup
clock output.

Confirm `show` reports the effective quantized frequency. Repeat representative
frequencies from each source: system, reference, USB, ADC, crystal, ring
oscillator, and low-power oscillator. Reject zero, targets above the selected
source, and divider-overflow targets without changing the active map. Move
CLOCK0 to GP21, exercise CLOCK1 independently, then disable each output and
confirm the pin returns to GPIO and the clock stops.

## Completion record

For each section, record:

- pass, fail, or not run
- exact circuit and peripheral
- commands or test program revision
- expected and observed values
- relevant daemon and kernel logs
- logic-analyser capture filename
- whether the next operation succeeded after an injected failure
- issue or commit resolving any failure

Hardware verification is complete only when every applicable acceptance
criterion has evidence, every failure is either fixed or explicitly moved to a
separate scoped feature, and the fixture can be reconstructed from the record.
