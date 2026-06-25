# P2P Mesh Test Notes — BOB (machine B, joiner)

> Live log for the `feat/p2p-setup-and-mesh` two-machine cycle.
> Protocol + scenarios: [`p2p-mesh-two-machine-testing.md`](p2p-mesh-two-machine-testing.md).
> Sibling (do not edit): [`p2p-test-notes-alice.md`](p2p-test-notes-alice.md).
> The v0.14 logs (`collab-test-notes-*.md`) are case-study material — do not touch.
> Rotate resolved issues + ✅ rows into `docs/p2p-test-notes-bob.archive-YYYYMMDD.md` past ~800 lines (protocol §0.2).

## Environment (this machine)

- Host / OS / IP:
- Branch @ commit (`git rev-parse --short HEAD`):
- `mae` / `mae-daemon` version + binary sha256 (`make verify-binary`):
- **Daemon node-id** (fingerprint, `mae-daemon identity`):
- This editor's identity (fingerprint):
- **Alice's daemon node-id** (from out-of-band / the join ticket):
- Join ticket received: `mae://join/…`
- KB fixture: `tests/fixtures/kb/collabtest` (sentinels ZEPHYRINE / QUOKKA / NARWHAL)
- iroh relay mode: `default` | `disabled` | custom `<url>`
- `connection_gate`: `authorized_keys` | `open`
- Ports / firewall / NAT notes:

## Issues (P-NN)

> Symptom → Evidence → Diagnosis (file:line) → Fix direction → Proof (commit + test).
> Mark `✅ FIXED (<sha>)` when a test pins it.

_(none yet)_

## Scenario log

> One row per action. Probe slugs `[BOB-S<N>]` in the edited node title.
> Status: ✅ pass · ❌ fail · ⚠️ unexpected · 🔧 worked-around · ⏳ pending peer.

| Scenario | My action (joiner) | Expected | Actual | Status | Evidence |
|---|---|---|---|---|---|
| S1 | receive the `mae://join/…` ticket from alice (OOB) | ticket in hand |  |  |  |
| S2 | `mae kb-join <ticket>` (then wait for alice to approve) | I pull the KB; `remote_id` verified; anchor set |  |  |  |
| S3 | `kb_sharing_status collabtest` (or `*KB Sharing*`) | I derive alice=Owner from the signed op-log (anchored) |  |  |  |
| S4 | watch `collabtest:overview` converge from alice | converges live to `[ALICE-S4]` |  |  |  |
| S5 | edit `collabtest:beta` title → `[BOB-S5]` | alice converges live (`changed=true`) |  |  |  |
| S6 | edit `collabtest:alpha` → `[B-S6]` (concurrent w/ alice) | both converge byte-identical |  |  |  |
| S7 | S7b: offline edit `[BOB-S7B]` + reconnect; S7c: switch network | converge after offline/switch; revoked peer denied on reconnect |  |  |  |
| S8 | observe my derived membership after alice's change | a removed member is denied at the gate |  |  |  |
| S9 | local-blocklist a principal (try even the owner) | blocked principal dropped from my derived set, locally |  |  |  |
| S10 | dial a ticket with a **wrong node-id** (alice's addr) | never joins; no anchor registered on failure |  |  |  |
| S11 | try to mesh-reach alice's hub-only KB | denied — "not shared over the P2P mesh" |  |  |  |
| S12 | (a 3rd unauthorized daemon dials alice) | rejected at alice's accept gate |  |  |  |
| S13 | `:collab-discover` | I discover alice's `_mae-sync._tcp` service; I'm self-filtered |  |  |  |

## Handoffs to Alice

> Append a `## → ALICE: pickup at S<N> (branch @ <sha>)` block, then `git push`.
