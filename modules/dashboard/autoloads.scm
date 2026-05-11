;;; dashboard/autoloads.scm — Dashboard module autoloads
;;;
;;; Registers the dashboard command. The splash screen itself is rendered
;;; by the kernel (Rust) — this module just provides the command binding
;;; and option registration.

(define-command "dashboard" "Show the dashboard/splash screen" "mae-dashboard-show")

(define (mae-dashboard-show)
  (run-command "dashboard"))

(provide-feature "dashboard-autoloads")
