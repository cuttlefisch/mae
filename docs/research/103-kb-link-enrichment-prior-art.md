# Prior-Art Research: Autonomous Local-LLM Enrichment of an Org-Roam-Style KB via Confidence-Gated Typed Auto-Linking

*Phase-0 grounding for ADR-103 (autonomous local-LLM link enrichment). Compiled August 2026.
Load-bearing claims are cited with URLs and labeled STRONG (peer-reviewed / primary engineering
evidence) or WEAK (vendor docs, forum/blog synthesis, indirect). Contested points are flagged inline.*

## Scope and the specific gamble in the design

The proposed system couples four things that each have their own literature: (1) embedding-based
candidate retrieval of "related notes," (2) LLM-based *typed* relation prediction (not just "these are
similar" but "A `supports` B"), (3) an LLM-reported *confidence score* used as a gate, and (4)
autonomous 24/7 operation with only high-confidence writes auto-applied. The prior art is broadly
encouraging on (1) and (4) but delivers pointed warnings on (2) and (3) — precisely the two novel
load-bearing steps. The central risk this report surfaces: **a small local LLM's self-reported
confidence is the weakest link, and typed relation extraction is where small models degrade most.**
The design's safety therefore cannot rest on the confidence number alone.

---

## Area 1 — Embedding-based "related notes" / auto-linking in real tools

**Findings.** The dominant shipped pattern is *suggestion, never auto-apply.* Obsidian's Smart
Connections — the most-installed semantic-linking plugin, running a **local** embedding model with
zero setup — surfaces related notes in a sidebar with similarity scores; the user must *drag results
in* to create a link, and the UI provides pause, pin, and hide controls to manage noise. Nothing is
written automatically ([GitHub: obsidian-smart-connections](https://github.com/brianpetro/obsidian-smart-connections))
(STRONG for the mechanism; it's the official repo/docs). Roam and Logseq's "unlinked references" are
the same philosophy: the tool *computes* candidate connections (text co-occurrence) and shows them;
linking is a manual click ([Ness Labs: Roam to Logseq](https://nesslabs.com/roam-to-logseq)) (WEAK,
secondary).

**What worked.** Suggestion-with-score plus cheap dismissal is the durable design. The value is
discovery ("connect notes in surprising ways") while leaving authority with the human
([Sweet Setup PKM comparison](https://thesweetsetup.com/obsidian-vs-roam/)) (WEAK).

**What failed.** Three recurring failure modes appear in Smart Connections' issue tracker and PKM
discourse: (a) **semantic false positives** — "a note that contains the exact query text might not
appear if it is not actually similar in meaning," and conversely semantically-near-but-contextually-
irrelevant notes surface ([Smart Connections docs](https://github.com/brianpetro/obsidian-smart-connections))
(STRONG); (b) **graph hairball / noise** — "Roam's graph view is famous, but it can get noisy"
([Sweet Setup](https://thesweetsetup.com/obsidian-vs-roam/)) (WEAK); (c) **auto-tagging degrades a
messy base** — "auto-tagging works well enough as a first pass, but as a final answer, no — review
the suggestions… a messy tag system will produce messy auto-tags"
([Medium: Automating Tagging in PKM](https://medium.com/@theo-james/automating-tagging-and-linking-with-ai-plugins-1a76bda05637))
(WEAK). False positives are the standing complaint in semantic-search-for-notes generally
([Buildin: PKM with AI](https://buildin.ai/blog/personal-knowledge-management-system-with-ai)) (WEAK).

**Design lesson.** No shipped, widely-adopted note tool auto-writes semantic links unattended. The
proposed system is stepping *past* the current art's safety line; that step must be earned by the
confidence gate, and even then the incumbents suggest keeping a visible, one-click "reject" affordance
and a similarity score on every candidate.

---

## Area 2 — Knowledge Graph Completion / link prediction

**Findings — embedding methods.** On the two canonical benchmarks, classic embedding models plateau at
modest accuracy. Entity-prediction (predict the missing head/tail given relation), filtered setting:
**ComplEx ≈ 0.32 MRR and RotatE ≈ 0.34 MRR on FB15k-237; both ≈ 0.47 MRR on WN18RR**
([ResearchGate link-prediction table](https://www.researchgate.net/figure/Link-prediction-results-on-WN18RR-and-FB15k-237-The-second-row-shows-our-results-of_tbl1_348345008))
(STRONG for the ballpark; these figures are stable across the literature). An MRR of ~0.33 means the
correct entity is, on average, ranked around 3rd — i.e., **top-1 precision is well below 50%** on
FB15k-237 even for tuned specialist models on a clean benchmark.

> **Contested / trap flagged:** Some recent papers advertise "**99.8% MRR on FB15k-237**" (e.g.,
> Flow-Modulated Scoring) ([arXiv 2506.23137](https://arxiv.org/abs/2506.23137)). These are
> **relation-prediction** numbers (predict *which relation* holds given both entities), a far easier
> task than entity prediction, and are **not comparable** to the ~0.33 entity-prediction MRR. Do not
> let a headline "99%" mislead the threshold design — the relevant task here (given two notes, predict
> the typed link) is closer to relation prediction, but the candidate-generation step is entity/link
> prediction, which is the ~0.33 regime.

**Findings — LLM relation extraction / KG construction.** LLMs are strong reasoners but weak
structured extractors. In the most-cited systematic study, **GPT-4 one-shot relation extraction scored
41.91 F1 on DuIE2.0 vs. a fine-tuned baseline's 69.42; 22.5 vs. 91.4 on Re-TACRED; 9.1 vs. 53.2 on
SciERC** ([arXiv 2305.13168](https://arxiv.org/html/2305.13168v3)) (STRONG). The authors conclude LLMs
are "**limited… as a few-shot information extractor, yet… proficient as an inference assistant.**"
Notably, GPT-4 did *better* on link *prediction/reasoning* (FB15K-237 hits@1 ≈ 40.0, near the
fine-tuned 32.4) than on extraction — relevant, because the proposed task is closer to "does relation
R hold between these two known nodes?" (a judgment/reasoning task) than to open extraction.

**What failed.** Hallucinated relations are the named risk: LLMs' "propensity… to generate non-factual
information" requires output scrutiny ([arXiv 2305.13168](https://arxiv.org/html/2305.13168v3))
(STRONG). Even frontier LLMs produce non-standardized relation labels that don't align to a fixed
schema without constraint
([Medium: LLMs for KG Construction](https://medium.com/@jack16900/llms-for-knowledge-graph-construction-and-reasoning-41cc5308f8c8))
(WEAK).

**Design lesson.** Frame the LLM's job as **binary/ternary judgment over a *pre-supplied* candidate
pair and a *fixed* relation vocabulary** ("does `supports`/`refutes`/`elaborates`/none hold?"), not
open extraction — the reasoning framing is where LLMs are relatively strong and the extraction framing
is where they collapse. And accept that even the retrieval step's precision, benchmarked, is
coin-flip-ish; the human review queue is not optional insurance, it is load-bearing.

---

## Area 3 — LLM confidence calibration (the crux)

**Findings.** Raw self-reported confidence is **partly trustworthy for large models in-domain and
untrustworthy out-of-domain / for small models.**

- **Kadavath et al., "Language Models (Mostly) Know What They Know"** (STRONG,
  [arXiv 2207.05221](https://arxiv.org/abs/2207.05221)): larger models are "**well-calibrated on
  diverse multiple choice and true/false questions when provided in the right format**"; P(True)
  self-evaluation scales encouragingly; **but models "struggle with calibration of P(IK) on new
  tasks"** — i.e., cross-domain generalization of "do I know this" is the failure point. A
  note-linking task over a heterogeneous personal KB *is* a new/shifting distribution.
- **Tian et al., "Just Ask for Calibration," EMNLP 2023** (STRONG,
  [ACL 2023.emnlp-main.330](https://aclanthology.org/2023.emnlp-main.330/)): for RLHF models,
  **verbalized confidence is better-calibrated than the model's token logprobs, often cutting expected
  calibration error ~50%** — because RLHF systematically *degrades* logprob calibration. Practical
  implication: **ask the model for a confidence number rather than reading raw logprobs** — but this
  result is on frontier RLHF models, not 7B–14B local ones.
- **Self-consistency / sampling** (STRONG): agreement across multiple stochastic samples is "generally
  better than standard token-probability calibration… and produces sharper confidence estimates"
  ([survey, arXiv 2607.08065](https://arxiv.org/html/2607.08065v1)); two samples can already help
  ([OpenReview: Two Samples Are Enough](https://openreview.net/forum?id=66D3rZrNjV)).

**What failed — the critical caveat.** **Agreement ≠ correctness, and confident errors are systematic,
not random.** The audit "When LLMs Agree, Are They Right?"
([arXiv 2607.08065](https://arxiv.org/html/2607.08065v1)) (STRONG) found: agreement is a "**positive
but weak predictor**" (correlations 0.20–0.59); the *most self-consistent* model (GPT-4.1) was
**wrong 48% of the time when it expressed high confidence** on GPQA; and **28% of hard cases showed
the *same wrong answer* at maximum consistency across every sampling run** — "shared bias rather than
sampling noise," recurring even across GPT and Claude. So sampling-agreement can manufacture false
confidence on exactly the systematically-hard cases.

**Design lesson.** (a) **Never trust a single verbalized number from a 7B–14B model as a hard gate.**
(b) Prefer **verbalized confidence + self-consistency across ≥2–3 samples**, but treat *unanimous
agreement* as a *necessary-not-sufficient* signal — because unanimity is where systematic errors hide.
(c) Because calibration collapses out-of-domain, the operating threshold must be **empirically set
against a labeled sample of your own KB's link judgments**, not adopted from a paper. (d) Miscalibration
is asymmetric-costly here (a wrong auto-applied link pollutes the graph silently), so gate
conservatively.

---

## Area 4 — Human-in-the-loop review for auto-generated knowledge

**Findings.** The industry-standard architecture for confidence-gated automation is a **double
(three-band) threshold**: auto-apply above a high bar, **route the middle band to a review queue**,
and auto-reject below a low bar
([Mavik Labs: HITL review queues](https://www.maviklabs.com/blog/human-in-the-loop-review-queue-2026/);
[Databricks: HITL](https://www.databricks.com/blog/human-in-the-loop)) (WEAK, but consistent across
multiple independent practitioner sources). A commonly cited concrete setting is **auto-approve ≥ 0.90,
review 0.70–0.90, reject < 0.70** ([Mavik Labs](https://www.maviklabs.com/blog/human-in-the-loop-review-queue-2026/))
(WEAK — a rule of thumb, not validated for link prediction). Active learning refines this by surfacing
"the most uncertain, risky, novel, or high-impact cases" for human labeling
([Databricks](https://www.databricks.com/blog/human-in-the-loop)) (WEAK).

**What worked / cost asymmetry.** The recurring justification is **trust**: "stakeholders accept AI
faster when they know a human can intervene and overrule" ([Databricks](https://www.databricks.com/blog/human-in-the-loop))
(WEAK). For a KB, the cost asymmetry is stark: a **false positive (a wrong auto-applied typed link) is
near-silent, persistent, and pollutes downstream retrieval/reasoning**, while a **false negative (a
missed link left in the review queue) is cheap and recoverable**. This argues for **precision-oriented
gating** — set the auto-apply bar high, accept lower recall.

**Design lesson.** Use three bands. Keep auto-apply narrow. Make the review queue the default
destination, not the exception, in early operation. Feed human accept/reject decisions back as
calibration labels (active learning) to *earn* a lower threshold over time rather than assuming one.

---

## Area 5 — Small/local LLM reliability for structured relation extraction / JSON

**Findings — accuracy.** 7B–14B models are materially behind frontier on the *judgment* content, even
where formatting is fine. Generic-domain relation/entity extraction is hard even for GPT-4 (average
NER F1 ≈ 59.5 in one study; [arXiv 2506.02589](https://arxiv.org/pdf/2506.02589)) (WEAK), so a 7B model
doing *typed relation* prediction should be expected to be worse on the semantic decision, not just
the syntax. Qwen2.5 is repeatedly singled out as **better than Llama at structured/JSON output and
entity extraction** ([Ertas AI benchmark](https://www.ertas.ai/blog/fine-tune-llama-3-3-qwen-2-5-qlora-benchmark);
[Qwen2.5 blog](https://qwenlm.github.io/blog/qwen2.5-llm/)) (WEAK).

**Findings — constrained decoding (STRONG here).** Grammar/JSON-schema-constrained decoding **reliably
fixes format** and closes much of the small-vs-large gap on *well-formedness*: a fine-tuned
**Mistral-7B with constrained decoding hits ~99.5% schema accuracy**, and **grammar-constrained
Llama-3.2-3B can outperform an unconstrained Llama-3.1-70B** on function-calling by eliminating
malformed calls ([TianPan: grammar-constrained generation](https://tianpan.co/blog/2026-04-16-grammar-constrained-generation-output-reliability);
[DOMINO, arXiv 2403.06988](https://arxiv.org/pdf/2403.06988)) (STRONG for the format claim).
JSONSchemaBench evaluates six frameworks on validity/coverage/quality
([arXiv 2501.10868](https://arxiv.org/abs/2501.10868)) (STRONG that this is the reference benchmark).

**What failed — the format tax.** Forcing structured output **degrades reasoning by ~10–15%**, and —
critically — "**format-requesting instructions alone cause most of the accuracy loss, before any
decoder constraint is applied**"; JSON mode "hinders reasoning because the model may be forced to
output answer fields before completing chain-of-thought"
([Let Me Speak Freely, EMNLP 2024](https://aclanthology.org/2024.emnlp-industry.91.pdf);
[The Format Tax, arXiv 2604.03616](https://arxiv.org/html/2604.03616)) (STRONG). Capacity-limited
(small) models suffer the *largest* penalty. The fix is to **decouple reasoning from formatting**: let
the model reason in free text first, then emit constrained JSON.

**Design lesson.** Yes to constrained decoding — but **only for the final structured emission of an
already-reasoned decision**, never wrapping the reasoning itself. Concretely: prompt the model to
reason about the relation in prose (including its confidence rationale), *then* constrain-decode a
small JSON object `{relation, confidence, evidence_span}`. Prefer a Qwen2.5-class model. Keep each call
a **single focused judgment** (one candidate pair, fixed relation vocabulary) rather than a long
multi-tool plan — small models degrade sharply on long agentic chains.

---

## Area 6 — Automated KB-maintenance failure stories

**Findings.** Direct, well-documented post-mortems of *LLM* auto-linking degrading a personal KB are
thin (the practice is new); the transferable failure evidence is (a) automation degrading tag/link
quality, (b) semantic "soft" link rot, and (c) sustained-effort dependence.

- **Auto-tagging into a messy base compounds the mess** ([Medium PKM tagging](https://medium.com/@theo-james/automating-tagging-and-linking-with-ai-plugins-1a76bda05637))
  (WEAK).
- **Semantic / soft link rot** — a link can return HTTP 200 yet point to content that has drifted to be
  "entirely irrelevant to the context in which the link was originally embedded"
  ([Wikipedia: Link rot](https://en.wikipedia.org/wiki/Link_rot)) (STRONG as a phenomenon). For typed
  KB links this maps to: an auto-applied `supports` link becomes wrong when either note is later edited
  — **auto-links decay silently and nobody is watching.**
- **Trust erodes through false positives** ([Buildin PKM](https://buildin.ai/blog/personal-knowledge-management-system-with-ai))
  (WEAK). The generalizable dynamic: a small number of visible bad auto-applied decisions
  disproportionately destroys trust in the whole automation
  ([Databricks HITL](https://www.databricks.com/blog/human-in-the-loop)) (WEAK).

**Design lesson.** Two under-appreciated risks specific to *autonomous* operation: (1) **link
staleness** — auto-applied links need a re-validation pass when either endpoint changes, or they rot
into wrong assertions; (2) **trust cliff** — because a handful of visible false links can make a user
abandon the feature entirely, the *first weeks* of operation should be review-heavy and the auto-apply
bar deliberately over-conservative until the user has seen the system be right repeatedly.

---

## Design implications → concrete recommendations (mapped to ADR-103 decisions)

**(a) Assigning link confidence [→ ADR-103 D4].** Do **not** use raw single-shot verbalized confidence
from a 7B–14B model. Combine three signals: **embedding similarity** (candidate prior) × **verbalized
confidence** (ask for it — better than logprobs, Tian et al.) × **self-consistency** across 2–3 samples.
Treat **unanimous agreement as necessary-but-not-sufficient** (arXiv 2607.08065). **Calibrate raw
score → gate on a hand-labeled sample of the actual KB** (Kadavath P(IK)-on-new-tasks).

**(b) Default auto-apply threshold [→ ADR-103 D5].** Start **high and precision-oriented**; the ≥0.90 /
0.70–0.90 / <0.70 three-band rule is a reasonable *initial* shape, but **begin even more conservative
(auto-apply only at ≥0.95 *and* full sample agreement)** and loosen only after the human accept-rate
justifies it.

**(c) Constrained decoding: yes — but scoped [→ ADR-103 D9].** **Yes** for the **final emission**
`{relation, confidence, evidence}`; **no** for the reasoning step (format tax). **Two-phase:** free-text
reasoning first, constrained JSON second. Restrict `relation` to a **fixed enum** via the grammar so
the model cannot invent relation types.

**(d) Review-queue structure [→ ADR-103 D6/D7].** Three bands. Review queue is the **default early
destination**; each entry carries evidence span + confidence + similarity prior. **Every human
accept/reject becomes a calibration label** (active learning). Auto-applied links are reviewable and
one-click-revertible.

**(e) Guardrails against over-linking / hairballs [→ ADR-103 D8 guardrails].** Cap link density per
node (top-k). Require a fixed relation vocabulary **+ an easy "none" option** (over-linking comes from
a model reluctant to say "no relation"). **Staleness re-validation** when either endpoint is edited.
**Federation caution** — cross-KB auto-apply held to a stricter threshold or routed to review
initially. **Trust-cliff protection** — review-heavy initial period + auditable, bulk-revertible trail.

**Bottom line.** The prior art *supports* the overall shape but *warns hardest* about the two novel
steps: small-model typed-relation judgment is materially weaker than frontier extraction (itself weak),
and small-model self-confidence is miscalibrated out-of-domain with systematic (not random) confident
errors. The safe reading: **the confidence number is an input to a conservative, empirically-calibrated,
human-backstopped gate — never the gate itself.**
