# Rampage 0.3.0 — Remote Assist

Rampage 0.3.0 turns a paired Windows worker into a machine the owner can both use and help—without
installing a separate remote-access account or expanding the AI system's authority.

## What is new

- Open an opted-in paired Windows worker in the Rampage owner app with **View desktop** or
  **Control desktop**.
- Route bounded JPEG frames and ordered mouse, keyboard, and wheel input over a dedicated
  authenticated QUIC protocol.
- Issue worker-, controller-, session-, mode-, epoch-, size-, frame-rate-, and expiry-bound leases.
- Renew authority only while the native viewer remains active; every lease expires within 30 seconds.
- Advertise the capability only while the worker's durable local opt-in is enabled.
- Show a prominent worker indicator and tray warning while a session is active.
- Revoke access by closing the viewer, turning off the worker toggle, or pressing local STOP.
- Persist nonce and epoch replay fences so a restarted agent still rejects consumed authority.
- Keep one active Remote Assist session per worker so the visible indicator has one clear owner.

## Security and operating-system boundary

Remote Assist is not unattended administration. It grants no shell, service-control, file-transfer,
elevation, or policy-changing authority. It is accepted only from the controller identity pinned during
pairing, and only after the worker has enabled the exact capability locally.

Windows remains the final authority. Capture and input stop at the lock screen and secure desktop;
input cannot cross User Interface Privilege Isolation into a higher-integrity application. UAC prompts
must be handled locally. Non-Windows workers do not advertise this capability.

## Honest release boundary

Source tests, native packaging, artifact hashes, and physical-device results are tracked in
[RELEASE_EVIDENCE_0.3.0.md](RELEASE_EVIDENCE_0.3.0.md). The showcase screenshot is not a substitute
for a physical two-machine control receipt. Published Windows installers remain unsigned until real
Authenticode credentials are configured and independently verified.

## Upgrade

Install 0.3.0 on the owner and worker. Existing paired identities are preserved by the normal upgrade
path. On the worker, open Rampage and turn on **Allow owner remote control**. On the owner, select the
worker and choose **View desktop** or **Control desktop**.
