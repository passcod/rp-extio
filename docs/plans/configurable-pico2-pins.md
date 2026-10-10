# Configurable Pico 2 peripheral pins

## Goal

Allow a firmware build to select any valid Pico 2 header routing for I2C0,
SPI0, UART0, and the two software-controlled SPI chip selects without editing
Rust source.
The firmware must expose every remaining usable header GPIO as a digital line,
and every host layer must describe that generated map rather than assuming the
original GP4/GP5 I2C layout.

## Firmware build configuration

Add a firmware build script that reads optional `RP_EXTIO_*` pin environment
variables.
The defaults preserve the existing GP0/GP1 UART, GP4/GP5 I2C, and
GP16–GP21 SPI layout.
Support configuration of UART0 TX/RX, I2C0 SDA/SCL, SPI0 MISO/SCK/MOSI, and
SPI CS0/CS1.

Validate each selected pin against the RP2350A peripheral mux, reject duplicate
assignments, and reject pins that are not externally usable on a Pico 2.
Generate typed pin-selection macros for Embassy and a pin-description constant
for the firmware protocol.
Derive the 17 exported GPIO pins as the ordered complement of the nine assigned
peripheral pins within GP0–GP22 and GP26–GP28.

## Capability protocol

Make the firmware dispatcher obtain its pin description from the board rather
than fixed crate constants.
Use the reserved bytes in the I2C capability to report SDA and SCL.
Extend the SPI capability with SCK, MOSI, MISO, and one pin per chip select.
Continue reporting UART and GPIO pins in their existing capability records.
Bump the USB protocol minor version for the new capability fields while
retaining major version 1.

The daemon must validate that the complete capability map uses distinct Pico 2
header pins and that each peripheral route is valid.
It must accept both the generated default map and other valid maps.

## Kernel GPIO description

Extend ABI major 1 with a new minor revision of the GPIO-create ioctl that
carries the physical pin number for each line.
Replace the kernel module's fixed 17-line name table with names owned by each
GPIO instance and derived from the supplied pin numbers.
Keep the maximum at the Pico 2's 17 unassigned header pins and validate line
count, pin range, and uniqueness before registration.

The daemon must pass the GPIO capability pin list through this ioctl so
`gpioinfo` remains accurate for every firmware build.

## User interface and documentation

Teach the UF2 build script to preserve and display the selected environment
configuration.
Document every supported variable, default, valid route, and a GP12/GP13 I2C
example.
Change pinout documentation and the hardware guide to describe the defaults as
defaults and require test wiring to match the firmware build.
