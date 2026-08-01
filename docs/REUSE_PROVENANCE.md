# Clean-room reuse provenance

Rampage is a new implementation. It may reproduce architecture patterns learned from ObtuseAI
projects, but it must not copy source, tests, assets, or protected datasets from repositories whose
licenses do not permit that use.

| Source family | Pattern learned | Rampage implementation rule |
| --- | --- | --- |
| DumbMoney | authority graph, drift latch, read-only native bridge | New typed contracts; no ledger/database writes |
| Blunder | capability registry, isolated missions | New lease/effect model |
| Dummy | scientific memory, preregistration, sealed holdouts | New evidence bundle and promotion gates |
| Doofus | typed missions and evidence court | New job and receipt schemas |
| Nimrod | threshold signing, evaluator/promoter split | New Governor signing boundary |
| Dopey | ambiguity state and protected surfaces | New fail-closed reconciliation state |
| Dimwit | proof-bearing tools | New artifact and verification receipts |
| Waterboy | deterministic replay | New replay contract and ledger verifier |
| Jester | diverse proposals, failure memory, spatial observatory | New lineage and Arena concepts |

Every future port must name the source commit, license, copied files (normally none), reviewer, and
decision in this document before merge.

