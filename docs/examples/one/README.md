# Example One: Disputed Handoff Payload

**Scenario:** A wire service's intake agent (Meridian) verifies a batch of images submitted by a freelance journalist, then hands off verdicts to a publisher's editorial agent (Castellan) for clearance decisions.

**What the logs show:** Both agents behave compliantly — each runs its Anchor instance, records all required events, and independently anchors the handoff using the same `handoff_id`. However, when OpenOrigins matches the two HANDOFF records, the `payload_hash` values differ: Meridian claims it sent one set of verdicts; Castellan recorded receiving a different one.

**What this illustrates:** The core handoff integrity guarantee. Neither party's log is authoritative on its own. OpenOrigins holds two independently anchored records and the hash mismatch is itself the finding — surfaced without reading any payload content. To resolve whose version is correct, OpenOrigins requests voluntary production of each party's plaintext and verifies it against the anchored hash.

**Finding type:** Cross-party contradiction detectable from PUBLIC-tier fields alone.
