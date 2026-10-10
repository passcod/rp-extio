#!/bin/sh
set -eu

cargo firmware
mkdir -p build
cp target/thumbv8m.main-none-eabihf/release/rp-extio-firmware \
  build/rp-extio-firmware.elf
picotool uf2 convert \
  build/rp-extio-firmware.elf \
  build/rp-extio-firmware.uf2 \
  --family rp2350-arm-s
