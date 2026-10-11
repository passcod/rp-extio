# Persisted button polarity

Persist button interpretation per board and GPIO pin in the daemon configuration.
Expose the saved button records in read-only board information and add authorized
control requests for setting and clearing them.

In the TUI, replace the process-wide polarity toggle with a per-pin cycle through
unconfigured, active-low, and active-high button roles. Render the saved role and
use it when displaying whether the selected input is pressed.

In `rp-extioctl button`, use an explicit `--active-low` or `--active-high`
override when supplied, otherwise read the pin's saved polarity from the daemon.
If the pin has no saved button role, report that clearly instead of silently
choosing a polarity.

Document the shared configuration and CLI override behaviour, including migration
of existing configuration files to the new schema.
