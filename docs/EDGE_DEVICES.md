# Phones, tablets, consoles, and handhelds

Edge devices are useful for bounded map work, hashing, validation shards, preprocessing, and other
restart-tolerant tasks. They are not presented as transparent RAM, durable protected storage, or an
always-on GPU. A phone or console joins only during an explicit foreground donation session.

The Governor requires all of the following before issuing an edge lease:

- a mobile/edge allowlisted adapter;
- a restart-tolerant job;
- foreground donation still enabled;
- AC power or battery at/above the owner's floor;
- thermal headroom at/above the owner's floor;
- a short, signed, fenced lease.

The session contract expires. Locking the screen, navigating away, crossing the battery/thermal
floor, losing the network, or pressing STOP ends eligibility. Independent shard sets preserve a
restart-tolerant unit that can be safely resubmitted elsewhere; automatic lost-lease retry is not a
0.1 claim. Rampage never treats the edge device as a protected replica.

The 0.1 release ships a Windows desktop worker and the cross-platform Rust worker source. Native
phone, tablet, and console companion binaries are a supported extension point, not a shipped claim.
Platform holders may also prohibit background execution or unsigned runtimes. Until a native client
passes the same enrollment, power, thermal, receipt, and kill-latch conformance suite, the Governor
will not classify it as an edge worker.
