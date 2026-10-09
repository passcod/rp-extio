# Pico 2 USB I2C and SPI bridge

## Goal and decisions

Expose I2C and SPI buses from a USB-connected Pico 2 as ordinary Linux
`/dev/i2c-*` and `/dev/spidev*` devices. I2C and SPI are the first required
interfaces. Add UART through CDC ACM after the core buses; additional
peripherals need their own requirements.

Use Rust for the firmware, shared protocol, and host daemon. A daemon is an
acceptable runtime dependency. Its hardware backend talks to the Pico 2 over
USB; its simulated backend creates the same Linux bus interfaces without
hardware, so software that normally talks to Raspberry Pi buses can be run
against scripted peripherals. Prefer Rust for the kernel component, subject
to an explicit feasibility gate. Consider a mixed Rust/C module if thin C
registration wrappers make it maintainable. If the necessary out-of-tree Rust
kernel support is unavailable or too fragile across supported kernels, keep
C confined to bus registration, kernel callbacks, and the userspace request
channel.

The initial board target is the standard Raspberry Pi Pico 2 with documented
pin assignments. Keep protocol handling and bus semantics independent of the
board layer so other RP2350 boards and later MCU families can provide their
own pin and peripheral bindings. The existing C prototype in this repository
was created before this plan was agreed; replace it rather than using its
architecture or protocol as a baseline.

## Architecture

```text
Linux I2C and SPI clients
        │
        ▼
virtual i2c_adapter and spi_controller  ──  /dev/i2c-* and /dev/spidev*
        │
        ▼
small kernel request channel
        │
        ▼
Rust daemon ── USB backend ── Rust Pico 2 firmware ── hardware buses
           └── simulated backend ── scripted I2C/SPI peripherals
```

The kernel component creates and removes bus instances at the daemon's
request. A bounded, versioned channel carries complete I2C transactions and
SPI messages to the daemon and returns data or Linux errors. The channel
must wake pending operations when the daemon exits, enforce ownership and
timeouts, and prevent stale replies from reaching a later transaction. The
daemon owns USB discovery, device identity, reconnects, and transport state;
the kernel component contains no board-specific USB code.

`io_uring` may be useful to the daemon later, but it does not register I2C or
SPI controllers. eBPF does not provide the required controller registration
either. Neither is part of the initial kernel boundary.

## Implementation sequence

1. **Resolve kernel language and channel boundary.** Inspect the target
   kernel's Rust support for a loadable module that registers an
   `i2c_adapter`, a `spi_controller`, SPI child devices, and a userspace
   request channel. Establish which supported kernel/toolchain combinations
   can build that module through DKMS. Compare a pure Rust module, a mixed
   Rust/C module with thin registration wrappers, and a small C module.
   Choose the smallest maintainable C surface with a stable userspace ABI;
   keep transport, device policy, and backend logic in Rust.
2. **Specify shared transaction semantics and protocols.** Define versioned
   kernel-to-daemon and daemon-to-firmware messages with length limits,
   request IDs, capability negotiation, errors, and disconnect behavior.
   Preserve Linux I2C combined transfers and repeated starts. Map SPI
   transfer sequences, mode, speed, chip select, full-duplex bytes, and
   `cs_change` without silently altering them. State which I2C flags and SPI
   options are supported or rejected. Keep the firmware protocol independent
   of the kernel request-channel ABI.
3. **Build the Rust firmware.** Use an RP2350-capable Rust HAL and USB stack,
   with a Pico 2 board layer for pin assignments. Put wire parsing and bus
   dispatch in reusable `no_std` crates and use explicit bounded buffers.
   Implement I2C and SPI transactions, bus error recovery, USB reconnect
   behavior, and a bootable UF2 image. Define the board-facing traits from
   the operations the Linux bridge needs; use `embedded-hal` interfaces where
   they preserve those operations.
4. **Build the kernel bus endpoint.** Register I2C and SPI controllers and
   create SPI child devices suitable for `spidev`. Implement the bounded
   request channel, transaction lifetime and cancellation, daemon ownership,
   teardown, and dynamic bus creation. Package the out-of-tree module for
   DKMS using the language chosen in step 1.
5. **Build the Rust daemon and both backends.** The USB backend discovers
   supported boards, matches firmware capabilities, and routes bus requests
   over USB. The simulated backend exposes configurable I2C addresses and SPI
   chip selects with scripted reads, writes, responses, and errors through the
   same Linux bus nodes. Provide a way for local test processes to load and
   reset scenarios. Both backends share the same request dispatcher and
   lifecycle handling.
6. **Complete integration and documentation.** Define stable device identity
   and USB permissions, service startup, reconnect behavior, DKMS install and
   removal, Pico 2 flashing, pin wiring, voltage and pull-up requirements,
   and examples using `i2c-dev` and `spidev`. Replace the premature C
   prototype and its claims with the agreed implementation and documentation.
7. **Add UART as the next interface.** Use USB CDC ACM where feasible so
   Linux's existing serial driver provides `/dev/ttyACM*`. Keep UART optional
   in board capability reporting; it must not delay the usable I2C/SPI
   bridge.

## Boundaries

USB full-speed and request latency make this a functional development bridge,
not a timing-equivalent replacement for a local Raspberry Pi controller.
The firmware reports its supported operations and limits rather than assuming
every later board has the Pico 2's buses or pins. Use a development USB ID
only locally; distribution requires an allocated VID/PID.
