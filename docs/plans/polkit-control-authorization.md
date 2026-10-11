# Polkit control authorization

Make board discovery and pin-map inspection available to unprivileged local
processes while retaining authorization for every operation that changes daemon,
firmware, or persisted state.

Change the daemon control socket to mode `0666` and enforce access per request
instead of treating permission to connect as permission to mutate. Classify
`list` and `show` as read-only. Classify `set`, `default`, and `reload` as
mutating, including requests that would ultimately make no state change, so a
caller cannot use request contents to bypass the authorization boundary.

Capture each Unix peer's kernel-supplied PID, UID, and GID when accepting the
connection. Resolve the process start time from `/proc` and pass the complete
`pid,start-time,uid` subject to Polkit, avoiding the PID-reuse race documented
for incomplete process subjects. Keep authorization attached to the connection
that supplied those credentials; never accept a caller-provided identity.

Authorize mutating requests without a prompt for root and for members of the
configured `--operator-group`, including supplementary group membership.
Otherwise ask Polkit for `org.rp-extio.configure` with user interaction enabled.
Translate denial, dismissal, missing authentication agents, malformed peer
identity, and Polkit execution failure into distinct control errors without
forwarding the mutation to the device manager.

Install a Polkit action definition that denies arbitrary sessions by default
and permits an active local user to authenticate as an administrator, retaining
the resulting temporary authorization according to normal Polkit policy. Keep
the authorization decision in the root daemon; the CLI and TUI must not invoke
`sudo`, `run0`, `runzero`, or a privileged helper for control operations.

Allow a normal user to run `rp-extioctl list`, `show`, and `tui`. Let the TUI
load and refresh board state without prompting, and let its existing apply,
default, and reload actions trigger the Polkit prompt through the daemon only
when invoked. Keep CLI output and JSON responses useful for noninteractive
callers when authorization is unavailable or denied.

Update installation to deploy the policy file with the daemon and reload the
relevant system configuration. Document the public read-only surface, the
Polkit-protected mutation surface, operator-group bypass behavior, and the need
for an authentication agent in interactive sessions.
