# ADR-120 prior art: the daemon's activity record

Date: 2026-11-25. Eight web searches and three page fetches. Each item records what was checked,
what it says, and what it changed in ADR-120. The brief was to refute the draft design, not to
confirm it. Where a source did not answer a question, that is stated rather than inferred. Several
sources are secondary summaries of a standard whose text is not freely published. Those are marked,
and nothing in the ADR rests on their exact wording.

## What the draft assumed before reading

Before reading, the draft had five positions:
1. a bounded queue that counts dropped events and raises an alarm;
2. a signature on every event;
3. one hash chain covering every event, with old rows deleted at the end of retention;
4. the IdP's `sid` claim as the key that joins the daemon's record to the browser client's backend;
5. forward-secure sealing as the way to resist a stolen key.

The sources changed four of the five. Only the signature question stayed open long enough to settle
on cost grounds.

## Linux audit (auditd): what "drop and count" means in practice

- Red Hat knowledge base, "auditd reports 'Error receiving audit netlink packet (No buffer space
  available)' and/or system reboots on its own": <https://access.redhat.com/solutions/7058400>
  (summary only; the full article needs a subscription).
- linux-audit list, "What constitutes -f failure?":
  <https://listman.redhat.com/archives/linux-audit/2013-October/008163.html>.
- CIS benchmark item "Ensure system is disabled when audit logs are full":
  <https://www.tenable.com/audits/items/CIS_Debian_Linux_12_v1.0.1_L2_Server.audit:22f81125ca3d56026eab708c0c2d294e>.

**What they say.**
- The kernel's `-f` flag sets how audit handles a critical error such as a backlog overflow. Its
  default is 1, which logs the error and drops the record. `-f 2` panics the host.
- The Red Hat case shows the default in production: a kernel counter of
  `audit_lost=126533574` against `audit_backlog_limit=65536`. More than 126 million records were lost
  and counted.
- A list post reports that `-b 8192 -f 2` "caused a kernel panic very quickly", and a reboot.
- `disk_full_action` is a separate setting, in user space, for a full log partition. CIS asks for
  `halt` or `single`.

**Decides.** "Count drops and alarm" was the draft's position. auditd's default does exactly that,
and the counter only proves the gap afterwards: it does not close it. The requirement says a dropped
read is a gap in the record, so counting does not meet it. Halting the host is the other extreme, and
it turns a slow disk into an outage for every user. ADR-120 D5 takes the middle course: a request
whose event cannot be recorded is refused, and only that request. That is AU-5(4)'s "partial
shutdown" (below), applied one request at a time.

## NIST SP 800-53 Rev. 5: AU-5 and AU-9

- AU-5 as catalogued by csf.tools: <https://csf.tools/reference/nist-sp-800-53/r5/au/au-5/>.
- AU-9 summary: <https://www.upguard.com/compliance/nist-sp-800-53/au/au-9> (secondary).
- FedRAMP's AU family page: <https://fedramp.gov/2026/reference/controls/audit-and-accountability/>.

**What they say.**
- **AU-5.** On a logging failure: "Alert [organization-defined personnel or roles] within
  [organization-defined time period]" and "Take the following additional actions". The guidance
  gives "overwriting oldest audit records, shutting down the system, and stopping the generation of
  audit records" as examples.
- **AU-5(4)**, *Shutdown on Failure*, offers full shutdown, partial shutdown, or a degraded
  operational mode, when no alternate audit logging capability exists.
- **AU-5(5)** asks for an alternate audit logging capability.
- **AU-9** protects audit information from unauthorized access, modification and deletion.
- **AU-9(3)** asks for cryptographic protection of its integrity.

**Decides.**
- The control family names "stopping the generation of audit records" as an acceptable response. A
  framework that requires a complete record of reads rules that response out, so the deployment's
  own requirement is stricter than the generic control. ADR-120 follows the requirement.
- Refusing service is a degraded operational mode in AU-5(4)'s sense. MAE builds no alternate
  capability (AU-5(5)) in this ADR: a second store would be a second record, against principle #8.
- An alert is required. D5 wires one (a metric and a health check that fails).
- AU-9(3) is the generic reason for a hash chain and a signature rather than access control alone.

## ISO/IEC 27001:2022, Annex A 8.15

- <https://www.isms.online/iso-27001/annex-a-2022/8-15-logging/> and
  <https://www.upguard.com/compliance/iso-27001/8-15> (secondary; the standard's text is not free).

**What they say.** 8.15 asks that logs recording activities, exceptions, faults and other relevant
events be "produced, stored, protected, and analysed". The 2013 predecessor, A.12.4.2, read:
"Logging facilities and log information shall be protected against tampering and unauthorized
access". The vendors list hashing, append-only and read-only recording as protections. One claims a
one-year minimum retention, which could not be confirmed from the standard.

**Decides.** "Analysed" means someone reads the record, so the export filters and the auditor's
read (D8) are part of the requirement, not extras. The one-year claim is not used: ADR-120 uses the
deployment's own retention figures, as the requirement states them.

## AWS CloudTrail: digest files, a signed chain at an interval

- <https://docs.aws.amazon.com/awscloudtrail/latest/userguide/cloudtrail-log-file-validation-intro.html>
- <https://docs.aws.amazon.com/awscloudtrail/latest/userguide/cloudtrail-log-file-validation-cli.html>
  (fetched).

**What they say.**
- Every hour CloudTrail delivers a digest file that names the log files of that hour with their
  hashes. The digest is signed, and "each digest file carries a link to its predecessor".
- `validate-logs` starts from the most recent digest and walks back "until the specified value for
  `start-time` is reached, or until the digest chain ends".
- "If a digest file is missing or not valid, the time range that cannot be validated is indicated in
  the output."
- "A starting digest file will also be generated when log file integrity validation is restarted".
- `validate-logs` checks "only the log files that are referenced in their corresponding digest
  files".
- Log files that have been downloaded to local disk cannot be validated with the CLI.

**Decides.**
- **One signature per interval, not per event, has production precedent.** CloudTrail signs once an
  hour, and the events after the last digest are covered by nothing until the next one. ADR-120 D7
  signs a head every `head_interval` (10 s by default) and accepts that short unsigned tail.
- **Verification starts from a stated point.** CloudTrail validates from a requested start time, and
  a restart begins a new starting digest. That supports D9's anchors: after a disposal, verification
  starts from a signed statement of what was disposed, not from the first event ever written.
- **A gap is reported as a range, not hidden.** D9's verifier does the same.
- **CloudTrail cannot validate local copies.** ADR-120 does better on this point: the export carries
  everything needed to verify it offline, as ADR-113 E10's export does.

## systemd journal: Forward Secure Sealing

- The LWN discussion of FSS's introduction, <https://lwn.net/Articles/513080/> and
  <https://lwn.net/Articles/513198/> (via search excerpts), and journald.conf(5),
  <https://man.archlinux.org/man/journald.conf.5.txt>.
- Dörre and Ottenhues, *Security analysis of the implementation of forward secure log sealing in
  journald*, IACR ePrint 2023/867: <https://eprint.iacr.org/2023/867> (abstract fetched).

**What they say.**
- FSS evolves a sealing key at intervals and keeps the verification key off the host. A key stolen
  later cannot re-seal the past.
- From the LWN discussion: an attacker "can delete it, or forge messages after the compromise, but
  can't go back in time". Deletion is not prevented.
- The 2023 analysis "discovered attacks on the log sealing in journald". One "allows to forge
  arbitrary logs for past entries without the validation tool noticing any problem", which
  "completely breaks the security guarantee of log sealing". Fixes for the two serious findings were
  merged in systemd 255.

**Decides.** The draft considered forward-secure sealing as protection against a stolen daemon key.
The most widely deployed implementation shipped for about a decade with a flaw that broke its one
guarantee, and it never protected against deletion. ADR-120 does not build its own forward-secure
scheme. It relies on standard Ed25519 over a hash chain (as ADR-113 E10 and ADR-116 D4 already do),
and on a head kept off the host for the stolen-key case. FSS stays rejected unless a vetted library
implements it.

## PostgreSQL pgaudit

- EnterpriseDB's pgaudit page:
  <https://www.enterprisedb.com/docs/supported-open-source/warehousepg/extensions/additional/pgaudit/>.
- pgEdge's caveats page: <https://docs.pgedge.com/pgaudit/v19beta3/caveats/>.
- Tom Lane on pgsql-hackers: <https://postgrespro.com/list/id/24636.1399216377@sss.pgh.pa.us>.

**What they say.**
- pgaudit writes through the server's ordinary log facility, and inherits its destination and its
  rotation. Its settings can be changed only by a superuser, who can therefore turn it off.
- pgEdge: "it is not possible to reliably audit superusers with pgAudit". It adds that entries can
  be lost if the log destination becomes unavailable after a transaction commits but before its
  audit entries are durably written.
- Tom Lane: prohibiting actions to the superuser "is a fundamentally flawed concept". Their actions
  "should be logged up until they disable auditing and hopefully those logs would be sent somewhere
  that they're unable to destroy".

**Decides.**
- **The operator is the trust anchor, and the record says so.** The daemon's operator can stop the
  daemon and edit its files, as a superuser can. ADR-120 does not claim otherwise. D7 makes tampering
  evident, and D11 records turning the record off.
- **Ship something off the host.** For ADR-120 that is the signed head, which is small.
- **pgaudit's loss window is the one D5 closes.** pgaudit's entry can be lost after the transaction
  commits. In ADR-120 a read is not answered until its event has committed. A write whose effect
  commits first is reconciled from its own transactional record (D6).

## Certificate Transparency: append-only logs, and how they end

- Chrome CT log policy: <https://googlechrome.github.io/CertificateTransparency/log_policy.html>;
  log states: <https://googlechrome.github.io/CertificateTransparency/log_states.html>.
- DigiCert, *Scaling Certificate Transparency Logs: Temporal Sharding*:
  <https://www.digicert.com/blog/scaling-certificate-transparency-logs-temporal-sharding>.
- Chromium ct-policy thread, "Proposal to require all new CT Logs be sharded annually":
  <https://groups.google.com/a/chromium.org/g/ct-policy/c/J3-K5-xPMLY>.
- RFC 6962 (already cited for ADR-116): <https://www.rfc-editor.org/rfc/rfc6962>.

**What they say.**
- A CT log never deletes an entry. Size and lifetime are handled by **temporal sharding**: each log
  accepts only certificates expiring in its range, `[rangeBegin, rangeEnd)`. Operators converged on
  annual shards.
- A shard past its range moves to ReadOnly, then to Rejected, and is retired whole.

**Decides.** This was the strongest alternative to D9's prefix disposal. It would mean one chain per
class per month, a whole shard dropped once its newest event is past retention, and an index chain
naming each shard's final head. ADR-120 rejects it:
- retention becomes as coarse as the shard, up to a month longer than the documented period;
- the number of heads to keep and verify multiplies;
- prefix disposal with a signed anchor gives the same verifiability at the granularity of a day.

CT's lesson that is kept: the record never rewrites or deletes from the middle. It only drops a whole
prefix, and says so in a signed statement.

## OpenID Connect `sid`

- Back-Channel Logout 1.0: <https://openid.net/specs/openid-connect-backchannel-1_0-07.txt>.
- OpenID Connect issue tracker, "`sid` is missing in core spec":
  <https://bitbucket.org/openid/connect/issues/1811/sid-is-missing-in-core-spec>, and "sid ID Token
  claim - definition and when to include":
  <https://bitbucket.org/openid/connect/issues/1031/sid-id-token-claim-definition-and-when-to>.

**What they say.**
- In Back-Channel Logout the `sid` claim is OPTIONAL: if supported, it "is also included in ID
  Tokens issued by the OP".
- OIDC Core does not define `sid`. The tracker issue notes that the logout specs do not say which
  tokens must carry it.
- No source found says whether an ID token from a refresh grant carries `sid`. A forum report has
  `sid` missing from an `id_token_hint` while a fresh login produced one.

**Decides.** The IdP's `sid` cannot be the join key between the daemon's record and the browser
client backend's record: it is optional, it is defined only by the logout specs, and it is
unspecified on refresh. The deployment's own IdP may omit it from a refreshed ID token. ADR-120 D4
mints the daemon's own session id. It returns that id in a response header so the relaying server can
log it, and it records `sid` as an extra field when present. The join therefore works whether or not
the IdP sends `sid`.

## Not consulted, and why

- **Schneier and Kelsey (1999) and Crosby and Wallach (2009)**, the academic tamper-evident log
  designs: the search budget went to deployed systems, whose failures (journald, pgaudit, auditd's
  lost counter) were more decisive than an idealised design. Crosby and Wallach's history tree, a
  Merkle structure with inclusion proofs, is the known answer to a filtered export too large to ship
  with its stubs. ADR-120 lists it as an open item (O4) rather than adopting it untested.
- **The deployment's own framework text** is not named or quoted, by the brief's rule.
