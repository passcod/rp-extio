#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "$script_dir/.." && pwd)
daemon=$repo_root/target/debug/rp-extiod

usage() {
  printf 'Usage: %s\n' "$0" >&2
}

if (($# != 0)); then
  if (($# != 1)) || [[ $1 != --privileged ]]; then
    usage
    exit 2
  fi
fi

if ((EUID != 0)); then
  if (($# != 0)); then
    printf '%s is an internal option\n' "$1" >&2
    exit 2
  fi
  for command in cargo runzero; do
    if ! command -v "$command" >/dev/null 2>&1; then
      printf 'Required command not found: %s\n' "$command" >&2
      exit 1
    fi
  done

  printf 'Building rp-extiod...\n'
  cargo build -p rp-extiod
  exec runzero "$script_dir/read-scd41.sh" --privileged
fi

for command in awk i2ctransfer modprobe; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'Required command not found: %s\n' "$command" >&2
    exit 1
  fi
done

if [[ ! -x $daemon ]]; then
  printf 'Daemon binary does not exist: %s\n' "$daemon" >&2
  exit 1
fi

if [[ ! -c /dev/rp-extio-control ]]; then
  printf 'Loading rp_extio kernel module...\n'
  if ! modprobe rp_extio; then
    module=$repo_root/linux/rp_extio.ko
    if [[ ! -f $module ]]; then
      printf 'modprobe failed and the locally built module does not exist: %s\n' "$module" >&2
      exit 1
    fi
    insmod "$module"
  fi

  if command -v udevadm >/dev/null 2>&1 && [[ -e /sys/class/misc/rp-extio-control ]]; then
    udevadm trigger --action=add --settle /sys/class/misc/rp-extio-control
  fi
fi

if [[ ! -c /dev/rp-extio-control ]]; then
  printf 'Kernel module loaded without creating /dev/rp-extio-control\n' >&2
  exit 1
fi

rp_extio_buses() {
  local path name
  shopt -s nullglob
  for path in /sys/bus/i2c/devices/i2c-*; do
    read -r name <"$path/name" || continue
    if [[ $name == 'rp-extio virtual I2C' ]]; then
      printf '%s\n' "${path##*-}"
    fi
  done
  shopt -u nullglob
}

declare -A existing_buses=()
while read -r existing; do
  existing_buses[$existing]=1
done < <(rp_extio_buses)

readonly address=0x62
bus=
daemon_pid=
daemon_log=$(mktemp /tmp/rp-extiod-scd41.XXXXXX)
measuring=false

cleanup() {
  local status=$?
  trap - EXIT INT TERM

  if $measuring && [[ -n $bus ]]; then
    i2ctransfer -y "$bus" w2@"$address" 0x3f 0x86 >/dev/null || true
    measuring=false
    sleep 0.5
  fi

  if [[ -n $daemon_pid ]] && kill -0 "$daemon_pid" 2>/dev/null; then
    printf 'Stopping rp-extiod...\n'
    kill -TERM "$daemon_pid" 2>/dev/null || true
    for _ in {1..20}; do
      if ! kill -0 "$daemon_pid" 2>/dev/null; then
        break
      fi
      sleep 0.1
    done
    if kill -0 "$daemon_pid" 2>/dev/null; then
      printf 'rp-extiod did not stop; process %s remains and log is %s\n' \
        "$daemon_pid" "$daemon_log" >&2
      exit 1
    fi
    wait "$daemon_pid" 2>/dev/null || true
  fi

  rm -f -- "$daemon_log"
  exit "$status"
}
trap cleanup EXIT INT TERM

printf 'Starting rp-extiod...\n'
"$daemon" --control /dev/rp-extio-control >"$daemon_log" 2>&1 &
daemon_pid=$!

for _ in {1..50}; do
  if ! kill -0 "$daemon_pid" 2>/dev/null; then
    printf 'rp-extiod exited during startup:\n' >&2
    sed 's/^/  /' "$daemon_log" >&2
    exit 1
  fi

  new_buses=()
  while read -r candidate; do
    if [[ ! -v existing_buses[$candidate] ]]; then
      new_buses+=("$candidate")
    fi
  done < <(rp_extio_buses)

  if ((${#new_buses[@]} == 1)) && [[ -c /dev/i2c-${new_buses[0]} ]]; then
    bus=${new_buses[0]}
    break
  fi
  if ((${#new_buses[@]} > 1)); then
    printf 'rp-extiod registered multiple new I2C adapters; connect one demo board only\n' >&2
    exit 1
  fi
  sleep 0.1
done

if [[ -z $bus ]]; then
  printf 'Timed out waiting for rp-extiod to register an I2C adapter\n' >&2
  if [[ -s $daemon_log ]]; then
    sed 's/^/  /' "$daemon_log" >&2
  fi
  exit 1
fi

crc8() {
  local crc=255
  local byte bit

  for byte in "$@"; do
    crc=$((crc ^ byte))
    for ((bit = 0; bit < 8; bit++)); do
      if ((crc & 0x80)); then
        crc=$((((crc << 1) ^ 0x31) & 0xff))
      else
        crc=$(((crc << 1) & 0xff))
      fi
    done
  done

  printf '%d\n' "$crc"
}

printf 'Starting SCD41 periodic measurement on i2c-%s address 0x62...\n' "$bus"
i2ctransfer -y "$bus" w2@"$address" 0x21 0xb1
measuring=true

printf 'Waiting for the first sample...\n'
sleep 5

i2ctransfer -y "$bus" w2@"$address" 0xec 0x05
read -r -a response <<<"$(i2ctransfer -y "$bus" r9@"$address")"

if ((${#response[@]} != 9)); then
  printf 'Expected 9 response bytes, received %d\n' "${#response[@]}" >&2
  exit 1
fi

for offset in 0 3 6; do
  high=$((${response[offset]}))
  low=$((${response[offset + 1]}))
  received_crc=$((${response[offset + 2]}))
  calculated_crc=$(crc8 "$high" "$low")
  if ((received_crc != calculated_crc)); then
    printf 'CRC mismatch for response word %d: received 0x%02x, expected 0x%02x\n' \
      "$((offset / 3 + 1))" "$received_crc" "$calculated_crc" >&2
    exit 1
  fi
done

co2=$((${response[0]} << 8 | ${response[1]}))
temperature_raw=$((${response[3]} << 8 | ${response[4]}))
humidity_raw=$((${response[6]} << 8 | ${response[7]}))

i2ctransfer -y "$bus" w2@"$address" 0x3f 0x86
measuring=false
sleep 0.5

awk \
  -v co2="$co2" \
  -v temperature_raw="$temperature_raw" \
  -v humidity_raw="$humidity_raw" \
  'BEGIN {
    temperature = -45 + 175 * temperature_raw / 65535
    humidity = 100 * humidity_raw / 65535
    printf "CO2: %d ppm\nTemperature: %.2f C\nRelative humidity: %.2f%%\n", \
      co2, temperature, humidity
  }'
