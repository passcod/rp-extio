# Persisted PWM startup state and clock persistence UX

Extend each serial-number-keyed daemon configuration with optional PWM startup
state. A saved PWM channel records its enabled state, period, duty cycle, and
polarity. Apply saved PWM state whenever the daemon connects to that Pico,
after its pin map is reconfigured, and when configuration is reloaded. Continue
to keep clock enable, route, source, and target frequency in the persisted pin
map, since clocks have no separate runtime state.

Bump the configuration file to version 5 while loading versions 1 through 4
with empty PWM startup state. Reject duplicate or invalid PWM channels, invalid
period/duty combinations, and saved PWM channels that are not enabled in the
board's pin map. Reject a map change that disables a channel with saved PWM
state. Restoring firmware defaults removes GPIO and PWM startup state together
with the custom map.

Add authorized daemon requests to set or clear one persisted PWM state and
include saved PWM startup state in board status. Apply newly persisted state to
a connected board immediately and roll the saved change back if device
application fails. Retain ordinary PWM requests as runtime-only. Expose this
through `--persist` on PWM set and disable commands plus an explicit PWM forget
command.

In TUI operate mode, make the existing save and forget keys work for the
selected enabled PWM channel and show its saved startup state. Preserve the
selected signal and physical-pin cursor after either operation. When a clock
is selected, explain that its enabled state, source, frequency, and route are
already persisted by applying the pin-map draft; do not report it as an
unsupported GPIO. Update the README and hardware verification guide with the
configuration format, CLI/TUI behavior, clock persistence model, reload
behavior, and safe PWM-output guidance.
