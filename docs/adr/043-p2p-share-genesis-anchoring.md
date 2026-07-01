# ADR-043: P2P share integrity & genesis anchoring

**Status:** Accepted (2026-07-01). Pre-dogfood deep dive, Workstream C (issue #182, umbrella #251).
Closes the "p2p/share_kb has no signed genesis" finding on #237.
**Amends:** ADR-025 (P2P mesh transport), ADR-026 (peer-verifiable signed-hash-chained membership),
ADR-037/039 (E2E content encryption / key-blind membership).
**Relates:** ADR-018 (roles/policy), ADR-042 (the derive that consumes the genesis).
**Prior art:** the hub `kb/share` genesis-seed (`daemon/src/collab_handler.rs`, "Seed the genesis
owner self-admit").

## Context

A KB shared over the **mTLS hub** is peer-verifiable and E2E-capable because `kb/share` seeds a
**signed owner-genesis** op (the anchored root of the membership op-log). A KB shared over the
**P2P mesh** was not: `establish_p2p_share` (the `p2p/share_kb` fresh-share path) built the
collection with `KnowledgeBase::to_collection` / `KbCollectionDoc::new` — a **roster-only manifest
with an empty op-log** — then set owner/policy/transport and exposed it.

Consequences of the missing genesis:
- **Not peer-verifiable (ADR-026).** A joining peer has no anchored, signed root to verify the
  membership chain against.
- **Not E2E-capable (ADR-037).** `derive_valid_members` and `find_wrapped_content_key` resolve
  membership + content-key wraps from the genesis-anchored op-log; with no genesis there is nothing
  to anchor, so enabling E2E on a fresh mesh-shared KB has no derivable member set to wrap to.

This blocked the dogfood's intended workflow: **share an enterprise KB over the mesh *with*
encryption.** (E2E did work on a KB first shared+enabled on the hub, then widened to the mesh — but
a fresh mesh-only share had no anchor.)

## Decision

`establish_p2p_share` **seeds the signed owner-genesis** (an `Admit` self-op, `Role::Owner`,
`can_invite`, authored + signed by the daemon's owner identity, epoch 0) into the collection op-log
**before** it shares the doc — byte-for-byte the same shape the hub `kb/share` path produces, so a
mesh-shared KB anchors membership and E2E key-derivation **identically** to a hub-shared one. It
fires whenever the op-log is empty (`to_collection` / `new` always start empty), and is idempotent
on a re-share (the widen path leaves an existing op-log untouched, B-12).

## Verification (the falsifier)

- **Unit:** `share_kb_seeds_a_signed_owner_genesis_so_the_collection_is_anchorable` — a fresh
  `p2p/share_kb` produces a collection whose op-log has a genesis, and the owner **derives as
  `Owner`** from that signed log anchored on the owner's key (was impossible before).
- **End-to-end (`MAE_E2E_MESH` gate, wired into CI):** two daemons over real iroh — owner shares a
  KB over the mesh and **enables E2E**. The gate PROVES: the content is **sealed** and both daemons
  are **key-blind** (canary plaintext ABSENT from either daemon's store + logs; the owner authors +
  reads it). This is the property the genesis anchor delivers — E2E now *seals* over the mesh, which
  was impossible before.

## What verify surfaced (a deeper gap — #255)

The gate also revealed that the genesis seed is **necessary but not sufficient** for a *member* to
read over the mesh: a joining member's published wrap pubkey does not reach the owner through the
mesh join path (`kb/approve: E2e KB but the pending request carries no pubkey`), so members are
admitted **keyless** and cannot decrypt yet. The gate pins this honestly (KNOWN-GAP #255, non-fatal)
so the *sealing/key-blindness* property is a green regression guard while member key-delivery over
the mesh is tracked + fixed separately. (A related finding: the **permissive** auto-join path records
a member without their wrap pubkey even on the hub — permissive is incompatible with E2E membership
as-is; use invite. Also #255.)

## Consequences

- E2E now **seals over the mesh** (relays key-blind) — real, gated progress. Full E2E-on-mesh (a
  member joining over the mesh and decrypting) additionally needs the #255 member-key-delivery fix;
  until then **E2E remains hub-recommended** and the E2E_USER_GUIDE §7 caveat stays (updated to name
  the exact gap rather than a blanket "mesh = non-E2E").
- No change to the hub path or to an existing mesh collection (re-share widens transport only).
- The daemon owner identity must be present (key mode) to sign the genesis — consistent with
  `p2p/share_kb` already requiring the mesh + owner identity.
