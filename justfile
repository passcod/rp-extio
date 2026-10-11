module_name := "rp-extio"
module_version := "0.1.0"
kernel_release := `uname -r`
module_source := "/usr/src/" + module_name + "-" + module_version
root := justfile_directory()

default:
    @just --list

# Build the host tools and kernel module.
build: release module

# Build optimized daemon and control binaries.
release:
    cargo build --release -p rp-extiod

# Build the module for the running kernel.
module:
    make -C linux

# Install the module, binaries, and systemd unit, but do not enable the service.
install: install-module install-userspace
    systemctl daemon-reload
    @echo "installed; run 'just enable' to enable and start rp-extiod.service"

# Register the module source with DKMS and rebuild it for the running kernel.
install-module:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v dkms >/dev/null || { echo "dkms is required" >&2; exit 1; }
    runzero install -d {{ module_source }}
    runzero install -m 0644 {{ root }}/linux/Makefile {{ root }}/linux/dkms.conf {{ root }}/linux/rp_extio.c {{ root }}/linux/rp_extio_uapi.h {{ module_source }}/
    current=$(dkms status -m {{ module_name }} -v {{ module_version }} -k {{ kernel_release }} 2>/dev/null || true)
    if [[ -n $current ]]; then
        runzero dkms remove -m {{ module_name }} -v {{ module_version }} -k {{ kernel_release }}
    fi
    registered=$(dkms status -m {{ module_name }} -v {{ module_version }} 2>/dev/null || true)
    if [[ -z $registered ]]; then
        runzero dkms add -m {{ module_name }} -v {{ module_version }}
    fi
    runzero dkms build -m {{ module_name }} -v {{ module_version }} -k {{ kernel_release }}
    runzero dkms install -m {{ module_name }} -v {{ module_version }} -k {{ kernel_release }}

# Install the release binaries and systemd unit.
install-userspace: release
    runzero install -m 0755 {{ root }}/target/release/rp-extiod /usr/local/bin/rp-extiod
    runzero install -m 0755 {{ root }}/target/release/rp-extioctl /usr/local/bin/rp-extioctl
    runzero install -m 0644 {{ root }}/dist/rp-extiod.service /etc/systemd/system/rp-extiod.service

# Enable and start the installed daemon service.
enable: install
    systemctl reset-failed rp-extiod.service
    systemctl enable --now rp-extiod.service

# Stop the daemon, reload the installed module without force, and start it again.
restart: install
    #!/usr/bin/env bash
    set -euo pipefail
    systemctl stop rp-extiod.service
    if [[ -d /sys/module/rp_extio ]]; then
        runzero rmmod rp_extio
    fi
    systemctl reset-failed rp-extiod.service
    systemctl start rp-extiod.service
