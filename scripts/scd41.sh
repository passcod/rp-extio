#!/usr/bin/env bash
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/.." && pwd)
if ((EUID != 0)); then
  cargo build -p rp-extiod
  make -C "$root/linux" >/dev/null
  exec runzero "$root/scripts/scd41.sh" --root
fi
[[ ${1:-} == --root ]] || exit 2

grep -q '^rp_extio ' /proc/modules || insmod "$root/linux/rp_extio.ko"
[[ -c /dev/rp-extio-control ]] || udevadm trigger --action=add --settle /sys/class/misc/rp-extio-control
latest_bus() { i2cdetect -l | awk '/rp-extio virtual I2C/{sub(/^i2c-/, "", $1); bus=$1} END{print bus}'; }
old_bus=$(latest_bus)
daemon_pid= bus=
cleanup() {
  status=$?; trap - EXIT INT TERM; set +e
  [[ -n $bus ]] && i2ctransfer -y "$bus" w2@0x62 0x3f 0x86 >/dev/null
  [[ -n $daemon_pid ]] && kill "$daemon_pid" 2>/dev/null
  [[ -n $daemon_pid ]] && wait "$daemon_pid" 2>/dev/null
  exit "$status"
}
trap cleanup EXIT INT TERM

"$root/target/debug/rp-extiod" --control /dev/rp-extio-control & daemon_pid=$!
for _ in {1..50}; do
  kill -0 "$daemon_pid" 2>/dev/null || wait "$daemon_pid"
  bus=$(latest_bus)
  [[ -n $bus && $bus != "$old_bus" ]] && break
  sleep 0.1
done
[[ -n $bus && $bus != "$old_bus" ]] || exit 1

i2ctransfer -y "$bus" w2@0x62 0x21 0xb1
sleep 5
i2ctransfer -y "$bus" w2@0x62 0xec 0x05
read -r -a data <<<"$(i2ctransfer -y "$bus" r9@0x62)"
i2ctransfer -y "$bus" w2@0x62 0x3f 0x86
bus=

awk -v c="$((${data[0]} << 8 | ${data[1]}))" -v t="$((${data[3]} << 8 | ${data[4]}))" -v h="$((${data[6]} << 8 | ${data[7]}))" 'BEGIN { printf "CO2: %d ppm\nTemperature: %.2f C\nRelative humidity: %.2f%%\n", c, -45 + 175*t/65535, 100*h/65535 }'
