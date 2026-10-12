#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
identity=${1:?usage: verify-esp32c3-smoke.sh DEVICE_ID}
if ((EUID != 0)); then
	cargo build --manifest-path "$root/Cargo.toml" --release -p rp-extiod
	exec runzero "$root/scripts/verify-esp32c3-smoke.sh" "$identity"
fi

ctl=$root/target/release/rp-extioctl
cleanup() {
	status=$?
	trap - EXIT
	"$ctl" default "$identity" >/dev/null 2>&1 || true
	"$ctl" onboard-led off "$identity" >/dev/null 2>&1 || true
	exit "$status"
}
trap cleanup EXIT

for _ in {1..25}; do
	"$ctl" show "$identity" >/dev/null 2>&1 && break
	sleep 0.2
done
"$ctl" show "$identity" >/dev/null

board=$("$ctl" --json show "$identity")
i2c_bus=$(jq -er '.board.i2c_buses[0]' <<<"$board")
spi_bus=$(jq -er '.board.spi_buses[0]' <<<"$board")
if i2c_output=$(i2ctransfer -y "$i2c_bus" w1@0x55 0x00 2>&1); then
	echo "unexpected I2C acknowledgement from unused address 0x55" >&2
	exit 1
else
	printf 'I2C negative-path transaction: %s\n' "$i2c_output"
fi
python3 - "/dev/spidev$spi_bus.0" "/dev/spidev$spi_bus.1" <<'PY'
import ctypes
import fcntl
import os
import struct
import sys

SPI_IOC_MESSAGE_1 = 0x40206B00
for path in sys.argv[1:]:
    tx = (ctypes.c_ubyte * 4)(0x9f, 0x55, 0xaa, 0x00)
    rx = (ctypes.c_ubyte * 4)()
    transfer = bytearray(struct.pack(
        "=QQIIHBBBBBB",
        ctypes.addressof(tx), ctypes.addressof(rx), len(tx), 500_000,
        0, 8, 0, 0, 0, 0, 0,
    ))
    fd = os.open(path, os.O_RDWR)
    try:
        fcntl.ioctl(fd, SPI_IOC_MESSAGE_1, transfer, True)
    finally:
        os.close(fd)
    print(f"SPI transaction {path}: {bytes(rx).hex()}")
PY
"$ctl" show "$identity" >/dev/null

for rgb in "255 0 0" "0 255 0" "0 0 255"; do
	read -r red green blue <<<"$rgb"
	"$ctl" onboard-led rgb "$identity" "$red" "$green" "$blue"
	sleep 0.4
done
"$ctl" onboard-led off "$identity"

"$ctl" gpio input "$identity" GP18 --pull pull-up
"$ctl" gpio snapshot "$identity"
"$ctl" gpio set "$identity" GP18 low
"$ctl" gpio set "$identity" GP18 high
"$ctl" gpio input "$identity" GP18

"$ctl" set "$identity" --route adc0=GP0 --enable adc0
"$ctl" adc "$identity" 0
"$ctl" default "$identity"

"$ctl" set "$identity" --route pwm0a=GP18 --enable pwm0a
"$ctl" pwm set "$identity" 0 --frequency-hz 1000 --duty-percent 25
"$ctl" pwm get "$identity" 0
"$ctl" default "$identity"

"$ctl" set "$identity" --uart1-tx GP18 --uart1-rx GP19 --enable-uart1
tty_number=$("$ctl" --json show "$identity" | jq -er '.board.uart_ttys[1]')
tty=/dev/ttyRPE$tty_number
test -c "$tty"
stty -F "$tty" raw -echo 115200 cs8 -cstopb -parenb
printf 'rp-extio-uart-smoke\n' >"$tty"
"$ctl" show "$identity"
"$ctl" default "$identity"
test ! -e "$tty"

"$ctl" onboard-led off "$identity"
trap - EXIT
"$ctl" show "$identity"
