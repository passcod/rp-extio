# ESP32-C3-DevKitM-1 support

Add the ESP32-C3-DevKitM-1 v1.6 as a second rp-extio board without weakening
the existing Pico 2 support. Treat board identity, routing rules, physical
layout, and peripheral capabilities as negotiated data rather than extending
the existing Pico-specific constants with more special cases.

## Board descriptions and wire protocol

Introduce a stable board kind in the wire crate, initially covering Pico 2 and
ESP32-C3-DevKitM-1. Define a board profile for each kind containing its exposed
GPIOs, physical header rows, supported signals, legal routes, default map,
reserved transport pins, warning flags, ADC properties, PWM topology, and clock
output and onboard LED support. Keep descriptions bounded and allocation-free
so firmware can use the same validation logic as the daemon and control tools.

Extend HELLO with a versioned board-description capability. It must identify
the board kind and transport, carry the stable device identifier, and provide
enough capability data for a newer daemon to reject an unknown profile rather
than applying Pico assumptions. Retain the current pin-map encoding as a
protocol superset: unsupported entries remain disabled and are rejected if
enabled. Bump the protocol major version because old daemons cannot safely
interpret a C3 map.

Move pin-map validation, GPIO-line derivation, defaults, signal visibility,
and signal route lookup behind the selected board profile. Preserve the Pico 2
routes and defaults byte-for-byte. Add distinct validation errors for an
unsupported signal and a reserved transport pin. Surface non-fatal pin
warnings separately so GPIO2, GPIO8, and GPIO9 can be selected deliberately
while the CLI and TUI explain their ESP32-C3 strapping or onboard-device roles.

The ESP32-C3 profile exposes header GPIO0 through GPIO10 except transport pins
GPIO20 and GPIO21. It provides one logical I2C controller backed by I2C0, one
logical SPI controller backed by SPI2, one external UART backed by UART1, ADC
channels on GPIO0 through GPIO4, and six independently routable LEDC PWM
channels. GPIO matrix signals may route to any exposed, non-reserved GPIO where
the silicon permits it. Logical I2C1 and SPI1 remain unsupported. Logical
UART0 is the control transport and remains unsupported as an external signal;
UART1 is exposed. GPIO18 and GPIO19 remain ordinary header GPIOs unless native
USB Serial/JTAG is deliberately enabled in a future profile revision.

Do not advertise general-purpose clock outputs for the first C3 profile. The
C3 does not offer a Pico-equivalent GP clock block through the selected HAL;
clock signals in a persisted map must remain disabled and produce an explicit
unsupported-signal error if enabled. Keep this a profile capability rather than
a firmware conditional so a later implementation can add a board-specific
clock source without another protocol redesign.

Describe the Pico 2's active-high monochrome LED on GP25 and the C3 board's
addressable RGB LED on GPIO8 as named onboard devices. Negotiate LED kind and
colour capabilities explicitly and add a get/set operation whose state uses
RGB components, with monochrome boards accepting only off or white. Keep GP25
outside the Pico header GPIO map. GPIO8 remains a legal C3 route with its
existing warning; an active route on GPIO8 makes the onboard LED operation
return busy rather than disturbing that peripheral.

## Transport and device management

Add a serial transport alongside the existing USB bulk transport. Use COBS
frames delimited by zero bytes, containing a framing version, encoded body
length, the existing request or reply header and payload, and a CRC-32. Limit
decoded frames to the existing maximum payload. Discard malformed frames and
resume at the next delimiter so ESP ROM boot messages, partial writes, reset
output, and reconnects cannot permanently desynchronise the link.

Configure UART0 at a fixed documented baud rate through GPIO20/GPIO21 and the
board's onboard USB-to-UART bridge. Discover supported USB-to-UART bridge
identities through sysfs and watch for them throughout the daemon lifetime.
Open each candidate exclusively and confirm rp-extio firmware with a framed
HELLO before registering it; never probe unrelated TTYs. Keep repeatable
`--serial` options as explicit additions for compatible bridges not yet known
to the discovery table. The stable rp-extio identity comes from the ESP32-C3
eFuse base MAC returned by firmware, not from the CP210x USB descriptor or host
device name.

Refactor device management around transport-independent connection keys and a
common request backend. USB and serial hotplug workers feed the same negotiated
device lifecycle, startup-state restoration, kernel registration, control API,
and disconnect cleanup. Serial workers reopen attached candidates after reset
or framing failure with bounded backoff and stop when the sysfs device is
removed. Only one live connection may claim a stable board identity. Extend
the systemd unit and installation documentation with serial-device access
requirements while retaining root service operation as the default.

## ESP32-C3 firmware

Add a separate no-std ESP32-C3 firmware crate using the current `esp-hal`
release and the ESP-IDF-compatible bootloader image format. Share protocol
dispatch and board-profile validation with the Pico firmware, but keep HAL
ownership and transport loops target-specific. Derive a stable uppercase hex
identifier from the eFuse base MAC and return it during HELLO.

Own UART0 permanently for framed host transport. Implement bounded blocking or
async reads with delimiter resynchronisation, CRC checking, request dispatch,
and complete reply writes. Firmware log output must never share the framed
UART after startup; panic/reset diagnostics may precede a new delimiter and
are handled as transport noise by the daemon.

Implement dynamic pin application transactionally. Validate the complete map
before releasing the old one, return all currently owned pins to a safe input
state, then construct the requested I2C0, SPI2, UART1, ADC1, LEDC, and GPIO
drivers using erased/dynamic pins. A failed construction restores the previous
map or the profile default. Keep GPIO20/GPIO21 reserved throughout and attach
strap/onboard-RGB warnings to GPIO2/GPIO8/GPIO9 without prohibiting them.

Implement I2C transfers at the protocol's advertised speed and supported
address modes, SPI messages including modes, chip-select polarity and optional
second chip select, GPIO direction/read/write/bias/snapshot, ADC1 conversions
on GPIO0 through GPIO4 with the reported resolution/reference, and LEDC PWM
state for six channels. Drive the GPIO8 addressable RGB LED with the requested
colour when the pin is not routed to another signal. Unsupported operations and
parameter combinations must return protocol status rather than panic.

Add an external UART transaction capability to the common protocol for RX,
TX, line configuration, and modem-free status. Implement it over ESP UART1.
Extend the Linux module with one dynamically registered tty port per advertised
UART and route tty writes, reads, termios changes, disconnects, and backpressure
through daemon transactions. Keep the Pico's direct CDC ACM UARTs unchanged;
its HELLO marks them as host-native UARTs so the daemon does not register tty
ports for them.

## Host interfaces

Carry board kind, transport kind, supported-signal mask, warnings, and physical
layout through `NegotiatedConfig`, persisted board records, and read-only
control responses. Migrate existing configuration files by assigning legacy
boards the Pico 2 profile. Key persisted state by firmware-reported stable
identity so swapping between two programmed C3 boards does not share routes.

Generalise CLI map parsing and display to the selected board profile. Commands
must reject unavailable instances and routes before mutation, display reserved
transport pins, and emit warnings when selecting strapping or onboard-device
pins. `default` selects the connected board's profile default rather than the
Pico map.

Add CLI and TUI controls for reading and setting the negotiated onboard LED.
The CLI accepts on/off for either board and RGB colours where supported; the
TUI shows the live LED state and offers the same board-appropriate controls.

Render the TUI from profile-provided header rows. Add a two-column
ESP32-C3-DevKitM-1 v1.6 diagram matching J1/J3, including power, ground, reset,
GPIO, ADC, transport, strap, and onboard-RGB annotations. Belly-up view mirrors
the physical board face in the same manner as the Pico view. Hide unsupported
signals and channels, prevent selection of reserved pins, retain the active
signal cursor across apply, and show warnings before applying a hazardous but
legal route.

Extend kernel registration metadata and control responses so active Linux bus,
GPIO, IIO, PWM, and tty identifiers remain visible for either board and either
transport. Avoid board names in kernel ABI decisions; registrations are driven
by negotiated capabilities.

## Build, installation, and documentation

Add target-specific build scripts and `just` recipes for building and flashing
the ESP32-C3 image with a locally verified Espressif flashing tool. Keep
`just flash` as the Pico command and add clearly named C3 recipes that require
an explicit serial path when more than one candidate exists. Discovery must
identify supported bridge hardware before performing the framed firmware
handshake, so unrelated serial devices are not touched.

Document the DevKitM-1 v1.6 header pinout, UART0 transport reservation,
3.3-volt electrical limits, ADC limitations, strapping-pin boot hazards,
GPIO8's onboard addressable RGB LED, automatic discovery, build/flash flow,
and differences from the Pico transport and peripheral set. Extend the
hardware-verification guide with C3 wiring for I2C, SPI, UART1 loopback or peer,
GPIO input/output/bias, ADC, and LEDC PWM, while keeping the existing Pico
instructions distinct.
