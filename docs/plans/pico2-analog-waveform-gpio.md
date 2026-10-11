# Pico 2 analog, waveform, clock, and GPIO controls

Extend the board configuration from serial-bus routing into a complete Pico 2
header pin-ownership model. Keep UART, I2C, and SPI routes, and add the three
exposed ADC inputs on GP26 through GP28, the sixteen RP2350A PWM outputs for
slices 0 through 7 channels A and B, and the two exposed general-purpose clock
outputs. Clock output 0 may use GP13 or GP21 and clock output 1 uses GP15. Treat
enabled ADC, PWM, and clock functions as pin owners which exclude GPIO and every
other enabled function. Preserve disabled functions' remembered settings
without claiming their pins.

Represent PWM channels by hardware slice and channel rather than inventing an
independent PWM for every header pin. Allow each channel to select either of its
exposed aliases where present. Enforce the RP2350 constraint that the A and B
channels of one slice share a period, returning a busy/conflict result rather
than silently changing an active sibling. Represent clock outputs with a
source, target frequency, and route; default newly enabled clocks to a safe
1 MHz system-clock-derived output. Validate source and divider ranges in shared
wire code and return the effective frequency after quantization.

Bump the USB and kernel protocol major versions together. Extend capability
negotiation and configuration encoding so the daemon learns ADC channels, PWM
channels and routes, clock routes and effective settings, and the remaining
GPIO pins from firmware rather than assuming Pico defaults. Migrate persisted
version-2 board maps to the expanded configuration with ADC, PWM, and clock
outputs disabled. Keep old firmware and old kernel modules rejected explicitly
instead of allowing incompatible layouts to register.

Add firmware operations for single ADC conversions, PWM apply/query, clock
configuration, and GPIO bias. Use the RP2350 ADC's 12-bit result and report the
3.3 V reference as an IIO scale. Program PWM slices and pin muxes dynamically,
preserving an active sibling channel when the other channel changes. Program
clock outputs from the supported RP2350 sources and release every added pin
function cleanly during reconfiguration and USB disconnect. Extend GPIO
snapshots to return levels, directions, pull-up state, and pull-down state for
all current GPIO lines in one transaction.

Extend the out-of-tree kernel module with a three-channel IIO voltage device
whose direct reads cross the existing userspace-backed transaction path. Add a
sixteen-channel PWM controller whose channels correspond to slice 0A through
slice 7B and whose apply callback transports period, duty cycle, polarity, and
enable state. Disabled/unrouted channels return unsupported. Add GPIO
`set_config` support for bias disabled, pull-up, and pull-down, and use bulk GPIO
snapshot data for multi-line reads where the GPIO API permits it. Register and
destroy the added IIO and PWM instances atomically with each board's existing
I2C, SPI, and GPIO devices.

Expose the added registrations and configuration through daemon board status,
the scripting CLI, and the pinout TUI. Let the TUI assign, enable, disable, and
display ADC, PWM, and clock signals alongside the existing peripheral signals;
retain grouped movement for serial buses while treating analog, PWM, and clock
signals individually. Add CLI configuration flags for the new routes and clock
settings, ADC read output in raw counts and millivolts, and discoverable IIO and
PWM device numbers.

Add daemon control requests for ADC reads, PWM state and apply, GPIO snapshot,
input configuration, and persistent output level. Classify snapshots, PWM
queries, and ADC conversions as read-only; classify direction, bias, output,
PWM apply, clock, and persisted routing changes as Polkit-protected mutations.
Build CLI conveniences on those requests: read ADC channels, configure PWM
frequency/duty/polarity, inspect all GPIO levels/directions/biases, drive a pin
high or low until changed, configure an input pull, express LED on/off including
active-low wiring, and wait for a button level or edge with an optional timeout.
Reject pins or channels not enabled in the current map and state clearly that
daemon-owned output state lasts until it is changed, the pin map is
reconfigured, or the device disconnects.

Update simulation support for deterministic ADC values, PWM state and shared
slice constraints, clock state, GPIO pulls, and expanded snapshots. Update the
README and hardware-verification guide with standard Linux IIO, PWM, and
libgpiod usage, the higher-level CLI commands, pin conflicts, voltage limits,
PWM slice sharing, clock-output routes, and breadboard procedures for the
existing potentiometer, LED, servo, and button fixtures.
