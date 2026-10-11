#!/usr/bin/env bash
set -euo pipefail

if (($# != 1)); then
  printf 'Usage: %s I2C_BUS\n' "$0" >&2
  printf 'Example: %s 28\n' "$0" >&2
  exit 2
fi

bus=$1
if [[ ! $bus =~ ^[0-9]+$ ]]; then
  printf 'I2C_BUS must be a bus number, got %q\n' "$bus" >&2
  exit 2
fi

for command in i2ctransfer awk; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'Required command not found: %s\n' "$command" >&2
    exit 1
  fi
done

device=/dev/i2c-$bus
if [[ ! -c $device ]]; then
  printf 'I2C adapter does not exist: %s\n' "$device" >&2
  exit 1
fi

readonly address=0x62
measuring=false

stop_measurement() {
  if $measuring; then
    i2ctransfer -y "$bus" w2@"$address" 0x3f 0x86 >/dev/null || true
    measuring=false
  fi
}
trap stop_measurement EXIT INT TERM

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

stop_measurement
sleep 0.5
trap - EXIT INT TERM

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
