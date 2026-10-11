# Persisted GPIO startup state

Extend each serial-number-keyed daemon configuration with optional startup
state for GPIO pins. A startup entry is either an output level or an input with
disabled, pull-up, or pull-down bias. Keep this state host-side, like the pin
map, and apply it whenever the daemon connects to that Pico or recreates its
kernel devices after a map change. Document that pins remain inputs during the
short interval before the daemon connects and applies the saved state.

Bump the configuration file to version 4 while loading versions 1 through 3
with empty startup state. Store startup entries inside their board object,
reject duplicate or unavailable pins, and reject a route change that would
claim a GPIO with persisted startup state. Restoring firmware defaults removes
both the custom map and startup states. Reloading the file applies both mapping
and startup changes to connected boards.

Add authorized daemon requests to set or clear one persisted GPIO state and
include saved startup state in board status. Apply a newly persisted state to a
connected board immediately, roll the persisted change back if device
application fails, and retain ordinary GPIO requests as runtime-only. Expose
this through CLI flags on GPIO output/input commands plus an explicit command
to forget a pin's startup state.

In TUI operate mode, show whether the highlighted GPIO has saved startup state.
Provide a key that saves its current direction, bias, and level, and another
that removes the saved state. Refresh board status after either operation while
preserving the selected signal and physical-pin cursor. Update the README and
hardware verification guide with configuration format, CLI/TUI usage, startup
ordering, pin-map conflicts, reload behavior, and safe output-level guidance.
