# ADR-115: A KB's attachments live in a per-KB, content-addressed store, never in node text

**Status:** Accepted (owner, 2026-11-24), design; built phase by phase. Amendment 1 pins the link
form, the on-disk layout, the mae-org resolver contract and the build order. Nothing is built yet.
**Relates to:** ADR-113 (publication review; its D6 revision attachments become one use of this
store), ADR-109 (resource budget; attachments get their own cap and a storage quota), ADR-111
(remote clients over HTTPS), ADR-112 (mae-org renders the reference), ADR-093 (node CRDT schema; an
additive `attachments` map), ADR-105 (KB-scoped addressing), ADR-034 (the derived-artifact cache,
which this is NOT).
**Evidence:** `docs/research/115-attachment-store-prior-art.md`.

## Context

The browser KB editor needs images in pages. Two options were open: put the bytes in the node's org
text (a base64 data block), or store them beside the node and reference them.

The text option was measured against what exists and rejected:
- **The node caps.** MAE's editor refuses a body over `kb_max_node_bytes` (1 MiB), and the daemon
  refuses a document over `max_document_size_bytes` (10 MB, ADR-109 D6). Base64 inflates by 4/3,
  so one ~750 KB image fills a node by itself.
- **The read API.** `kb/query.get` cuts a body at `max_body_bytes` (and flags `body_truncated`), so
  an inline image can push the page text after it out of the response.
- **Sync.** The node's whole Yjs state travels on every join and full sync, and every client holds it
  in memory. A replaced image leaves deleted content in the document's history.
- **Search.** The projector indexes the body for full-text search and embeddings, so base64 noise
  would land in both.

What exists to store files beside a node:
- The daemon's content-addressed store (ADR-034) holds derived embeddings, not user content.
- ADR-113 D6 designs per-revision attachments, uploaded over HTTPS, but that is not built.

Three assumptions from the first sketch were wrong, and this design corrects them:
- **Tenant quotas count requests and connections, not bytes** (`tenant.rs`'s `RequestCost`). A
  storage-byte quota is new.
- **The HTTPS listener reads each request body whole** (`http_body_util::Limited`) and routes only
  JSON-RPC, health and the KB view. Streaming needs new routes.
- **A browser `<img>` cannot send a bearer token.** Images are fetched with the header and displayed
  from a `blob:` URL, as Matrix's clients do (MSC3916). Tokens never go in URLs.

## Decision

### D1. Bytes in files, metadata in SQLite

Each blob is an immutable file named by its KB and its sha256 (`blobs/<kb_id>/ab/<sha256>`). It is
written to a temporary file, flushed with `fsync`, then renamed into place, so a blob is either
complete or absent. SQLite holds the metadata:

- `kb_blob(kb_id, sha256, size, media_type, width, height, stored_at)`;
- `blob_ref(kb_id, sha256, holder)`, recording what references it (D5 lists every kind of holder).

SQLite's own measurement puts the internal-versus-file crossover near 100 KB, and images sit above it.
Storing immutable files avoids the usual consistency problem with external blobs: a file never
changes after its name exists. `mae-daemon backup` copies the blob directory alongside its
`VACUUM INTO` snapshot. Because the files are immutable, an incremental copy is exact.

Deduplication is per KB only. Two KBs holding the same bytes hold two files. Sharing one file across
KBs would let an upload's timing, or its response, reveal that another KB already holds those bytes.
A read is authorized by the KB's own row, never by knowing the hash.

### D2. Access is the KB's access

- **Read** (`GET`) needs the same `kb_access` read decision as `kb/query.get`.
- **Upload** (`POST`) needs `Edit`.

A non-member gets the same refusal whether or not the blob exists, in this KB or any other: no
oracle. Because storage is per KB (D1), nothing about another KB's blobs is ever consulted.

### D3. Transport: streaming HTTPS on the OAuth listener

| Request | Meaning |
|---|---|
| `POST /kb/<kb_id>/blobs` | Upload a file, with `Content-Length`. The answer is its address and description (below). |
| `GET /kb/<kb_id>/blobs/<sha256>` | Download a blob. |
| `HEAD /kb/<kb_id>/blobs/<sha256>` | Check whether this KB holds the blob, without downloading it. |

These are new routes, beside JSON-RPC.

**Upload:**
- The body is streamed to a temporary file, with constant memory whatever the size.
- The cap (`max_attachment_bytes`) is enforced as the body arrives. A declared or actual size over
  the cap stops the read: `413`, `quota_exceeded`.
- The daemon processes the file (D3a), then hashes the RESULT. That hash is the address. The client
  cannot choose it, and the answer is `{sha256, size, media_type, width?, height?}`.
- If the client sends `Content-Digest: sha-256=:…:` (RFC 9530) for the bytes it sent, a mismatch
  with what arrived is refused (`blob_hash_mismatch`) and the file deleted. This catches corruption
  in transit.
- An identical upload repeats harmlessly: the same processed bytes give the same address.

**Download:**
- `Content-Type` comes from an allowlist sniffed when the blob was stored: `image/png`, `jpeg`,
  `gif`, `webp`, `avif`, and `svg+xml`. Anything else is stored as `application/octet-stream`.
- Every response carries `X-Content-Type-Options: nosniff`,
  `Content-Security-Policy: sandbox; default-src 'none'`, and
  `Content-Disposition: attachment` for anything not on the allowlist.
- SVG is never served inline from the API origin. Clients that display it sanitise it first (the
  browser client's images ADR).
- `ETag` is the sha256. `Cache-Control: private, max-age=31536000, immutable`.

### D3a. Images are processed on the server, before hashing

What the daemon stores and serves is what it has checked. Every upload is typed by sniffing; images
are then processed:

- **Type from content.** The type is sniffed from the bytes, never taken from the client's header
  or file name. Anything not on the allowlist is stored as `application/octet-stream` and served as
  a download.
- **A decoding bound, checked before decoding.** Dimensions are read from the header and refused
  over `max_attachment_pixels` (default 40 megapixels). A decode runs under a memory and time limit,
  with a frame limit for animated formats. Decoding uses a memory-safe decoder in a bounded worker,
  never on the request task.
- **Metadata stripped.** EXIF (including location), XMP, comments and maker notes are removed.
  Orientation is applied to the pixels, rotating them, so the stored image displays upright without
  metadata. An embedded colour profile is kept.
- **SVG sanitised** with an element and attribute allowlist (no script, no external references, no
  event handlers) before hashing. It is still never served inline from the API origin (D3).
- **Processing is idempotent.** Processing an already-processed file returns the same bytes, so an
  exported KB seeds back to the same hashes (D7). This is a test, not an assumption: a re-encoding
  step that is not idempotent fails it.
- **Other files** (a revision's PDF, ADR-113 D6) are typed by sniffing and otherwise stored as they
  arrived.

Each rule has a refusal kind (`attachment_too_many_pixels`, `attachment_undecodable`,
`attachment_unsupported`) and a test.

**Storage quota.** A new per-tenant storage quota counts the bytes each KB holds and refuses an
upload that would exceed it (`tenant_quota_exceeded`).

### D4. The reference: org-attach links plus a map in the node

- **The source text** carries `[[attachment:NAME]]`, Org's own attachment link, so the file stays
  valid org.
- **The node's CRDT** gains an additive `attachments` map (ADR-093: a tolerant reader, no upcast on
  read). Each entry maps `NAME` to `{sha256, size, media_type}`.
- **The map is a root type of the node document**, not a key inside the `node` map. Every node that
  exists today has no such container, so a nested map could only be created lazily, which ADR-093 D4
  forbids for a good reason: two peers attaching the first image to the same node concurrently would
  each insert their own map at the key, and one of the two maps, with its image, would be dropped. A
  root type has no creating operation, so every peer's writes land in the same map, and a document
  without it reads as empty. Reading never creates it.
- **Renaming** a file changes a map key, not the content. Replacing a file binds the name to a new
  hash.
- Concurrent edits to different names merge cleanly. Two edits to the same name resolve last
  writer wins, the CRDT map's own rule. "Last" is yrs's deterministic order of the two writes by
  client id, not wall-clock time, so every peer picks the same winner whatever order the updates
  arrive in.

**A NAME is one portable file name**, because an export writes it as a file on Linux, macOS or
Windows (principle #13). It is 1 to 255 bytes of UTF-8, and at most 255 bytes in its NFD form too,
because APFS stores names decomposed (85 `é` fit, 86 do not, though both are under 255 bytes as
NFC). It is not `.` or `..`, and contains no `/`,
`\`, control character (Unicode Cc), format character (Unicode Cf: bidi controls, zero-width
characters), or any of `< > : " | ? *`. It does not end in a dot or a space, and it is not a Windows
device name, case-insensitively, with or without an extension: `CON`, `PRN`, `AUX`, `NUL`,
`COM1`–`COM9`, `LPT1`–`LPT9`, and, following Python's `ntpath.isreserved`, `CONIN$`, `CONOUT$` and the
superscript-digit ports `COM¹`–`COM³`, `LPT¹`–`LPT³`.

**No two NAMEs of one node may be one file.** Two NAMEs with the same canonical caseless key,
`NFD(toCasefold(NFD(NAME)))` written in NFC, using full default case folding (the C and F mappings
of `CaseFolding.txt`, not lowercase, and never the Turkic mappings, so no locale enters it and every
peer computes the same key), are taken as the same file on APFS and NTFS. That is the seed's
path-collision key too. `ſ` and `s`, `ς` and `σ`, `ß` and `ss` collide; `ı` and `i` do not, and `İ`
is `i` with a combining dot. A writer refuses to bind the
second, on set and on rename (a rename to another spelling of the same NAME is allowed). Two peers can
still bind such a pair concurrently, each write valid where it was made. The reader keeps both
rather than picking one, every peer converges on the same pair, and the export must refuse such a
node, naming both NAMEs, unless a defined disambiguation exists by then (P4; an `@ai-caution` marks
the place).

**There are two readers, failing in opposite directions.** The display reader is tolerant: an entry
with a bad NAME or shape is reported and left out, never repaired. The sweep's reader (D5) fails
safe: every string anywhere in any entry, under any key and at any depth, that is 64 hex digits once
ASCII whitespace is trimmed counts as referenced, whatever else about the entry is rejected, because
an entry an older daemon rejects may be one a newer peer still binds, and leaving it out would delete
its bytes. Its walk is bounded in depth and in values; a document past either bound, like a holder
that cannot be read at all, marks the KB's live set incomplete, and a sweep collects nothing in that
KB. For the same reason a rebirth copies the `attachments` root as stored, every key and value, and
only the set, rename and remove operations apply the NAME rule.

**The mapping is part of the node's content, everywhere content is fixed.** Replacing `diagram.png`
under the same name must never change what a revision, a history version or an export shows. So the
`attachments` map is a materialized field of the node:

- **`KbNodeDoc::version`** digests it. It destructures `MaterializedNode` without `..`, so a new
  field cannot be left out of the digest. A change of image is a change of version.
- **An ADR-113 revision** freezes it with the text (D2's frozen source covers the materialized
  fields). The revision's source hash, and with it the rendered PDF's declared source (ADR-113 D6),
  cover the names and hashes as well as the text. Two revisions with the same text and a replaced
  image therefore differ.
- **An ADR-106 history version** snapshots it.
- **The seed plan's `plan_hash` and an export's fingerprint** include it.

Images are fetched by KB and hash, never by name, so a cached copy can never be stale.

The daemon derives `blob_ref` rows from all of these (D5).

### D5. Collection: mark and sweep, with a grace period, under the binding lock

A periodic sweep runs on the maintenance tick, bounded per pass. For each KB:

- **The live set** is the union of every holder of a reference:
  - every working node's `attachments` map;
  - every node in the bin, until it is purged;
  - every ADR-106 history version of a node. That history is a record whether or not the KB uses
    publication review;
  - every ADR-113 revision, in every state, including rejected and withdrawn. A revision record is
    never deleted, so neither is what it references (ADR-113 D5, D7). That covers the attachments
    in its frozen source and its own attachments (`rendered_pdf`, `signed_pdf`, current or
    annulled). *(Amended by ADR-113 PR-6, its O2: the one release of a revision's attachment is
    ADR-113 B5's owner-only, audited deletion after the retention period, which releases the
    revision's hold on those bytes, never the record. Nothing else releases it.)*
  - anything under an audit or legal hold the operator has placed;
  - every blob uploaded within the grace period (default 24 hours, configurable), the window
    between an upload and the edit that references it.
- A `kb_blob` row outside the live set is dropped.
- A file is deleted when no `kb_blob` row names it.

Binding a reference and sweeping a KB take the same per-KB lock, so a sweep can never act on a stale
live set. The grace period only covers the gap between upload and reference.

Purging a node or deleting a KB releases its references. Its revisions' references survive, as
ADR-113 requires.

### D6. Rendering: the resolver says what happened

mae-org gains an attachment resolver beside its KB-link targets. For each `attachment:NAME` link, the
host supplies one of these states:

| State | Meaning |
|---|---|
| `Resolved { url, media_type, width?, height? }` | The image can be shown. |
| `Missing` | The name is not in the map, or the blob is gone. |
| `Forbidden` | The reader may not see it. |
| `TooLarge` | Over the cap. |
| `Unsupported` | Not an image type the page displays. |

Compose emits `<img>` only for a `Resolved` image, with width and height when known (so the page does
not jump while it loads), and the link's description or the name as `alt`. Every other state renders
a labelled placeholder (`<span class="org-attachment" data-state="…">NAME</span>`), never a broken
image. A load that fails after rendering is the browser client's to handle (its images ADR): swap in
the same placeholder, with a retry.

### D7. Seeding and export

- **Seeding (H).** Seeding reads, at the same revision, the attachment files that
  `[[attachment:…]]` links in selected pages refer to, under the node's ID-based attachment
  directory. It uploads them through the same processing (D3a) and fills each node's map with the
  processed hashes. The dry run runs the same processing code, so its `plan_hash` names the hashes
  the apply will store.
  - A link whose file is missing is a refusal, listed like the others.
  - A file over the cap is refused.
  - The dry run lists every attachment, with its size.
- **Export.** Export writes each node's attachments under its ID-based attachment directory, so the
  exported tree round-trips through seeding.

### D8. Scope limits of v1

- **Plaintext KBs only.** An E2E KB refuses attachments (`e2e_unsupported`) until client-side
  encryption is designed. Its address would be the hash of the ciphertext, and the server could not
  process what it cannot read (D3a): the client would have to.
- **Hub-local.** Mesh peers replicate the `attachments` map with the node, but not the bytes. A
  peer without a blob resolves it as `Missing`. Fetching it on demand from the hub is a later phase.
- **Display only, in MAE's own editor.** It shows the link, and the image when the blob is
  reachable. Uploading from MAE's editor is a later phase.

## Implementation plan: each phase must prove itself

| Phase | Builds | Proven by |
|---|---|---|
| P0 | This ADR; the schema; the request contract | Review |
| P1 | The blob store, the three routes, the storage quota, backup | The tests and benchmarks below |
| P2 | The `attachments` map in the node CRDT; `blob_ref` projection; the sweep | The sweep tests below |
| P3 | The mae-org resolver and placeholder states (a mae-org tag) | Render tests below |
| P4 | Seeding and export carrying attachments | A round trip: a repo with images, seeded, exported, then `diff -r` |
| P5 | Display in MAE's editor | An editor test |

### Tests that must exist (each fails without its fix, mutation-checked)

**Access:**
- A non-member's `GET` and `HEAD` get identical refusals for a blob this KB holds, one another KB
  holds, and one nobody holds.
- A member of KB A cannot read KB B's blob by its hash.
- An upload of bytes another KB holds is read and verified in full before binding (instrumented
  read count).
- A viewer cannot upload.

**Upload integrity and processing:**
- A digest mismatch is refused and leaves no file and no row.
- The stored image has no EXIF, XMP or location data; a rotated photo is stored upright; a
  colour profile survives.
- An image whose header declares more than the pixel limit is refused without being decoded.
- A malformed image fails within the decode limits and is refused.
- An SVG with a script, an external reference or an event handler is stored without it.
- The client cannot choose the address: the answer's sha256 is that of the stored bytes.
- Processing a processed file returns identical bytes, for every allowed format.
- The same bytes uploaded to two KBs produce two files and identical responses and timing
  classes; deleting one KB leaves the other's file.
- An upload over the cap stops reading at the cap. Peak memory stays constant for a body ten times
  the cap.
- An upload interrupted mid-stream, or by a daemon restart, leaves no file and no row.
- Concurrent identical uploads both succeed and leave one file.
- The quota refuses the upload that would exceed it, and not the one before.

**Collection:**
- A blob referenced only by a historical revision survives a sweep, and so does one referenced only
  by an old history version, one referenced only by a node in the bin, and one under a hold.
- A blob uploaded but not yet referenced survives within the grace period, and goes after it.
- A bind racing a sweep never loses a blob (deterministic interleavings, as in the race suite).
- Purging a node frees its blobs unless a revision holds them.

**Serving:**
- An SVG is served with `Content-Disposition: attachment` and the sandbox CSP.
- A non-image is never served with an image type.
- `ETag`/`If-None-Match` answers `304` without reading the file.

**Rendering:**
- Each resolver state renders its placeholder, and `<img>` appears only for `Resolved` image types.
- A link to an unknown name renders `Missing`, never a dangling `src`.

**Freezing:**
- Replacing an image under the same name changes the node's `version`. It leaves a frozen
  revision's content, source hash and PDF declared-source check unchanged. An approved page still
  shows the old image.
- Two seed plans differing only in one attachment's bytes have different `plan_hash`es.

**Round trip:**
- Seed, then export, then compare: identical, including the attachment trees.
- A missing referenced file refuses the seed.

### Performance: measured, budgeted, re-runnable

Benchmarks are `#[ignore]`d tests, re-runnable like ADR-109's `d4_bench`, and each reports its
numbers:
- **Upload:** throughput and peak RSS for 10 KB, 100 KB, 1 MB, the cap, and ten times the cap
  (refused). The budget: peak RSS does not grow with size.
- **Processing:** decode, strip and rotate time and peak memory per format (JPEG, PNG, WebP, an
  animated GIF), at 1, 12 and 40 megapixels. Those numbers set the time and memory limits.
- **Download:** p50 and p95 for 10 KB, 100 KB, 1 MB and 5 MB, plus the `304` path. The SQLite
  internal-versus-file crossover is re-measured here on the target hardware, and D1 revisited if
  the result contradicts it.
- **Sweep:** time per pass for 1,000 and 10,000 nodes with three attachments each, and the bound
  per maintenance tick.
- **Page:** composing a page with 50 attachments, the resolver's cost, and how many parallel fetches
  a browser client should allow.
- **Backup:** size and time with 1 GB of blobs, and an incremental second run.

Budgets for each are set from the first measurement and pinned in the benchmark, following ADR-109's
rule that a budget is set before launch.

### Failure behaviour: what each failure looks like to a user

| Failure | Daemon | What the reader sees |
|---|---|---|
| Not a member, or access revoked | `403` / `not_member` | `Forbidden` placeholder |
| Blob missing (never uploaded, or swept) | `404` / `blob_missing` | `Missing` placeholder |
| Over the cap | `413` / `quota_exceeded`, read stopped | The upload fails with the limit shown |
| Tenant storage full | `507` / `tenant_quota_exceeded` | The upload fails with the reason |
| Hash mismatch (a corrupt upload) | `400` / `blob_hash_mismatch` | The upload is retried once, then fails |
| Image over the pixel limit, or undecodable | `413` / `attachment_too_many_pixels`, or `422` / `attachment_undecodable` | The upload fails with the reason |
| Network failure or timeout during a download | — | Placeholder with a retry; the rest of the page unaffected |
| Image not decodable by the browser | `200` | Placeholder (the browser client's images ADR) |
| Daemon restart during an upload | Temporary file discarded | The upload is retried |

## Consequences

- A KB's size is no longer only its nodes. The ADR-109 budget, backup and the tenant quota all
  count blobs.
- The OAuth listener gains its first non-JSON-RPC data routes, and with them streaming, a new
  surface reviewed under principle #9.
- Node text stays small and searchable, and every client keeps its sync cost bounded.

## Amendment 1 (2026-11-24): accepted, with the link form, the layout and the build order pinned

The owner accepted this ADR on 2026-11-24, to put images in published pages: in the live site and in
the browser client's static documentation site alike. Pinning what both need before any page is
written:

### E1. One link form

`[[attachment:NAME]]` is the only form a KB uses for a file it carries: never `[[file:…]]` or a
relative path. It is what seeding carries into the store (D7), and what Emacs resolves natively. An
image is such a link **without a description**: Org's rule, which mae-org already applies to `http`
images. `[[attachment:x.png][text]]` is a link to the file, not an image. A caption is `#+CAPTION:` on
the line before. `alt` is the file name, as ox-html writes it.

### E2. The layout on disk, as Emacs's org-attach has it

From `org-attach.el` (Emacs 30): `org-attach-id-dir` is `"data/"`, relative to the Org file's directory,
and `org-attach-id-to-path-function-list` tries the UUID format (`<ID[0:2]>/<ID[2:]>`, for an ID longer
than two characters), then the timestamp format, then the fallback (`__/<ID[0]>/<ID>`). So a file lives
at `data/<ID[0:2]>/<ID[2:]>/NAME` next to the note, and an ID of two characters or fewer at
`data/__/<ID[0]>/<ID>/NAME`. The ID is the nearest enclosing entry's `:ID:`: the headline the link is
under, else up the outline to the file-level ID. MAE fixes `org-attach-use-inheritance` on, so an
image under a headline with no ID of its own belongs to the page.

Divergences, registered as such (ADR-112, in mae-org's `render-divergences.toml`): Emacs takes the
first function whose folder already **exists**, which depends on the filesystem, and with no folder
present at all its export writes the image as a file next to the Org file; MAE takes the first rule
that applies, always, so every peer derives the same path. `:DIR:` and `:ATTACH_DIR:` are not
supported in v1: as Org reads them, a `:DIR:` anywhere up the outline counts (it is read first, with
inheritance), and such a link has no `attach_dir`; seeding refuses a node that has either.

Which links are images follows Org's `file` link type, not only `file:`: `file+APP:` and a bare file
name are file links too (`file:` is matched case-insensitively, `attachment:` case-sensitively), and a
link with a `::search` part is never an image (Emacs's export fails on one). Built in mae-org 0.11.0;
an Emacs differential over 400 generated outlines (1,793 links) agreed with `org-attach-dir` on
every one.

### E3. mae-org's resolver contract (D6, made concrete)

The resolver is data, like `targets`, because it crosses the WASM boundary as JSON: `attachments`
maps a link target exactly as written (`"attachment:diagram.png"`, as the semantics expose it) to a
state: `resolved` (`url`, `media_type`, optional `width` and `height`), or `missing`, `forbidden`,
`too_large` and `unsupported`. Only `attachment:` and `file`-type links without a `::search` part are
looked up, and never a link inside another link's description. The rule is Org's:

- A resolved link **without a description** to an image extension renders `<img>`.
- Any other resolved link (one with a description, even to an image, or a description-less one to
  a non-image such as a PDF) renders `<a class="org-link" href="URL">`, showing the description as
  rendered, or the NAME when there is none, as ox-html links a file.
- Any other state renders the labelled placeholder
  `<span class="org-attachment" data-state="…">`, showing the same text.
- A target that is not in the map renders as today (`org-unresolved`).

The `http` images option is unchanged. The host reads a page's link targets from the semantics,
resolves them, then renders, and owns its policy (which types it shows or offers for download, size
caps). Built in mae-org 0.11.0; the differences from ox-html (the URL, the NAME where Org shows a disk
path, the placeholder, no image inside a link's description) are registered in
`render-divergences.toml`.

### E4. Build order

P3 (mae-org's resolver, a mae-org release) comes first. It is the first blocker for images anywhere,
and it lets the static site resolve images from the repository before the store exists. Then P1 (the
store and its routes; a live page's `<img src>` is `/kb/<kb-id>/blobs/<sha256>`, authorised like the
page), P2 and P4. P5 (MAE's editor) follows.

## As built: P1 (2026-11-24)

P1 (the store, its three routes, the storage quota, backup) departs from the text above in six
places. Each is recorded here with its reason rather than left for the code to imply.

### B1. AVIF is refused

D3 lists `image/avif` on the allowlist, and D3a requires a memory-safe decoder. Every AVIF decoder the
Rust image stack offers is C (dav1d, through libavif), and metadata cannot be stripped from a container
the daemon does not parse. So an AVIF upload is refused, `415 attachment_unsupported`, rather than
decoded by C or stored unprocessed. Revisit when a memory-safe decoder exists.

### B2. The quota is charged to the uploader, and nobody is outside a budget

D3a counts "the bytes each KB holds". A KB shared across tenants has no single tenant to charge, so
each row records the tenant whose principal uploaded it (`charged_to`, a column beyond D1's), and a
tenant's usage is the sum of its rows. The uploader is the one who chose to spend the space, and a KB
owner's quota cannot be exhausted by another tenant's member uploading into it. An upload deduplicated
within its KB adds no row and charges nothing.

A principal in no configured `[[tenant]]` is charged to one shared bucket,
`oauth.default_storage_bytes` (default 1 GiB, the same as `[tenant.quota] max_storage_bytes`; `0` is
an explicit opt-out, unlimited but still recorded). Before this, such a principal, and every principal
on a daemon with no `[[tenant]]`, had no storage limit at all. Under ADR-108's one process per tenant
no `[[tenant]]` is configured, so the bucket is that tenant's budget. Agents land in it too: every
self-issued token is an agent (ADR-098 C3), and an ADR-118 `agent:<id>` principal is in no tenant. The
bucket's label (`unassigned`) and a tenant's (`tenant:<name>`) cannot collide whatever an operator
names a tenant. Until ADR-118 maps an agent to its person, an agent of a person who is in a tenant is
charged to the shared bucket, not to that person's tenant: counted, but in the wrong place.

### B3. Backup copies every blob, every time

D1 expected an incremental copy. `mae-daemon backup create` instead snapshots `blobs.db` and copies
exactly the files its rows name, into the one archive, on every run. One archive stays self-contained
and checkable on its own (`backup verify`); an incremental mode needs a chain of archives and a restore
that walks it. Deferred until the backup benchmark, with real attachment volumes, says it matters.

### B4. Blobs are cached, an exception to the listener's `no-store`

Every other answer from the HTTPS listener is `no-store` (audit #588.3). A blob (`200`, and `304`) is
served as D3 says, `private, max-age=31536000, immutable`: its address is its content, so it can never
be stale. `private` keeps shared caches out; refusals on the blob routes are still `no-store`. The
consequence, accepted: a member removed from a KB keeps, on each device, what that device already
fetched, and can view it offline for up to a year. Removal stops new fetches only. They could equally
have saved what they had already seen.

### B5. Refusals: one 403 for everyone outside, 404 only for a reader

D2's "no oracle", made exact. Every caller the KB does not admit gets one answer, byte for byte:
`403 not_member`. That covers a non-member, an unbound identity-provider subject, a KB that does not
exist, a malformed KB ID, a KB not shared over the hub, and a collection that cannot be read. It is
decided before any blob is looked up. Only a reader is told `404 blob_missing`, for an address the KB
does not hold; only a member is told that their role cannot upload (`403 role_forbidden`) or that the
KB is end-to-end encrypted (`409 e2e_unsupported`).

### B6. Uploads are bounded in time, and processing in workers and queue

D3 bounded an upload's size, not its duration, and D3a's "bounded worker" had no queue policy. As
built:

- **Receiving.** `oauth.attachment_idle_timeout_secs` (default 30 s: a lossy link's TCP
  retransmission backoff, 15 s in all, fits inside it) bounds every wait for the next piece of the
  body, and `oauth.attachment_upload_deadline_secs` the whole body (unset: derived from the size cap
  at a 64 KiB/s floor, 160 s at the default cap). Past either, `408 upload_timeout`: the temporary
  file is deleted and nothing is charged. A daemon that crashes mid-upload leaves its temporary files
  under `blobs/.tmp`, and the store clears that directory when it opens at startup.
- **Workers.** `oauth.attachment_image_workers`, default 1: the slowest measured processing (a
  rotated 40-megapixel JPEG) peaks at +229 MiB, and ADR-109 D2 gives a tenant process 400 MB in all.
- **Queue.** A tenant has at most four uploads waiting for or holding a worker (a principal in no
  tenant counts alone, so the people of a one-tenant process do not starve one another), and a job
  waits at most 30 s for a worker. Past either, `503 attachment_busy` with `Retry-After: 10` (four
  worst-case jobs), nothing stored or charged.
- **Processing timeout.** Past 30 s an upload is refused, and its job is cancelled: one atomic claim
  decides between the worker committing and the request cancelling, so a refused upload never stores
  or charges anything afterwards. The worker, and the tenant's place in the queue, stay held until the
  worker actually returns, so a decode the daemon cannot interrupt still counts against both bounds.
- **SVG.** Beyond D3a's allowlist: an attribute holding a backslash is dropped (a CSS escape,
  `\75rl(`, is `url(` in disguise), as is one calling `image-set()`, `image()` or `src()`. A document
  is refused, `422 attachment_undecodable`, when its elements nest deeper than 256 (libxml2's default;
  deeper exhausts a worker's stack), when a `<use>` chain is deeper than 8, when a `<use>` instantiates
  its own ancestor, or when its `<use>`s expand to more than 200,000 elements (the parser's own node
  bound). The expansion bound, not a count of `<use>` elements, is what stops fan-out: sixty `<use>`s,
  ten per level over six levels, already expand to a million.

### B7. Residuals, accepted and stated

- **Principals in no tenant share one storage bucket.** One of them can fill it and refuse the others.
  That is deliberate: a shared bucket bounds the disk, while a budget per principal would not (every new
  principal would bring another gigabyte). Separating people is what `[tenant]` is for; the default
  budget is a safety net, not a tenancy.
- **Queue places are per principal**, so a person with several agent principals holds several places
  against the worker pool. This holds until ADR-118 maps an agent to its person. Then key the queue,
  and the storage charge, by the person.
- **The request head has no read timeout.** The listener bounds the TLS handshake and caps connections
  at accept, but a client that never finishes its headers holds a connection slot until the cap runs
  out. That predates this store and applies to every route on the listener, so it is tracked as an
  issue of its own, not fixed here.

## As built: P4 (2026-11-25)

P4 (seeding, export and the read path) follows D7 and E1/E2. What it carries, and where it departs:

### B8. What round-trips

`kb seed`, then `kb export`, then `kb seed` again binds the same NAME → sha256 map on every node: the
export writes the bytes the store holds at the directory the page's text names (E2), and processing is
idempotent, so the second seed stores the same hashes. Pages come back as their normal form N(F). A
PDF, any non-image file and an image that carried no metadata come back byte for byte. **An image with
metadata does not**: what the seed stored, and so what the export writes, has its EXIF, GPS, XMP and
comments stripped and an orientation tag applied to the pixels (a rotated JPEG is re-encoded, D3a). A
repository that commits the export therefore changes such a photo once, and never again. Tested in
`daemon/src/seed/tests/attach_tests.rs` with a generated JPEG carrying EXIF, GPS and orientation 6.

### B9. The seed's refusals, each named

A link is refused, at its page, when its NAME fails the NAME rule; when no `:ID:` is up the outline or
a `:DIR:`/`:ATTACH_DIR:` is (`attachment_dir`); when the directory built from an ID is not a plain
relative path, an ID such as `../../x` (`attachment_path`); when two links of the page are one NAME
once case and normalization are ignored but name different files (`attachment_collision`); when the
commit has no file there (`attachment_missing`), or a symlink on the way, which is never followed
(`attachment_symlink`); when a checkout attribute (an LFS `filter` above all) is set on the path;
when the file is over `max_attachment_bytes`, judged from its size before it is read
(`attachment_too_large`); and when processing refuses it (too many pixels, undecodable, AVIF). The
dry run lists every refusal; an apply of a plan with any writes nothing. The files are read from the
commit's object store, never a working copy, each checked to hash to its name.

### B10. The NAME rule refuses `%`

Added to D4's rule after the Windows-forbidden characters: "contains a percent sign". A link target
or URL may be percent-decoded on its way (`a%20b.png` read as `a b.png`), so one NAME could name two
files, or a different one in a client than in the store. The rule in check order, with examples and
the fold pairs `fold_key` gives (ß/ss, ẞ, ﬁ, ς/σ, a Cherokee pair, İ and ı), is
`docs/verification/fixtures/kb-attach/name_rule.json`, generated from the code by a test that fails
when the file drifts.

### B11. The read shape

`kb/query.get`, `kb/query.list`, `kb/history.get` and the revision read report a node's attachments
as a list in NAME order, each `{name, sha256, size, media_type, url}`, with `url` the blob route
`/kb/<kb-id>/blobs/<sha256>` (D3), which authorizes the reader again on every request. A reader who
is not a member gets the refusal it always got, naming no hash. The live site (ADR-119) resolves each
page's `[[attachment:NAME]]` links itself (E3): resolved, with the image's size, when the KB's store
holds the blob; `missing` when the node binds no such NAME or the store lacks the blob. Example:
`docs/verification/fixtures/kb-attach/query_get_attachments.json`.

### B12. Residuals, stated

- **An update's dry run cannot list the fields of an edit to a node with attachments.** It rebuilds
  what the seed wrote from the recorded page blob, which does not hold the attachment map, so the
  rebuilt version differs from the recorded one and the edit is reported as "fields unknown". The
  edit is still found and still needs `--discard-edits`. Fixing it means recording the map with the
  seed row.
- **The dry run processes every image** (to name its hash in `plan_hash`), so a repository of many
  large photos makes a slow dry run; the apply processes them again.

## Rejected

- **Base64 in node text**, for the measured reasons in Context.
- **The ADR-034 artifact cache**, which holds derived, recomputable data under a different lifecycle.
- **Blob bytes inside SQLite**, which the SQLite measurement puts on the slow side for images. It
  stays open if P1's re-measurement says otherwise.
- **Cross-KB deduplication**, which saves disk at the cost of revealing, by timing or response,
  that another KB holds the same bytes.
- **Client-chosen addresses** (`PUT …/blobs/<sha256>`), which cannot work once the server
  processes images before hashing.
- **The hash as the reference in the text** (`[[blob:<sha256>]]`), which breaks org-attach
  compatibility and makes a renamed file look like a new one.
- **Tokens in image URLs**, which Matrix deprecated as unsafe.
