# ADR-116 prior art: node version history

Date: 2026-11-11. Each item records what was checked and what it decides. Where a source did not
say something, that is stated rather than inferred.

## SharePoint: version limits, retention and holds

<https://learn.microsoft.com/en-us/sharepoint/document-library-version-history-limits>.

**Limits.**
- Version limits are set at organization, site or library level, and narrower settings override
  broader ones.
- The manual setting combines a count limit with an optional expiration. For example, "500 major
  versions with a 365-day expiration".
- The UI refuses "less than 100 major versions or less than 30 days", because lower values "can
  result in the user activity causing an inadvertent data loss".

**Retention and holds.**
- "For items that are subject to a retention policy (or an eDiscovery hold), the versioning limits
  for the document library are ignored."
- When a trim job meets such a version, it "will stamp an expiration date on the version instead of
  deleting it".
- "Version deletion on documents marked as records is blocked."
- Audit events are logged for changes to limits, for user deletion of versions, and for "versions
  deleted when expired or bulk deleted".

**Decides:**
- retention is an explicit policy, set at KB scope;
- a floor exists below which a policy cannot go;
- holds and records override the policy;
- pruning and policy changes are themselves audited.

## Certificate Transparency (RFC 6962): tamper evidence needs a published head

<https://www.rfc-editor.org/rfc/rfc6962>: "The append-only property of each log is technically
achieved using Merkle Trees". "The log SHOULD sign the corresponding tree hash and tree
information". If a log presents "two different, conflicting views of the Merkle Tree at different
times and/or to different parties", this "can be efficiently detected", and "Merkle consistency
proofs prove the append-only property of the tree."

**Decides:**
- each node's history is a hash chain;
- the daemon signs the KB's history head;
- verification checks a head kept somewhere else.

A hash an attacker can recompute proves nothing. A chain proves order and integrity only relative
to a head the attacker could not also rewrite.

## MediaWiki: rollback is a new revision

<https://www.mediawiki.org/wiki/Manual:Administrators>: a rollback "will be marked as a minor edit
and given an automatic edit summary". The page does not say outright whether the rollback is
logged, and it says nothing about removing history.

**Decides:** a rollback is a new version that records what it restored. It never deletes or
rewrites the versions after its target.

## Google Docs: versions group edits, and named versions are kept apart

<https://support.google.com/docs/answer/190843> (and secondary guides): naming a version keeps it
from being merged with others, and "Only show named versions" filters the list. None of the sources
checked state the rule for grouping unnamed edits automatically.

**Decides:** a version is a checkpoint over many fine-grained CRDT updates, not one per update. The
checkpoint rules are MAE's own and stated in the ADR. A named version is a distinct, retained kind.
