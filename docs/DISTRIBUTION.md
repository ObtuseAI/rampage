# Native distribution and signing

Rampage separates build provenance from platform identity. Every non-pull-request native build is
eligible for a GitHub OIDC-backed artifact attestation. A stable Windows or macOS release must also
pass the platform's own identity and trust checks; provenance alone is not presented as
Authenticode, Gatekeeper, or notarization.

## Qualified builders

The native distribution workflow builds on three fresh GitHub-hosted environments:

| Artifact | Runner | Package formats | Stable trust gate |
| --- | --- | --- | --- |
| Windows x64 | `windows-2022` | MSI and NSIS | Every bundled executable and installer must have a valid Authenticode signature |
| Linux x64 | `ubuntu-24.04` | Debian and AppImage | GitHub artifact attestation; repository/AppImage signing remains channel-specific |
| macOS Apple Silicon | `macos-15` on M1 | App bundle and DMG | `codesign`, Gatekeeper assessment, and a stapled notarization ticket |

These runners qualify reproducible native packages for their named environments. A Windows Server
2022 runner is not evidence for Windows 10. Windows 10 remains unqualified until a dedicated
`self-hosted`, `windows`, `x64`, `rampage-windows-10` runner completes the installer and lifecycle
campaign. The manual `Windows 10 qualification` workflow is fail-closed on the operating-system
caption and architecture, then exercises installation, desktop and shell shortcuts, autonomous
sidecars, close-to-tray behavior, explicit exit, uninstall cleanup, and restoration of any
pre-existing shortcuts and registry state. No matching runner is currently enrolled, so the matrix
continues to label Windows 10 unexecuted.

## Candidate versus stable

- Pull requests build unsigned candidate packages on all three platforms and upload them as
  workflow artifacts. They are test inputs, not releases.
- A prerelease tag such as `v0.3.0-rc.1` may publish unsigned candidate packages, but the manifest
  states that platform signatures were not required or verified.
- A stable tag such as `v0.3.0` activates fail-closed platform trust. Missing credentials, an
  invalid signature, a failed Gatekeeper assessment, or a missing notarization ticket stops the
  workflow before release publication.
- A release tag must match the Cargo workspace, Tauri app, desktop package, intelligence service,
  and both SDK versions exactly; drift stops every matrix job before packaging.
- Every staged asset receives a SHA-256 checksum and a `rampage.distribution-manifest.v1` record
  bound to the exact source commit.

## Required stable-release configuration

Windows requires repository secrets `WINDOWS_CERTIFICATE` (base64 PFX) and
`WINDOWS_CERTIFICATE_PASSWORD`, plus repository variable `WINDOWS_TIMESTAMP_URL`. The certificate
must be a real code-signing identity with its private key. The workflow imports it into a temporary
runner certificate store, signs every external sidecar, asks Tauri to sign the application and
installers, and verifies every signature before staging.

macOS requires repository secrets `APPLE_CERTIFICATE` (base64 PKCS#12),
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, and
`APPLE_TEAM_ID`. The workflow imports the Developer ID identity into an ephemeral keychain; Tauri
signs and notarizes, then the staging gate independently verifies the signature, Gatekeeper result,
and stapled ticket.

No signing key is stored in the repository, emitted into an artifact, or made available to pull
request builds.

## Verification

After downloading an attested artifact:

```powershell
gh attestation verify .\Rampage_0.3.0_x64-setup.exe -R ObtuseAI/rampage
Get-FileHash .\Rampage_0.3.0_x64-setup.exe -Algorithm SHA256
```

Compare the second result to `SHA256SUMS-windows-x64`. Platform signature verification remains
additional: Windows Explorer or `Get-AuthenticodeSignature` for Windows; `codesign`, `spctl`, and
`stapler` for macOS.
