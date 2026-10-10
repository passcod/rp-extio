# Runtime Pico 2 pin routing

## Goal

Replace build-time pin selection with daemon-controlled runtime routing for the
Pico 2's UART0, I2C0, SPI0, software chip selects, and exported GPIO lines.
A stock firmware image must be able to use any valid conflict-free RP2350A
route without rebuilding or reflashing.

## Configuration ownership

Add daemon configuration keyed by the Pico's stable USB serial number.
The configuration describes peripheral signal pins rather than the derived
GPIO list.
The daemon validates syntax locally, while firmware remains authoritative for
RP2350 mux validity and pin ownership.

Use the daemon configuration as the persistent source of truth.
Reapply it after every firmware reset or USB reconnect before registering Linux
buses.
Keep a safe firmware default so an unconfigured board remains usable.

## USB protocol lifecycle

Add a pin-configuration operation available after USB framing negotiation but
before Linux bus registration.
The request carries the complete desired map and a configuration generation.
The reply returns the accepted map or a precise conflict or invalid-route
status.

Firmware applies a configuration atomically: quiesce UART and bus activity,
release all old outputs, construct the new peripheral routes, reset newly
exported GPIOs to inputs, then publish the new capabilities.
A rejected configuration leaves the previous map intact.
HELLO after configuration reports the active map and generation.

Changing configuration on an already registered board requires the daemon to
tear down that board's Linux instances, apply the new map, renegotiate
capabilities, and register replacement instances.
No in-flight operation may cross a routing change.

## Firmware pin abstraction

Replace compile-time concrete pin ownership with runtime-owned, type-erased
pins after one central uniqueness and mux validation pass.
Use Embassy type erasure where its public APIs support it.
Add narrow RP2350 UART and SPI construction adapters where Embassy's pin marker
traits require concrete types; keep register access and safety invariants in
the board layer.

The board owns all externally usable pin tokens and lends each pin to exactly
one active peripheral or GPIO line.
Reconfiguration moves ownership through a neutral disconnected state so no
safe Rust alias or electrically driven stale route remains.

## Daemon interface

Support a configuration file and command-line override suitable for both a
single development board and multiple serial-numbered boards.
Report the active map in startup logs and expose a command that prints the map
without registering buses.
Reject partial maps, duplicate pins, unknown board serials where strict mode is
requested, and configuration changes that race with shutdown.

Retain the current build-time variables only as firmware-default selection and
recovery tooling.
Document runtime configuration as the normal interface once this work lands.
