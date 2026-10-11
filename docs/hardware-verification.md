# Hardware verification

This guide verifies `rp-as-extio` against physical circuits and peripherals.
It complements the automated simulator and protocol tests by exercising the
complete Linux kernel, daemon, USB, firmware, and electrical path.

Record the commit, firmware build, host kernel, Pico serial number, wiring, and
peripheral part numbers for each run. Linux bus numbers can change after any
reconnect, so discover them again rather than carrying numbers between steps.

## Safety

- Use 3.3 V logic only. Never drive a Pico GPIO above 3.3 V.
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
Pico USB serial:
Pico USB path:
I2C0 / I2C1 adapters:
SPI0 / SPI1 controllers and children:
GPIO chip:
UART0 / UART1 CDC ACM devices:
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
systemctl status rp-extiod.service
rp-extioctl list
```

With the startup map, the board must have a stable USB serial, two CDC ACM
devices, one I2C0 adapter, two SPI0 `spidev` children, and one 17-line GPIO
chip. UART1 remains enumerated over USB while disabled; I2C1 and SPI1 have no
Linux buses until enabled. Run the ignored USB hardware test before attaching a
peripheral:

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
5. Move UART0 TX/RX, reconnect the signal wires without reconnecting USB, and
   verify both CDC-to-UART and UART-to-CDC traffic. Restore the startup map
   before enabling all remembered instance-1 routes below.
6. Enable I2C1 on its remembered GP10/GP11 route. Confirm a second I2C adapter
   appears, each adapter reaches only its wired bus, and the GPIO count falls
   from 17 to 15.
7. Enable SPI1 on its remembered GP12/GP14/GP15, CS GP13/GP20 route. Confirm a
   second controller with two children appears, exercise both SPI instances,
   and confirm the GPIO count falls to 10.
8. Enable UART1 on GP8/GP9, exercise the second ACM path in both directions,
   and confirm the GPIO count falls to 8. The two ACM devices must have remained
   enumerated throughout.
9. Disable each second instance again. Its remembered routes must stay visible,
   its I2C or SPI Linux bus must disappear, and its pins must return to the GPIO
   list in ascending GPIO order.
10. Disable all six peripherals. Confirm there are no I2C or SPI buses for the
    board, both ACM devices remain, and the GPIO controller exposes all 26
    header GPIOs. Restore the startup state with `rp-extioctl default SERIAL`.
11. Attempt an invalid mux route and a duplicate enabled assignment. The
    client or firmware must reject each one while the previous buses and active
    map remain usable.
12. Change remembered routes while an instance is disabled, including a route
    overlapping an enabled peripheral. The edit may be saved, but enabling the
    conflicting instance must fail atomically without changing the active map.
13. Start two `rp-extioctl set` operations from the same observed generation.
   One may apply; the other must report a generation conflict.
14. Restart the daemon and then reset the Pico. The persistent map must be
   reapplied in both cases before Linux buses appear.
15. Run `rp-extioctl default SERIAL`; the startup map must return, its
    persistent entry must disappear, and Linux buses must be rediscovered.
16. Run `rp-extioctl tui SERIAL`, exercise every signal selector, grouped moves
    within both hardware instances, the `e` enable toggle, conflict display,
    apply, refresh, reload, and default action. Press `f` and confirm the board
    mirrors left-to-right as viewed belly up, without exchanging the USB and
    SWD ends, and that arrow navigation remains screen-relative. Exit with both
    `q` and an interrupt and confirm the terminal is restored.

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

### GPIO acceptance criteria

- Every exposed line can become an input and an output.
- Every protected pair reads both driven levels in both directions.
- Deterministic 100 ms transitions produce the requested rising and falling
  events without missing or inventing stable transitions.
- The LED and button circuits behave according to their schematics.
- Invalid offsets and attempts to claim an already-owned line fail promptly.
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

## ADC and PWM stretch fixtures

ADC and PWM are not exposed by the current USB protocol, daemon, or Linux
kernel module. GP26, GP27, and GP28 currently appear only as digital GPIOs.
Software-toggling those lines through the GPIO bridge is not a substitute for
hardware PWM, and digital reads cannot verify ADC values.

Keep these circuits as fixtures for separate ADC/IIO and PWM implementations.

### ADC potentiometer fixture

Use a 10 kΩ linear potentiometer with its ends connected to 3.3 V and ground.
Connect the wiper through 1 kΩ to GP26, GP27, or GP28:

```text
3V3 ----/\/\/\/\/---- GND
             |
           wiper ---- 1 kΩ ---- ADC input
```

Future ADC verification should measure ground, 3.3 V, midpoint, monotonic
sweeps, repeated-sample noise, channel isolation, and conversion scaling
through a Linux IIO device. Never let the wiper exceed 3.3 V.

### PWM LED fixture

The GPIO LED circuit can become a PWM fixture. Future PWM verification should
sweep duty cycle from 0% through 100%, check frequency and duty cycle with a
logic analyser, confirm the visible brightness trend, and exercise multiple
channels concurrently.

### PWM servo fixture

A small servo provides a pulse-width integration test, but power it from a
separate suitable supply. Connect the supply ground to Pico ground and connect
only the PWM signal to the Pico. Do not power a servo from the Pico 3.3 V pin.
Future verification should test endpoint and centre pulse widths, frequency,
update behaviour, and safe output state during disconnect.

Adding ADC requires a new board capability and transaction protocol plus a
Linux IIO registration path. Adding PWM likewise requires a new capability,
protocol operations, and Linux PWM controller. Those are separate feature
designs rather than extensions of the current digital-GPIO test.

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
