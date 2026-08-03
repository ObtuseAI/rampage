# Easy and secure pairing

Rampage is designed so a person can add a Windows laptop without a terminal, account, IP address,
or copied invitation.

## The normal path

1. Install Rampage on both machines and keep them on the same private Wi-Fi or Ethernet network.
2. On the laptop, choose **Join my fabric**, then **Find my fabric**. Leave it waiting.
3. On the main PC, open Rampage and choose **Add machine**.
4. Rampage shows the nearby laptop and the same four digits on both screens.
5. Check the digits, then choose **Codes match—approve** on the main PC.
6. The main PC reports **Connected securely**. The laptop stores its enrollment durably and restarts
   automatically into a contributor status screen. **Worker active** appears only after the owner
   PC accepts the laptop's first signed resource offer.

The first pairing action may trigger one standard Windows administrator prompt. Rampage uses it to
install executable-scoped UDP allowances for its pairing, controller, and worker binaries on
**private networks only**. Public-network access is not enabled.

## What the four digits mean

The code is a short authentication string derived independently by both machines from an ephemeral
X25519 key exchange and the exact pairing transcript. It lets the owner detect a different or
intercepted device. It is deliberately not a password and cannot recreate the real invitation.

After approval, the owner creates the normal short-lived, Governor-signed Rampage invite, encrypts
it with AES-256-GCM under a key derived with HKDF-SHA-256, and sends the ciphertext directly to the
laptop. The laptop authenticates and persists it, sends an encrypted completion receipt, and
restarts. After the controller consumes the one-time secret, the laptop replaces the invitation
with a Governor-signed controller pin and removes the enrollment secret. The pin remains valid as a
transport identity anchor after the ten-minute discovery record expires; it grants no job or lease
authority. The long invitation never appears in the normal UI and is never sent in discovery
broadcasts.

The controller also persists its authenticated UDP port. An upgrade migrates the newest proven
legacy port from the evidence ledger before selecting the fixed port used by a new installation, so
an enrolled laptop does not lose its signed route merely because the main app restarts.

## Why discovery remains narrow

- The owner listens only after **Add machine** and the window closes after three minutes.
- The laptop request expires after five minutes and contains only bounded metadata plus an
  ephemeral public key.
- Rampage accepts at most five new requests per source address per minute and sixteen pending
  requests total.
- Datagram size, labels, clock skew, schemas, and unknown fields are validated before state is
  created.
- Repeated requests reuse the same challenge; repeated approvals resend the same encrypted payload
  so ordinary packet loss does not force the user to start over.
- The controller and intelligence APIs remain token-protected and bound to loopback.

## If the machines do not appear

Check these in order:

1. Both machines are on the same private LAN, not a guest Wi-Fi network with client isolation.
2. **Find my fabric** is still waiting on the laptop.
3. **Add machine** is open on the main PC.
4. Windows Defender Firewall allows Rampage on private networks.
5. A VPN or corporate endpoint policy is not blocking local broadcast and multicast.

Retrying creates fresh ephemeral keys and a fresh code. For intentionally segmented networks, open
**Advanced: use a complete invite** on the joining machine and transfer the complete signed invite
through a trusted channel. That fallback is less convenient but retains the existing cryptographic
enrollment checks.
