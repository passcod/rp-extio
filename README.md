# Pico 2 USB buses for Linux

This project turns a Raspberry Pi Pico 2 into a USB adapter for Linux I2C,
SPI, and UART. The firmware uses the Pico 2 hardware controllers. The Linux
module registers an I2C adapter and an SPI controller, with two `spidev`
devices. UART appears through the kernel's existing `cdc_acm` driver.

The USB IDs `cafe:2350` are **development IDs only**. Replace both definitions
in `firmware/usb_descriptors.c` and `linux/rp_extio.c` with an allocated VID/PID
before distributing hardware or firmware.

## Wiring

| Bus | Pico 2 pins | Linux interface |
| --- | --- | --- |
| I2C0 | GP4 SDA, GP5 SCL | `/dev/i2c-N` |
| SPI0 | GP18 SCK, GP19 MOSI, GP16 MISO | `/dev/spidevB.0`, `/dev/spidevB.1` |
| SPI CS0 / CS1 | GP17 / GP21, active low | chip selects 0 / 1 |
| UART0 | GP0 TX, GP1 RX | `/dev/ttyACM*` |

Connect grounds. GPIO signals are **3.3 V only**; do not connect 5 V signals.
Provide pull-ups from SDA and SCL to 3.3 V appropriate to the wiring and
target devices. The Pico 2's weak internal pull-ups are enabled but are not a
substitute for bus pull-ups. Do not draw target-board power from a GPIO pin.
This adapter is a bus master and cannot act as an I2C or SPI target. UART has
no hardware flow control in this firmware.

## Build and flash firmware

Install the Raspberry Pi Pico SDK, a CMake toolchain with `arm-none-eabi-gcc`,
and set `PICO_SDK_PATH` to the SDK directory. Then:

```sh
cmake -S firmware -B build/firmware -DPICO_BOARD=pico2 -DCMAKE_BUILD_TYPE=Release
cmake --build build/firmware
```

Hold BOOTSEL while connecting the Pico 2, then copy
`build/firmware/rp_extio.uf2` to the `RP2350` mass-storage volume. Reconnect
the board normally. If your board is not a Pico 2, check its USB connector,
pin mapping, and `PICO_BOARD` definition before flashing.

## Build and install the Linux module

The module uses the USB, I2C, and SPI kernel APIs. Build it against the
headers for the running kernel:

```sh
make -C linux
sudo modprobe spidev
sudo insmod linux/rp_extio.ko
```

For DKMS, copy the four files in `linux/` into a versioned source directory
and register that version:

```sh
sudo install -d /usr/src/rp-extio-0.1.0
sudo install -m 0644 linux/Makefile linux/dkms.conf linux/rp_extio.c linux/protocol.h /usr/src/rp-extio-0.1.0/
sudo dkms add -m rp-extio -v 0.1.0
sudo dkms build -m rp-extio -v 0.1.0
sudo dkms install -m rp-extio -v 0.1.0
sudo modprobe spidev
sudo modprobe rp_extio
```

Load `spidev` before connecting the board so its two SPI devices bind. The
USB module autoloads on systems with normal module alias handling. Kernel
module signing or Secure Boot policy may require local signing. Check
`dmesg` for the allocated I2C and SPI bus numbers; they are dynamic. A CDC
ACM interface can also match another board, so use `/dev/serial/by-id/` or
the USB parent path to identify the right UART.

Example usage after reading the bus numbers from `dmesg`:

```sh
i2cdetect -y N
spidev_test -D /dev/spidevB.0 -s 1000000
stty -F /dev/ttyACM0 115200 raw -echo
```

I2C supports 7-bit addresses, combined transfers, and repeated starts; the
firmware runs the bus at 400 kHz. I2C messages with flags besides `I2C_M_RD`
are rejected. SPI supports modes 0–3, 8 bits per word, 50 kHz–20 MHz, two
chip selects, full-duplex data, and `cs_change` between transfers. SPI word,
transfer, and chip-select delays are rejected. Requests and responses have a
2048-byte payload limit; an I2C transfer can contain up to 16 messages and
an SPI message up to 32 transfers. These limits include protocol metadata.
USB transport errors require unplugging and reconnecting the board to reset
the byte stream. Physical Pico 2 and target-device operation remains untested
in this environment.

## Protocol

The vendor interface is USB interface 2, with bulk OUT endpoint `0x03` and
bulk IN endpoint `0x83`. All integers are little endian. Every exchange has
one 16-byte header followed by the specified payload; replies carry the same
sequence and command.

| Offset | Size | Field |
| --- | ---: | --- |
| 0 | 4 | Magic `RPE2` (`0x32455052`) |
| 4 | 1 | Protocol version `1` |
| 5 | 1 | Command: `1` I2C, `2` SPI |
| 6 | 1 | Status: `0` success, `1` bad request, `2` I/O error, `3` unsupported |
| 7 | 1 | Reserved, zero |
| 8 | 4 | Sequence number |
| 12 | 2 | Payload length |
| 14 | 2 | Reserved, zero |

I2C request payload: one-byte message count, then for each message a 16-bit
address, 16-bit flags (`1` means read), 16-bit length, and the write bytes if
writing. The response concatenates read bytes. SPI request payload: one-byte
chip select, one-byte mode, one-byte segment count, one reserved byte; then
each segment has a 32-bit speed in Hz, 16-bit length, one-byte `cs_change`,
one reserved byte, and exactly that many TX bytes. The SPI response
concatenates all RX bytes.

## Rust status

The current firmware and module are C. The installed kernel has Rust enabled,
but its Rust API does not expose the USB driver, I2C adapter, and SPI
controller registration interfaces needed here. An out-of-tree Rust module
would need substantial C bindings around those APIs and would tie this bridge
to a particular kernel version. The Pico SDK and TinyUSB provide the complete
USB and peripheral path used by this implementation. The wire protocol is
independent of either language, so a later Rust firmware can use the same
host driver.
