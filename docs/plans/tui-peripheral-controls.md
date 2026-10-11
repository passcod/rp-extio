# TUI peripheral controls and live state

Extend the Pico 2 pinout TUI from a routing editor into the interactive front
end for every daemon peripheral control. Preserve the physical board diagram,
draft routing workflow, grouped moves, board flipping, and read-only startup.
Separate route editing from live operation so an action cannot accidentally
change a draft route or confuse route enablement with PWM output enablement.

Refresh the connected board's GPIO snapshot while the TUI is idle and annotate
every configured GPIO pin with its live high/low level, input/output direction,
and internal bias. Show full details for the highlighted pin. In operate mode,
allow that pin to be driven low or high, toggled as an LED output, or returned
to input with disabled, pull-up, or pull-down bias. Provide an active-low view
for interpreting the highlighted input as a button and continuously show its
pressed or released state instead of blocking the interface on a wait command.
All mutations continue through daemon control requests and therefore retain
the existing Polkit boundary.

Read an enabled ADC when its signal is selected and show raw counts plus the
reference-scaled voltage in the TUI. Query the selected PWM channel and show
its output state, effective frequency, duty cycle, and polarity. Provide a PWM
editor overlay with typed frequency and duty fields plus enable and inversion
toggles; validate entries before applying the complete state atomically through
the daemon. Refresh telemetry after every operation and periodically while the
TUI is otherwise idle, without polling disabled or unrelated ADC/PWM channels.

Keep clock route, source, and target-frequency editing in route mode and show
the configured values in operate mode. Make mode-specific keys and the current
pin/signal state visible in the on-screen help and status areas, including on
narrow terminals. Update user documentation for live state, route/operate mode,
GPIO/LED/button actions, ADC display, and the PWM editor.
