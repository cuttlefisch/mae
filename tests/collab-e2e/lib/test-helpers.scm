;;; test-helpers.scm — Collab-specific test helpers for MAE E2E tests
;;;
;;; Provides async predicates for common collab workflow patterns.
;;; Requires mae-test.scm to be loaded first (handled by --test CLI).

;; (wait-connected TIMEOUT-MS) — wait until collab status is "connected" or "synced".
(define (wait-connected timeout-ms)
  (wait-until
    (lambda ()
      (let ((status (collab-status)))
        (let ((s (cadr (car status))))  ; status field value
          (or (string=? s "connected")
              (string=? s "synced")))))
    timeout-ms))

;; (wait-for-content BUFFER-NAME SUBSTRING TIMEOUT-MS)
;; — wait until the named buffer contains SUBSTRING.
(define (wait-for-content buffer-name substring timeout-ms)
  (wait-until
    (lambda ()
      (let ((text (buffer-text buffer-name)))
        (and (string? text)
             (string-contains? text substring))))
    timeout-ms))

;; (wait-synced BUFFER-NAME TIMEOUT-MS) — wait until buffer is in synced-buffers list.
(define (wait-synced buffer-name timeout-ms)
  (wait-until
    (lambda ()
      (let ((synced (collab-synced-buffers)))
        (member buffer-name synced)))
    timeout-ms))
