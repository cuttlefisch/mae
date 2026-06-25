# P2P Mesh Test Notes — ALICE (machine A, KB owner)

> Live log for the `feat/p2p-setup-and-mesh` two-machine cycle.
> Protocol + scenarios: [`p2p-mesh-two-machine-testing.md`](p2p-mesh-two-machine-testing.md).
> Sibling (do not edit): [`p2p-test-notes-bob.md`](p2p-test-notes-bob.md).
> The v0.14 logs (`collab-test-notes-*.md`) are case-study material — do not touch.

## Environment (this machine)

- Host / OS / IP:
- Branch @ commit (`git rev-parse --short HEAD`):
- `mae` / `mae-daemon` version + binary sha256 (`make verify-binary`):
- **Daemon node-id** (fingerprint, `mae-daemon identity`):
- This editor's identity (fingerprint):
- **Bob's daemon node-id** (from out-of-band):
- KB fixture: `tests/fixtures/kb/collabtest` (sentinels ZEPHYRINE / QUOKKA / NARWHAL)
- iroh relay mode: `default` | `disabled` | custom `<url>`
- `connection_gate`: `authorized_keys` | `open`
- Ports / firewall / NAT notes:

## Issues (P-NN)

> Symptom → Evidence → Diagnosis (file:line) → Fix direction → Proof (commit + test).
> Mark `✅ FIXED (<sha>)` when a test pins it.

_(none yet)_

## Scenario log

> One row per action. Probe slugs `[ALICE-S<N>]` in the edited node title.
> Status: ✅ pass · ❌ fail · ⚠️ unexpected · 🔧 worked-around · ⏳ pending peer.

| Scenario | My action (owner) | Expected | Actual | Status | Evidence |
|---|---|---|---|---|---|
| S1 | `mae kb-share-p2p collabtest`; send the ticket to bob OOB | ticket mints; KB p2p-exposed |  |  |  |
| S2 | approve bob if he lands pending (`kb_approve` / `*KB Sharing*`) | bob pulls; his `remote_id` verified; anchor set |  |  |  |
| S3 | — (bob verifies membership) | bob derives me=Owner from the signed op-log |  |  |  |
| S4 | edit `collabtest:overview` title → `[ALICE-S4]` | bob converges live |  |  |  |
| S5 | watch `collabtest:beta` converge from bob | I converge live (`changed=true`) |  |  |  |
| S6 | edit `collabtest:alpha` → `[A-S6]` (concurrent w/ bob) | both converge byte-identical (both slugs) |  |  |  |
| S7 | S7a: restart my daemon | converge after restart; revoke enforced on reconnect |  |  |  |
| S8 | add a member, then remove one | removed member denied at the gate |  |  |  |
| S9 | observe bob's local-blocklist effect | blocked principal dropped on bob only (not globally) |  |  |  |
| S10 | — (bob tests spoofing) | a wrong-node-id ticket never joins |  |  |  |
| S11 | share a 2nd KB **hub-only**; keep editing it locally | hub-only not mesh-reachable; owner-bypass works for me |  |  |  |
| S12 | gate = `authorized_keys` (default) | an unauthorized 3rd daemon is rejected at my gate |  |  |  |
| S13 | `collab-start` (registers `_mae-sync._tcp` mDNS) | bob's `:collab-discover` lists me; I'm self-filtered |  |  |  |

## Handoffs to Bob

> Append a `## → BOB: pickup at S<N> (branch @ <sha>)` block, then `git push`.
