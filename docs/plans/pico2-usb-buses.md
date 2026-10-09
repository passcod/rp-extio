# Pico 2 USB bus bridge

Build a Pico 2 firmware image and an out-of-tree Linux module that expose the board's hardware buses through standard Linux interfaces.

## Hardware and USB layout

- Target the Raspberry Pi Pico 2 board, using its USB device connector.
- Use GP4/GP5 for I2C0 SDA/SCL, GP18/GP19/GP16 for SPI0 SCK/MOSI/MISO, GP17 for SPI0 CS0, and GP0/GP1 for UART0 TX/RX.
- Use the Pico SDK and TinyUSB for firmware. Present a vendor-specific bulk interface for I2C and SPI and a CDC ACM interface for UART, so Linux exposes `/dev/ttyACM*` without a custom serial driver.
- Document 3.3 V logic, common ground, I2C pull-ups, power limits, and fixed pin choices.

## Protocol and firmware

- Define a versioned, little-endian, fixed-header request/response protocol with bounded payloads and request IDs.
- Implement I2C combined transfers with repeated starts, 7-bit addresses, and explicit errors; reject unsupported message flags.
- Implement SPI mode, clock, chip select, and full-duplex transfers with explicit limits.
- Bridge CDC ACM traffic to UART0 with bounded buffers and line coding support.
- Include build and UF2 flashing instructions.

## Linux integration

- Add a USB driver that binds only the vendor interface and registers one `i2c_adapter` and one `spi_controller`.
- Route I2C transfers and SPI messages over USB with locking, timeouts, disconnect handling, and protocol validation.
- Add a DKMS manifest, module Makefile, udev guidance, and normal Linux usage examples.
- Keep the module in C unless the installed kernel Rust bindings support USB, I2C, and SPI registration sufficiently for an out-of-tree Rust module; explain that decision in the README.
