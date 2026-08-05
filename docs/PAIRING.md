# Easy and secure pairing

Rampage is designed so a person can add a Windows laptop without a terminal, account, IP address,
copied invitation, or verification code.

## The normal path

1. Install Rampage on both machines and keep them on the same trusted private Wi-Fi or Ethernet
   network.
2. Create the fabric on the main PC. Rampage keeps its nearby-device listener available
   automatically while the app is running.
3. On the laptop, choose **Join my fabric**, then **Find my fabric**. Leave it waiting.
4. The main PC automatically shows a **New machine found** card with the laptop name. If Rampage is
   hidden in the system tray, it restores the window and labels the tray with the waiting machine;
   retransmitted discovery packets do not repeatedly interrupt the owner, and the alert clears
   after approval, rejection, or expiry.
5. Choose **Approve this machine** on the main PC.
6. The main PC reports **Connected securely**. The laptop stores its enrollment durably and restarts
   automatically into a contributor status screen. **Worker active** appears only after the owner
   accepts the laptop's first signed resource offer.

The first network action may trigger one standard Windows administrator prompt. Rampage uses it to
install executable-scoped UDP allowances for its pairing, controller, and worker binaries on
**private networks only**. Public-network access is not enabled. A valid existing firewall marker is
preserved when an unconfirmed legacy bootstrap is converted into a joining worker.

## What happens underneath

The laptop generates an ephemeral X25519 key and repeatedly announces only bounded request metadata
on the private LAN. The owner derives a one-time AES-256-GCM channel with HKDF-SHA-256, then waits for
the explicit device approval. After approval, the owner creates the normal short-lived,
Governor-signed Rampage invite, encrypts it for that request, and sends the ciphertext directly to
the laptop. The laptop authenticates and persists it, returns an encrypted completion receipt, and
restarts.

After the controller consumes the one-time secret, the laptop replaces the invitation with a
Governor-signed controller pin and removes the enrollment secret. The pin remains a transport
identity anchor after the ten-minute discovery record expires; it grants no job or lease authority.
The full invitation never appears in the normal UI and is never sent in discovery broadcasts.

This code-free flow is intentionally trust-on-first-approval inside a private LAN. The device name is
helpful context, not hardware attestation. Use it only when you expect that device and control the
network; a hostile user already on the same LAN could attempt to race or spoof a request. For an
untrusted or segmented network, use **Advanced: use a complete invite** and transfer the complete
signed invite through a separately trusted channel.

Rampage sends discovery through multicast, global broadcast, and the directed broadcast address of
every active non-loopback LAN interface. A VPN or virtual adapter therefore cannot win one routing
decision and hide the real Wi-Fi or Ethernet path. Each machine uses its own bounded local request
lifetime, so clock drift cannot silently reject an otherwise valid nearby machine.

The controller also persists its authenticated UDP port. An upgrade migrates the newest proven
legacy port from the evidence ledger before selecting the fixed port used by a new installation, so
an enrolled laptop does not lose its signed route merely because the main app restarts.

## Why discovery remains narrow

- The owner listener receives only bounded nearby requests; admitting a machine still requires one
  explicit owner action.
- A laptop request expires after fifteen minutes and contains only bounded metadata plus an ephemeral
  public key.
- Rampage accepts at most five new requests per source address per minute and sixteen pending
  requests total.
- Datagram size, labels, schemas, request identifiers, and public keys are validated before state is
  created. Remote timestamps never control the local expiry.
- Repeated requests reuse the same challenge; repeated approvals resend the same encrypted payload
  in authenticated sub-1 KiB fragments so packet loss and mixed Wi-Fi, VPN, or wired path MTUs do
  not force the user to start over.
- The controller and intelligence APIs remain token-protected and bound to loopback.

## First-run and recovery guarantees

An empty runtime is neutral: it creates no owner controller, worker pin, or fabric authority until
the person chooses **Create my fabric** or **Join my fabric**. Creating a fabric writes a durable owner
confirmation marker. Joining writes a protected pairing-intent marker before any invitation may be
persisted.

Recovery can retire an older implicit owner bootstrap only when it has never been confirmed. It
stops the managed sidecars, waits for their process trees to exit, rotates the bounded runtime into
setup, preserves a valid firewall marker, and then begins discovery. A confirmed owner fabric and an
active worker identity are never self-deleted by this transition.

## If the machines do not appear

Check these in order:

1. Both machines are on the same private LAN, not a guest Wi-Fi network with client isolation.
2. **Find my fabric** is still waiting on the laptop.
3. Rampage is running on the main PC.
4. Windows Defender Firewall allows Rampage on private networks.
5. A VPN or corporate endpoint policy is not blocking local broadcast and multicast.

Retrying creates fresh ephemeral keys and a fresh request. For intentionally segmented networks,
open **Advanced: use a complete invite** on the joining machine and transfer the complete signed
invite through a trusted channel.
