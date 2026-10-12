#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
identity=${1:?usage: verify-esp32c3-smoke.sh DEVICE_ID}
if ((EUID != 0)); then
	cargo build --manifest-path "$root/Cargo.toml" --release -p rp-extiod
	exec runzero "$root/scripts/verify-esp32c3-smoke.sh" "$identity"
fi

ctl=$root/target/release/rp-extioctl
install -m 0755 "$root/target/release/rp-extiod" "$ctl" /usr/local/bin/
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
