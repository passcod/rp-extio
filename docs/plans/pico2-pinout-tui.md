# Pico 2 pinout TUI

## Goal

Replace the signal-list editor with a physical Raspberry Pi Pico 2 board view.
The terminal UI must show the two 20-pin headers in their real orientation,
overlay the draft peripheral routes on the GPIO pins, and let an operator place
each runtime-routable signal directly on the board.

The daemon API and complete-map model remain unchanged.
All nine UART, I2C, and SPI signals stay assigned; the TUI edits where they are
routed and applies the complete draft atomically.

## Board model and presentation

Define a TUI-local physical pin model for header pins 1 through 40, including
GPIO number where applicable and the board labels for ground, power, RUN,
ADC reference, and enable pins.
Render the USB connector at the top and SWD at the bottom so pin 1 is visibly at
the upper left, pin 40 is at the upper right, pin 20 is at the lower left, and
pin 21 is at the lower right.

Each GPIO cell shows its physical pin number, GP number, and current draft role.
Assigned UART, I2C, and SPI signals use distinct peripheral colours; unassigned
header GPIOs are labelled as exported GPIO lines.
Power, ground, control, and reference pins remain visible but cannot be selected
as routes.

Highlight every valid candidate GPIO for the selected signal, distinguish its
currently assigned pin, show draft changes relative to the active map, and make
duplicate assignments and mismatched I2C pairs visible in the board and status
area.

## Interaction model

Use the arrow keys to move the pin cursor by physical row and header side.
Use Tab and Shift-Tab to cycle the selected peripheral signal, moving the cursor
to that signal's draft assignment.
Use Enter or Space to assign the selected signal to the highlighted pin.
Reject non-GPIO and invalid-route selections locally with a concise status
message while allowing conflict-producing valid assignments to remain in the
draft for correction.

Keep `[` and `]` for board selection, `a` for atomic apply, `r` for refresh,
`l` for persistent-config reload, and `q`, Escape, and Ctrl-C for exit.
Change `d` to load the firmware default into the draft and add `u` to discard
the draft and restore the board's active map, so every routing edit follows the
same preview-then-apply flow.

Show the selected board, connection state, generation, Linux bus assignments,
selected signal, draft route, validation state, and dirty state without
requiring a separate signal table.

## Responsive layout and documentation

Use the full pinout plus a compact signal legend on wide terminals.
On narrower terminals, retain the board diagram and fold signal and device
details into compact header and footer lines.
On short terminals, scroll physical rows around the cursor while preserving
header-side orientation and indicate the visible row range.
Display a clear minimum-size message only when even the compact board cannot be
rendered safely.

Update the README controls and workflow to describe physical-pin navigation,
draft assignments, defaults, undo, and apply semantics.
