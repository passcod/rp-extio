#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
identity=${1:?usage: verify-esp32c3-led.sh DEVICE_ID}
if ((EUID != 0)); then
	cargo build --manifest-path "$root/Cargo.toml" --release -p rp-extiod
	if [[ ${2:-} == --display-only ]]; then
		exec runzero "$root/scripts/verify-esp32c3-led.sh" "$identity" --display-only
	fi
	cargo firmware-esp32c3
	shopt -s nullglob
	ports=()
	for candidate in /dev/serial/by-id/*; do
		properties=$(udevadm info --query=property --name "$candidate" 2>/dev/null || true)
		[[ $'\n'$properties$'\n' == *$'\nID_VENDOR_ID=10c4\n'* ]] || continue
		[[ $'\n'$properties$'\n' == *$'\nID_MODEL_ID=ea60\n'* ]] || continue
		ports+=("$candidate")
	done
	((${#ports[@]} == 1)) || { echo "expected exactly one supported ESP32-C3 bridge" >&2; exit 2; }
	exec runzero "$root/scripts/verify-esp32c3-led.sh" "$identity" "${ports[0]}" "$(command -v espflash)"
fi

ctl=$root/target/release/rp-extioctl
daemon_was_active=false
display_only=false
cleanup() {
	status=$?
	trap - EXIT
	$display_only || systemctl is-active --quiet rp-extiod.service || systemctl start rp-extiod.service >/dev/null 2>&1 || true
	"$ctl" onboard-led off "$identity" >/dev/null 2>&1 || true
	$display_only || $daemon_was_active || systemctl stop rp-extiod.service >/dev/null 2>&1 || true
	exit "$status"
}
trap cleanup EXIT
if [[ ${2:-} == --display-only ]]; then
	display_only=true
else
	port=${2:?missing prepared serial port}
	espflash=${3:?missing espflash path}
	if systemctl is-active --quiet rp-extiod.service; then
		daemon_was_active=true
		systemctl stop rp-extiod.service
	fi
	"$espflash" flash --chip esp32c3 --port "$port" --non-interactive \
		"$root/target/riscv32imc-unknown-none-elf/release/rp-extio-firmware-esp32c3"
	systemctl start rp-extiod.service
fi
for _ in {1..25}; do
	"$ctl" show "$identity" >/dev/null 2>&1 && break
	sleep 0.2
done
for sample in 'red 255 0 0' 'green 0 255 0' 'blue 0 0 255' 'purple 128 0 128'; do
	read -r name red green blue <<<"$sample"
	printf '%s: %s %s %s\n' "$name" "$red" "$green" "$blue"
	"$ctl" onboard-led rgb "$identity" "$red" "$green" "$blue"
	sleep 2
done
echo 'off'
"$ctl" onboard-led off "$identity"
sleep 2
