# Multi-peripheral routing and board orientation

## Goal

Expose both RP2350A UART, I2C, and SPI instances, allow each peripheral instance
to be enabled or disabled independently, and let the pinout TUI rotate the board
view for an upside-down physical mounting.
Disabled peripherals release all of their pins into the exported GPIO set while
retaining their configured routes for later re-enablement.

The firmware default keeps UART0, I2C0, and SPI0 enabled so an upgraded board
starts with the existing behavior.
UART1, I2C1, and SPI1 start disabled with mutually compatible remembered routes,
allowing all six instances to be enabled without first resolving default-pin
conflicts.

## Configuration and wire model

Replace the single-instance nine-signal pin map with routes for UART0, UART1,
I2C0, I2C1, SPI0, and SPI1 plus an enabled bit for each peripheral instance.
Each UART has TX and RX, each I2C controller has SDA and SCL, and each SPI
controller has MISO, SCK, MOSI, CS0, and CS1.
Validation checks mux compatibility for every remembered route, checks paired
I2C pins, and checks duplicate pins only among enabled peripherals.
GPIO derivation excludes only enabled peripheral signals and therefore produces
between 8 and 26 GPIO lines.

Bump the USB protocol major version and encode the complete configuration in
configure-pins requests, replies, and a mandatory pin-configuration capability.
HELLO emits one capability record for every enabled bus instance and identifies
it by hardware bus ID.
Transactions retain their bus ID through the kernel, daemon, USB request, and
firmware dispatch path so both instances reach the matching controller.

Persist a version-2 daemon configuration containing the complete routes and
enabled mask.
Load version-1 files by migrating their existing routes into instance 0 over the
new defaults, leaving instance 1 disabled, and write version 2 on the next save.

## Firmware and Linux devices

Own and configure SPI0/SPI1 and UART0/UART1 in the Pico firmware.
Route I2C transactions through the selected bit-banged bus pins, route SPI
transactions through the selected hardware block, and configure or neutralize
pins whenever an instance is enabled or disabled.
Create two permanent CDC ACM functions, one per UART hardware block; a disabled
UART accepts no hardware traffic while its pins participate in GPIO.

Bump the kernel ABI and add hardware bus IDs to I2C and SPI registrations.
Allow the GPIO registration to describe any ordered subset of the 26 Pico 2
header GPIOs.
Have the daemon dynamically register only enabled I2C and SPI instances, always
register the variable-width GPIO controller, and report Linux bus numbers by
hardware instance.
Update the simulator to expose and distinguish the dynamic adapter/controller
set.

## CLI and TUI

Extend `rp-extioctl set` with instance-qualified pin options and explicit
enable/disable options for all six peripheral instances.
Keep the existing unqualified UART, I2C, and SPI pin options as aliases for
instance 0.
Make list, show, JSON responses, and documentation identify both hardware
instances and their enabled state.

Expand the pinout TUI signal selector and legend to all 18 routes.
Grouped editing moves the selected UART pair, I2C pair, or SPI data group within
its own hardware instance, while chip selects remain independent.
Add a key that enables or disables the selected signal's peripheral instance;
enabling rejects conflicts atomically, while disabling immediately previews the
released pins as GPIO.

Add a board-flip key that rotates the physical view by 180 degrees: SWD moves to
the top, USB to the bottom, header row order reverses, and left/right sides swap.
Arrow navigation continues to follow screen direction after rotation, and the
orientation is presentation-only rather than persisted board configuration.

## Documentation

Document both instances' valid RP2350A routes, enabled defaults, variable GPIO
line mapping, the two permanent UART ACM interfaces, instance-aware CLI usage,
peripheral toggling, and board rotation controls.
Update the hardware verification guide with second-instance and disable/re-enable
checks so every exported Linux device and released GPIO route can be exercised.
