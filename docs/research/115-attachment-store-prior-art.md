# ADR-115 prior art: attachments for KB nodes

Date: 2026-11-10. Each item records what was checked and what it decides. Where a source did not
say something, that is stated rather than inferred.

## SQLite: internal versus external BLOBs

<https://www.sqlite.org/intern-v-extern-blob.html> measures reading BLOBs from the database against
reading them from separate files. The ratio is external time over internal time, so above 1.0
means internal is faster:

| page size | 10 KB | 50 KB | 100 KB | 500 KB | 1 MB |
|---|---|---|---|---|---|
| 1024 | 1.535 | 0.608 | 0.456 | 0.247 | 0.233 |
| 8192 | 2.240 | 1.334 | 1.035 | 0.625 | 0.720 |
| 16384 | 2.439 | 1.292 | 1.023 | 0.820 | 0.598 |

Below about 100 KB, the database is faster; above it, files are. The page itself warns that the
numbers come from 2011 hardware and SQLite 3.7.8, and should be re-measured on the target system.

**Decides:** images are typically above 100 KB, so their bytes go in files, while their
metadata goes in SQLite. The threshold is re-measured in phase P1 rather than assumed.

## Matrix: authenticated media (MSC3916)

<https://github.com/matrix-org/matrix-spec-proposals/blob/main/proposals/3916-authentication-for-media.md>

Before MSC3916, Matrix media was unauthenticated, and the proposal names the problems that caused:
- "The only protection for media is the obscurity of the URL", and URLs leak through sharing and
  access logs;
- anyone could make a homeserver copy media into its store;
- redacted media stayed visible, and could not be deleted.

The fix requires an `Authorization: Bearer` header on downloads. The proposal notes that this is
"particularly annoying for web clients" and suggests service workers or locally cached `blob:` URIs,
warning that such caching "can lead to significant memory usage, particularly for large media". It
also says clients "SHOULD NOT use the deprecated ?access_token" query parameter, because it is
"generally unsafe".

**Decides:**
- knowing a blob's hash is never authorization;
- downloads carry the bearer header, and the browser displays the result from a `blob:` URL;
- tokens never go in URLs;
- the client bounds its own cache.

## git gc: the grace period before pruning

<https://git-scm.com/docs/git-gc>: `gc.pruneExpire` defaults to two weeks. The reason given is that
when gc "runs concurrently with another process, there is a risk of it deleting an object that the
other process is using but hasn't created a reference to." Git keeps any object newer than the grace
period, and admits this falls "short of a complete solution".

**Decides:** a blob that was uploaded but is not yet referenced by a node is kept for a grace period.
On top of that, the sweep's mark and its delete are taken under the same per-KB lock as binding a
reference, which closes the race git leaves open, because MAE owns both sides of it.

## org-attach

<https://orgmode.org/manual/Attachments.html> says Org names attachment directories "by the 'ID'
property of a heading or by a 'DIR' property". The manual's attachment-link page
(<https://orgmode.org/manual/Attachment-links.html>) shows `[[attachment:info.org]]` for a file
attached "using the unique 'ID' of the heading". Neither page spells out how the path is resolved;
in Emacs the function `org-attach-expand` resolves it against the attachment directory.

**Decides:** the source keeps the org-attach link form `[[attachment:NAME]]`, so a file stays valid
org. An exported KB writes each node's attachments under a directory derived from the node's `:ID:`,
so that Emacs's ID-based attachment directories can open them.
