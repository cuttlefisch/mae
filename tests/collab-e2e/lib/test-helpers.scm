;;; test-helpers.scm — Collab-specific test helpers for MAE E2E tests
;;;
;;; Provides async predicates for common collab workflow patterns.
;;; All wait-* functions use sleep-ms internally, which yields to the
;;; event loop — collab events are drained during every poll cycle.
;;;
;;; Requires mae-test.scm to be loaded first (handled by --test CLI).

;; (wait-connected TIMEOUT-MS) — wait until collab status is "connected" or "synced".
(define (wait-connected timeout-ms)
  (wait-until
    (lambda ()
      (let ((status (collab-status)))
        (and (pair? status)
             (pair? (car status))
             (let ((s (cadr (car status))))
               (or (string=? s "connected")
                   (string=? s "synced"))))))
    timeout-ms))

;; (wait-for-content BUFFER-NAME SUBSTRING TIMEOUT-MS)
;; — wait until the named buffer contains SUBSTRING.
;; This is the PRIMARY sync barrier for CRDT convergence testing.
;; It polls every 50ms (via wait-until → sleep-ms), draining collab
;; events on each cycle, so CRDT updates are applied between polls.
(define (wait-for-content buffer-name substring timeout-ms)
  (wait-until
    (lambda ()
      (let ((text (buffer-text buffer-name)))
        (and (string? text)
             (string-contains? text substring))))
    timeout-ms))

;; (wait-content-absent BUFFER-NAME SUBSTRING TIMEOUT-MS)
;; — wait until the named buffer does NOT contain SUBSTRING.
;; Used after undo operations to confirm CRDT propagation of removals.
(define (wait-content-absent buffer-name substring timeout-ms)
  (wait-until
    (lambda ()
      (let ((text (buffer-text buffer-name)))
        (and (string? text)
             (not (string-contains? text substring)))))
    timeout-ms))

;; (wait-synced BUFFER-NAME TIMEOUT-MS) — wait until the server has confirmed
;; the share/join for this buffer. Uses collab-confirmed-shares which is only
;; populated after BufferShared/BufferJoined events from the server, NOT on
;; optimistic intent drain. This ensures the server has the document before
;; the test proceeds.
(define (wait-synced buffer-name timeout-ms)
  (wait-until
    (lambda ()
      (let ((confirmed (collab-confirmed-shares)))
        ;; Check both exact match and suffix match (doc IDs include project prefix).
        (or (member buffer-name confirmed)
            (let loop ((lst confirmed))
              (cond
                ((null? lst) #f)
                ((string-contains? (car lst) buffer-name) #t)
                (else (loop (cdr lst))))))))
    timeout-ms))

;; (wait-for-file PATH TIMEOUT-MS) — wait until PATH exists on disk.
;;
;; For handing control back to the driving SHELL SCRIPT. A scenario that ends
;; with a fixed `(sleep-ms N)` is asserting that N ms is enough for whatever the
;; script still needs the editor alive for — and once the editor exits, nothing
;; the script waits for afterwards can still happen, so a longer wait on the
;; script side cannot rescue it. Let the script signal instead, bounded so a
;; signal that never comes fails rather than hangs.
(define (wait-for-file path timeout-ms)
  (wait-until
    (lambda () (file-exists? path))
    timeout-ms))

;; (wait-buffer-exists BUFFER-NAME TIMEOUT-MS) — wait until buffer exists.
(define (wait-buffer-exists buffer-name timeout-ms)
  (wait-until
    (lambda ()
      (get-buffer-by-name buffer-name))
    timeout-ms))
