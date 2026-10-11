# A/B firmware updates

Add a power-loss-safe firmware update path over the existing rp-extio USB
connection while retaining the standard `picotool` reset interface and BOOTSEL
workflow as the development and recovery mechanism.

Use the RP2350 boot ROM's partition-table, A/B selection, flash-update boot,
explicit-buy, and image-chaining facilities rather than overwriting the running
image in place. Reserve two equal application slots in the Pico 2's 2 MiB flash
and keep the partition table and any boot/update metadata outside both slots.
Define the layout once and consume it from the linker configuration, firmware,
image-packaging tools, and host-side validation so their addresses and sizes
cannot drift. Preserve enough unallocated flash for future persistent device
configuration and release metadata.

Research and document the precise RP2350 boot ROM contract before fixing the
layout: slot address translation, IMAGE_DEF requirements, A/B version choice,
try-before-you-buy state, downgrade behavior, explicit-buy semantics, and the
recovery behavior for invalid partition tables or two invalid images. Keep OTP
security policy out of the first implementation, but choose an on-flash format
that can later be signed without replacing the update protocol.

Produce one position-correct application artifact that the boot ROM can launch
from either slot, using the RP2350 flash translation mechanism where applicable
rather than maintaining divergent A-slot and B-slot firmware builds. Generate
the partition table and release image as reproducible build artifacts alongside
the ELF and UF2. Continue producing a full recovery UF2 that can initialize or
repair the complete layout through BOOTSEL.

Define a versioned update manifest containing the target hardware and firmware
ABI, image format version, semantic/build version, byte length, cryptographic
digest, and any boot ROM image metadata needed to reject an incompatible image
before erasing flash. Initially require digest integrity and leave a clearly
versioned field for an authenticated signature policy. Never accept an image
whose declared extent crosses its inactive slot or reserved flash regions.

Extend the USB protocol with update-status, begin, chunk, finalize, abort, and
reboot operations. Negotiate the feature through capabilities so older firmware
and daemons remain diagnosable. Make every request idempotent or explicitly
resumable: identify an update by manifest digest, report committed offsets, use
bounded aligned chunks, and allow a host reconnect to continue staging the same
image without erasing valid progress. Reject normal bus and pin-map mutation
operations while finalization or a flash-critical section is active, while
keeping status and abort available whenever safe.

Write only the inactive slot. Use Embassy's RAM-resident RP2350 flash routines,
respect erase-sector and program-page alignment, and arrange execution so no
interrupt, USB callback, or second-core activity reads XIP flash during a flash
critical section. Buffer one bounded chunk in RAM, acknowledge it only after
readback agrees, and maintain update progress in erase-safe metadata whose
state transitions remain interpretable after power loss.

On finalize, hash the complete staged image from flash, validate its manifest
and IMAGE_DEF, and ask the boot ROM to select it for a try-before-you-buy boot.
Do not invalidate the current slot. Reboot only after returning a successful
final response to the daemon. During startup, expose the running slot, image
identity, and pending-trial state in HELLO capabilities.

Define the firmware health boundary as successful USB initialization, HELLO
negotiation, restoration of the desired pin map, and a short stable servicing
period. Confirm the trial image with the RP2350 explicit-buy mechanism only
after that boundary. Let watchdog, panic, early reset, invalid image, and an
unconfirmed trial select the previous bought image again. Preserve the existing
panic-to-BOOTSEL behavior when neither application slot is launchable or the
boot/update state cannot be made safe.

Add daemon update orchestration that owns one update session per board, streams
the packaged artifact, resumes from firmware-reported progress, reports phase
and byte progress, expects the planned disconnect/reconnect, and verifies that
the reconnected board is running the requested image. A failed update must
leave ordinary bus teardown behavior intact and must distinguish transport
loss, image rejection, rollback, and recovery-required states.

Expose the workflow as `rp-extioctl update SERIAL IMAGE`, plus machine-readable
status and abort commands suitable for scripts. Require an explicit downgrade
option when the manifest version is older than the running version. Keep update
authorization at the daemon control-socket boundary; do not add a network
listener to the firmware or daemon as part of this work.

Teach `just` to build the packaged A/B artifact and invoke the control client,
while retaining `just flash` unchanged in purpose as the BOOTSEL/picotool path.
Document first-install migration from today's unpartitioned image: it must use a
full BOOTSEL recovery flash because a running legacy image cannot safely create
the new partitioned layout in place. Document release packaging, rollback and
recovery behavior, update compatibility policy, and the boundary between USB
host-mediated updates and genuinely network-delivered OTA systems.
