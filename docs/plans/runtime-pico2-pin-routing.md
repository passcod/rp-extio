# Runtime Pico 2 pin routing

## Goal

Replace build-time pin selection with daemon-controlled runtime routing for the
Pico 2's UART0, I2C0, SPI0, two software chip selects, and exported GPIO lines.
A stock firmware image must be able to use any valid conflict-free RP2350A
route without rebuilding or reflashing.

Provide `rp-extioctl` as both a scriptable command-line client and a full-screen
terminal UI.
Both clients communicate with the running daemon, which remains the sole owner
of the USB bridge and Linux control device.

## Shared pin-map model and USB protocol

Move the complete Pico 2 pin map, header-pin set, signal identifiers, route
validation, GPIO derivation, and compact wire encoding into the shared wire
crate.
The map contains UART TX/RX, I2C SDA/SCL, SPI MISO/SCK/MOSI, and two SPI chip
select pins; the 17 GPIO lines are derived in physical-pin order from the
remaining header pins.
Validation distinguishes an unavailable header pin, an invalid peripheral
route, a mismatched I2C pair, and a duplicate assignment, including the signal
and conflicting pin in the error.

Add a `ConfigurePins` USB operation carrying a complete map and a 64-bit
configuration generation.
Its success reply echoes the accepted generation and map.
Its error reply carries a stable validation-error encoding in addition to the
USB status.
Extend HELLO's general capability with the active generation while retaining
the existing per-interface pin reporting.

## Firmware pin ownership and reconfiguration

Replace generated concrete-pin constructors with one board-owned collection of
type-erased header pins.
Use SIO-backed pins for software I2C, chip selects, and GPIO.
Add narrow RP2350A register-level adapters for SPI0 and UART0, because
Embassy's marker traits cannot construct those peripherals from runtime-erased
pins.
Keep register access, mux selection, and the invariant that each physical pin
has exactly one owner inside the Pico 2 board module.

Validate a requested map before changing hardware.
To apply it, disable UART0 and SPI0, release open-drain and chip-select outputs,
return every header pin to an input in the neutral SIO function, configure the
new peripheral muxes and electrical directions, reset all newly exported GPIO
lines to inputs, and only then publish the new map and generation.
A rejected map leaves the active hardware, map, and generation unchanged.

Replace the concrete Embassy buffered-UART instance with cooperative CDC/UART
tasks backed by the runtime UART0 adapter.
The adapter continues using 115200 8N1 and follows the active pin map after a
configuration change without rebuilding the USB device.

Retain the current `RP_EXTIO_*` build variables only to select the safe startup
and recovery default.
Simplify generated firmware code to a default map rather than concrete pin
types.

## Daemon lifecycle and persistence

Add a daemon state manager that owns connected USB backends, Linux
registrations, GPIO polling tasks, active generations, and serial-number keyed
desired maps.
All hotplug and control requests pass through this task so configuration cannot
race disconnect or shutdown.

Load persistent JSON configuration from `/etc/rp-extio/config.json` by default,
with daemon flags for alternate config and control-socket paths.
Write accepted changes atomically in the configuration file's directory.
Support strict configuration loading that rejects entries for USB serials not
present during initial discovery; non-strict mode retains desired maps for
boards that are currently disconnected.

On connection, negotiate HELLO, apply the desired map before Linux bus
registration, renegotiate capabilities, and log the active generation and
signal map.
On a live change, stop GPIO polling, destroy that board's Linux instances,
apply the complete map, renegotiate, and register replacement instances.
If firmware rejects a map, restore the existing registration unchanged.
If transport becomes uncertain after teardown, remove the board and let USB
reconnection perform normal recovery.
No I2C, SPI, or GPIO operation may cross a routing change.

## Daemon control API

Serve newline-delimited JSON on `/run/rp-extiod.sock` by default.
Expose operations to list connected and configured boards, inspect one board,
apply a generation-checked complete map, restore the firmware default, and
reload persistent configuration.
Responses include connection state, active and desired maps, generation,
allocated Linux bus and GPIO instance information where available, and
structured validation or lifecycle errors.

Use peer credentials and socket filesystem permissions for local access
control.
Create the socket with administrator-writable permissions by default and add a
daemon option for an operator group so installations can delegate routing
without granting access to the kernel control device or USB device.

## CLI and TUI

Add an `rp-extioctl` binary that connects to the daemon socket.
Its noninteractive commands are `list`, `show`, `set`, `default`, and `reload`.
`set` accepts individual signal options for convenient shell use, fetches the
current map, applies those edits locally, validates the resulting complete map,
and submits it with the observed generation so concurrent edits fail instead
of overwriting one another.
Support JSON output for stable scripting and concise human-readable output by
default.

Add `rp-extioctl tui`, optionally selecting a serial number.
The TUI lists boards and presents every peripheral signal with valid candidate
pins, marks conflicts before submission, shows derived GPIO lines and Linux
device assignments, and supports apply, restore-default, reload, refresh, and
quit actions.
It must restore terminal state on normal exit and errors.

## Documentation and packaging

Document the persistent file format, daemon flags, socket permissions, CLI
commands, TUI controls, generation conflicts, bus renumbering, and runtime pin
routing as the normal workflow.
Update the service packaging to create the control socket and pass the default
configuration path.
Update hardware-verification instructions so one stock UF2 is exercised across
multiple valid maps, invalid and conflicting changes leave the old map active,
connected peripherals move with their configured pins, UART CDC follows its
new route, configuration survives daemon restart, and firmware defaults return
after an explicit reset command.
