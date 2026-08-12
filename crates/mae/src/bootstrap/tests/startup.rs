//! Bootstrap tests: recent-file history lists and the daemon-connection handshake.

use super::super::*;

#[test]
fn parse_history_lists_round_trips_escaped_paths() {
    let mut files = mae_core::RecentFiles::new(100);
    files.push(PathBuf::from("/home/user/say \"hi\".txt"));
    files.push(PathBuf::from(r"C:\weird\backslash\path.txt"));
    let mut projects = mae_core::RecentProjects::new(20);
    projects.push(PathBuf::from("/home/user/proj a"));

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("history.scm");
    std::fs::write(
        &path,
        format!(
            ";; MAE generated history file. Do not edit by hand.\n\n{}",
            {
                let mut s = String::new();
                for f in files.list().iter().rev() {
                    s.push_str(&format!(
                        "(recent-files-add! \"{}\")\n",
                        f.to_string_lossy()
                            .replace('\\', "\\\\")
                            .replace('"', "\\\"")
                    ));
                }
                for p in projects.list().iter().rev() {
                    s.push_str(&format!(
                        "(recent-projects-add! \"{}\")\n",
                        p.to_string_lossy()
                            .replace('\\', "\\\\")
                            .replace('"', "\\\"")
                    ));
                }
                s
            }
        ),
    )
    .unwrap();

    let (parsed_files, parsed_projects) = parse_history_lists(&path);
    // File-order (oldest -> newest) is the reverse of MRU `.list()` order.
    let expected_files: Vec<PathBuf> = files.list().iter().rev().cloned().collect();
    let expected_projects: Vec<PathBuf> = projects.list().iter().rev().cloned().collect();
    assert_eq!(parsed_files, expected_files);
    assert_eq!(parsed_projects, expected_projects);
}

/// Adversarial case for the exit-time merge: a naive "just serialize the
/// session's own list" implementation would silently drop `old2` (added
/// by a different, concurrently-running `mae` process) because this
/// session's in-memory list never saw it. `merge_history_lists` must
/// preserve it — the session's recency wins for anything it touched,
/// but disk-only entries survive as "older", not vanish.
#[test]
fn merge_history_lists_preserves_disk_only_entries() {
    let disk_files = vec![PathBuf::from("/old1"), PathBuf::from("/old2")];
    let mut session_files = mae_core::RecentFiles::new(100);
    session_files.push(PathBuf::from("/old1")); // re-touched this session
    session_files.push(PathBuf::from("/new1"));

    let (merged, _) = merge_history_lists(
        disk_files,
        vec![],
        &session_files,
        &mae_core::RecentProjects::new(20),
    );

    let result: Vec<PathBuf> = merged.list().iter().cloned().collect();
    assert_eq!(
        result,
        vec![
            PathBuf::from("/new1"),
            PathBuf::from("/old1"),
            PathBuf::from("/old2"),
        ],
        "session recency wins for touched entries; disk-only entries survive as older, not dropped"
    );
}

/// #323: `daemon_version_skew` (main.rs) was already implemented, unit-tested,
/// and — this is the load-bearing part — already CALLED here on every daemon
/// connect (not just when the user thinks to run `:collab-doctor`). But the
/// call site only reached `tracing::warn!`, a log line invisible during normal
/// use — exactly the "daemon silently serves stale data with nothing surfacing
/// it" gap #323 reports. Real fake daemon socket (not a synthetic call to
/// daemon_version_skew in isolation) speaking real Content-Length JSON-RPC,
/// responding to `daemon/status` with a version that differs from this
/// editor's own — asserts the mismatch reaches the durable notification bus.
#[test]
#[cfg(unix)]
fn init_daemon_connection_notifies_on_a_real_version_mismatch() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixListener;

    let tmp = tempfile::tempdir().unwrap();
    let socket_path = tmp.path().join("mae-daemon-fake.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();

    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;

        // Read one Content-Length-framed JSON-RPC request (real framing, the
        // same the daemon's own server side speaks).
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length: ") {
                content_length = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).unwrap();
        let req: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(req["method"], "daemon/status");

        // Respond with a version that DIFFERS from this editor's own build.
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": req["id"],
            "result": { "version": "0.0.1-fake-old-daemon", "primary_exists": false },
        });
        let resp_body = serde_json::to_vec(&resp).unwrap();
        write!(writer, "Content-Length: {}\r\n\r\n", resp_body.len()).unwrap();
        writer.write_all(&resp_body).unwrap();
        writer.flush().unwrap();
    });

    let mut editor = mae_core::Editor::new();
    editor.kb.daemon_enabled = true;
    editor.kb.daemon_socket = socket_path;

    init_daemon_connection(&mut editor);
    server.join().unwrap();

    let notes = editor.notifications.active_sorted();
    let hit = notes
        .iter()
        .find(|n| n.source == "daemon" && n.title.contains("version differs"));
    assert!(
        hit.is_some(),
        "a durable notification must be raised for a real daemon version \
         mismatch, not just a tracing log line; got: {:?}",
        notes.iter().map(|n| &n.title).collect::<Vec<_>>()
    );
    assert_eq!(
        hit.unwrap().severity,
        mae_core::notifications::Severity::Warning
    );
    assert!(
        hit.unwrap()
            .body
            .as_ref()
            .is_some_and(|b| b.contains("0.0.1-fake-old-daemon")),
        "the notification body must name the mismatched daemon version"
    );
}
