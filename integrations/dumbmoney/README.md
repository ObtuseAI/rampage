# DumbMoney infrastructure cell

Rampage integrates as an optional bounded infrastructure fabric cell. It stays outside
DumbMoney's investment decision path and receives no trading, portfolio, credential, policy, or
promotion authority.

```text
DumbMoney owned telemetry exporter
  -> rampage.dumbmoney.telemetry-bundle.v1
  -> Rampage read-only observer
  -> governed compute / experiments / evidence
  -> Rampage CAS artifact + signed rampage.proposal-envelope.v1
  -> DumbMoney-owned ACL inbox
  -> DumbMoney validates, reviews, and decides
```

The bridge never opens DumbMoney's ledger or runtime database, never receives its install HMAC
secret, and never calls Dummy/Dopey credentials or broker adapters. The same Rampage installation
continues to work universally when this integration is absent.

Adoption is intentionally additive: the DumbMoney repository remains the owner of its stages,
runtime bindings, venues, and contract catalog. Rampage only supplies the versioned contract pair
in this directory until DumbMoney explicitly accepts it.
