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

| Scenario | Action | Expected | Actual | Status | Evidence |
|---|---|---|---|---|---|
| S1 |  |  |  |  |  |

## Handoffs to Bob

> Append a `## → BOB: pickup at S<N> (branch @ <sha>)` block, then `git push`.
