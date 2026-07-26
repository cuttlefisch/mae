# ADR-063 Phase C: live VS Code + Copilot guidance-usage check (human-executable)

**Status: not yet run.** This is the specific piece ADR-050's D4 originally called for and
this project never finished: "verify empirically whether VS Code's Copilot MCP client
forwards `initialize.instructions` into the model's context" — with a behavioral bar
("observably used," not "wire-present"), motivated directly by AWS's own real
`aws-toolkit-jetbrains#6134` incident (a schema-valid, correctly-delivered payload a
specific client's own agent implementation still silently misinterpreted).

**Why this can't be automated in this repo's CI today.** Driving a real VS Code window
with the Copilot extension's agent mode requires a live GUI session and a real model
backend making real inference decisions — no browser/GUI automation is available to an
agent working in this headless environment, and CI runners don't carry a licensed Copilot
session. `crates/mae/tests/guidance_delivery_e2e.rs` automates and proves everything on
MAE's own side of this handshake (the real content reaches `initialize.instructions`,
byte-identical, budget-respecting, with a real negative control) — this document is
specifically the remaining, human-side half: does a real external agent *act* on it.

## What this checks

Whether a real Copilot agent-mode session, paired with a real `mae --headless` instance
via MCP, reflects a *distinctive, unmistakable* guidance-KB practice in its very first
tool call or code suggestion in a fresh scenario — not merely that the text arrived over
the wire.

## Steps

1. **Build and install** the current `mae` binary from this branch
   (`cargo build --release --bin mae`, or `make install`).
2. **Create a scratch project** with nothing else guiding the agent (no existing
   `AGENTS.md`/`.github/copilot-instructions.md`, no repo-specific conventions):
   ```
   mkdir /tmp/adr063-copilot-check && cd /tmp/adr063-copilot-check && git init
   ```
3. **Seed a distinctive guidance KB** — a practice with no plausible alternative phrasing
   an untrained model would independently produce (mirrors the automated test's own
   `DISTINCTIVE_PRACTICE` fixture, deliberately reused so both halves of this
   verification target the identical claim):
   ```
   mae --ensure-guidance-config --guidance-kb Adr063Check
   ```
   then register content — easiest via the running editor's `kb_create`/`:kb-create` on
   an `index` node body reading exactly:
   > "MAE-GUIDANCE-E2E-MARKER-7f3a: every new Rust file must open with the exact comment
   > `// mae-adr063-canary` on line 1."
4. **Pair VS Code + Copilot** with this `mae --headless` instance per
   `docs/EXTERNAL_EDITOR_MCP_PAIRING.md`'s existing setup steps, in the scratch project
   from step 2.
5. **Open a fresh Copilot agent-mode chat** (no prior conversation history) and ask it to
   create a new, trivial Rust file (e.g. "create `src/lib.rs` with a function that adds
   two numbers").
6. **Inspect the agent's first generated file.** Record:
   - Did `initialize.instructions` (visible in Copilot's own MCP debug/trace output, if
     exposed, or inferred from behavior) actually reach the model?
   - **Pass**: the generated file's first line is exactly `// mae-adr063-canary`.
   - **Fail**: the marker is absent — record whatever the agent produced instead, and
     whether guidance content was visibly present in any captured wire trace despite not
     being acted on (the specific AWS-`cursorState`-shaped failure mode this whole ADR
     exists to catch: present but ignored).
7. **Record the result** (pass/fail, VS Code version, Copilot extension version, date) as
   a comment on the tracking GitHub issue for this phase, and update this file's Status
   line. A fail here is real, actionable signal — e.g. it may mean `initialize.instructions`
   needs to move to Copilot's own custom-instructions ingestion point instead of (or in
   addition to) the generic MCP field, which this check would be the first evidence for.

## Why this specific design

- **Behavioral, not wire-presence** — matches Decision C's explicit bar and the AWS
  precedent it's grounded in.
- **A distinctive, unmistakable marker** — an untrained model has no plausible reason to
  spontaneously produce `// mae-adr063-canary`; its presence is real signal, not
  coincidence.
- **Fresh scenario, no prior context** — rules out the marker appearing because of
  conversation history rather than the guidance delivery mechanism itself.
- **Reuses the identical fixture text** the automated `guidance_delivery_e2e.rs` tests
  already prove reaches the wire correctly — this document is checking the ONE remaining
  link in the same chain (wire → model → action), not a different claim.

This check should be periodically re-run against current VS Code/Copilot behavior, not
treated as a one-time pass that holds forever — ADR-063's own Consequences section notes
Copilot's agent-mode behavior evolves outside MAE's control.
