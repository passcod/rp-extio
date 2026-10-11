# Optional second SPI chip select

Allow each SPI controller's second chip select to be disabled independently of
the controller so a one-device SPI bus reserves only CS0 and returns the CS1 pin
to GPIO service.

Encode the two CS1 enable states in the currently unused upper bits of
`PinMap::enabled`, retaining the existing pin-map length and protocol version.
Add wire helpers for signal-level enable state and per-controller chip-select
count, and use them for conflict validation and GPIO derivation. Keep CS0
mandatory whenever its SPI controller is enabled so Linux chip-select numbering
remains contiguous and the kernel ABI does not change.

Report one or two chip selects in each firmware SPI capability and include only
the corresponding CS pin bytes. Reject transfers to disabled CS1 and configure,
release, and neutralize only enabled chip-select pins. Add build-time default
switches for both CS1 routes while preserving the current two-chip-select
default.

Expose `spi0_cs1_enabled` and `spi1_cs1_enabled` in persisted and control pin
maps. Treat the fields as enabled when reading existing version-2 configuration
files, preserve enabled CS1 for legacy version-1 migration, and include the
states in map formatting and validation errors. Add CLI enable/disable flags for
both optional chip selects.

In the TUI, show CS1 enabled state separately from its parent SPI controller.
Make `e` toggle the selected CS1 route while continuing to toggle the whole SPI
controller for its other signals, and immediately preview a disabled CS1 pin as
GPIO. Keep CS0 tied to the parent controller.

Update user documentation, examples, and the hardware verification procedure
to cover a one-chip-select SPI controller and returning CS1 to GPIO.
