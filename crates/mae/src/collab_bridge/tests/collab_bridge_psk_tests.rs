//! Split from the monolithic `collab_bridge_tests.rs`: PSK wiring — CI-runnable, no network required: auth handshake, channel setup precedence, credential resolution, peer discovery.

use super::*;

#[tokio::test]
async fn perform_psk_auth_correct_key_succeeds() {
    // Test perform_psk_auth against a real PskAuth server handshake
    // using tokio duplex streams (no TCP needed).
    use mae_mcp::auth::{AuthProvider, PskAuth};
    use tokio::io::{duplex, BufReader, BufWriter};

    let psk = "test-secret-for-collab-bridge";
    let (client_stream, server_stream) = duplex(4096);
    let (cr, cw) = tokio::io::split(client_stream);
    let (sr, sw) = tokio::io::split(server_stream);

    let server_auth = PskAuth::new(psk);
    let server_handle = tokio::spawn(async move {
        let mut sr = BufReader::new(sr);
        let mut sw = BufWriter::new(sw);
        server_auth.server_handshake(&mut sr, &mut sw).await
    });

    let client_handle = tokio::spawn(async move {
        let mut cr = BufReader::new(cr);
        let mut cw = BufWriter::new(cw);
        perform_psk_auth(&mut cr, &mut cw, psk, None).await
    });

    let (server_result, client_result) = tokio::join!(server_handle, client_handle);
    assert!(
        server_result.unwrap().is_ok(),
        "server handshake should succeed with correct PSK"
    );
    assert!(
        client_result.unwrap().is_ok(),
        "perform_psk_auth should succeed with correct PSK"
    );
}
#[tokio::test]
async fn perform_psk_auth_wrong_key_fails() {
    use mae_mcp::auth::{AuthProvider, PskAuth};
    use tokio::io::{duplex, BufReader, BufWriter};

    let (client_stream, server_stream) = duplex(4096);
    let (cr, cw) = tokio::io::split(client_stream);
    let (sr, sw) = tokio::io::split(server_stream);

    let server_auth = PskAuth::new("server-key");
    let server_handle = tokio::spawn(async move {
        let mut sr = BufReader::new(sr);
        let mut sw = BufWriter::new(sw);
        server_auth.server_handshake(&mut sr, &mut sw).await
    });

    let client_handle = tokio::spawn(async move {
        let mut cr = BufReader::new(cr);
        let mut cw = BufWriter::new(cw);
        perform_psk_auth(&mut cr, &mut cw, "wrong-key", None).await
    });

    let (server_result, client_result) = tokio::join!(server_handle, client_handle);
    let server_ok = server_result.is_ok_and(|r| r.is_ok());
    let client_ok = client_result.is_ok_and(|r| r.is_ok());
    assert!(
        !server_ok || !client_ok,
        "mismatched PSK should cause at least one side to fail"
    );
}
#[tokio::test]
async fn perform_psk_auth_empty_key_skips_auth() {
    // Empty PSK should skip auth entirely (no reads/writes on the stream).
    use tokio::io::{duplex, BufReader, BufWriter};

    let (client_stream, _server_stream) = duplex(4096);
    let (cr, cw) = tokio::io::split(client_stream);
    let mut cr = BufReader::new(cr);
    let mut cw = BufWriter::new(cw);

    let result = perform_psk_auth(&mut cr, &mut cw, "", None).await;
    assert!(result.is_ok(), "empty PSK should skip auth and return Ok");
}
#[test]
fn setup_collab_channels_propagates_psk_direct() {
    // When collab.psk is set (no psk_command), it should flow through to CollabSpawn.psk.
    let mut editor = Editor::new();
    let _ = editor.set_option("collab_psk", "my-secret-key");

    let (_evt_rx, _cmd_tx, spawn) = setup_collab_channels(&editor);
    assert_eq!(
        spawn.transport.plain_psk(),
        Some("my-secret-key"),
        "transport should carry the direct PSK value"
    );
}
#[test]
fn setup_collab_channels_propagates_psk_command() {
    // When collab.psk_command is set, it should be prefixed with "cmd:" sentinel.
    let mut editor = Editor::new();
    let _ = editor.set_option("collab_psk_command", "cat /tmp/test-psk.txt");

    let (_evt_rx, _cmd_tx, spawn) = setup_collab_channels(&editor);
    assert_eq!(
        spawn.transport.plain_psk(),
        Some("cmd:cat /tmp/test-psk.txt"),
        "transport should carry the cmd: prefix for deferred resolution"
    );
}
#[test]
fn setup_collab_channels_psk_command_takes_precedence() {
    // When both psk and psk_command are set, psk_command wins.
    let mut editor = Editor::new();
    let _ = editor.set_option("collab_psk", "plaintext-key");
    let _ = editor.set_option("collab_psk_command", "pass show mae/psk");

    let (_evt_rx, _cmd_tx, spawn) = setup_collab_channels(&editor);
    let psk = spawn.transport.plain_psk().unwrap_or("");
    assert!(
        psk.starts_with("cmd:"),
        "psk_command should take precedence over psk: got '{psk}'"
    );
    assert_eq!(psk, "cmd:pass show mae/psk");
}
#[test]
fn setup_collab_channels_empty_psk_is_empty() {
    // With no psk/psk_command AND no keystore, the credential is empty.
    let (psk, key_id) = resolve_client_credential("", "", None);
    assert!(psk.is_empty(), "no creds → empty psk, got '{psk}'");
    assert_eq!(key_id, None);
}
#[test]
fn resolve_credential_precedence() {
    // psk_command wins, returned as a cmd: sentinel, no key_id.
    let (psk, id) = resolve_client_credential("pass show k", "plain", None);
    assert_eq!(psk, "cmd:pass show k");
    assert_eq!(id, None);
    // psk wins over keystore when no command.
    let (psk, id) = resolve_client_credential("", "plain", None);
    assert_eq!(psk, "plain");
    assert_eq!(id, None);
}
#[test]
fn resolve_credential_from_keystore_primary() {
    // A keystore with a named primary key → present its secret + name.
    let dir = std::env::temp_dir().join(format!("mae-cred-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("trusted_keys");
    mae_mcp::keystore::add_key(&path, Some("framework"), "deadbeef").unwrap();
    mae_mcp::keystore::add_key(&path, Some("thinkpad"), "cafef00d").unwrap();

    let (psk, id) = resolve_client_credential("", "", Some(&path));
    assert_eq!(psk, "deadbeef", "presents the primary (first) key");
    assert_eq!(id.as_deref(), Some("framework"), "advertises the key name");

    let _ = std::fs::remove_dir_all(&dir);
}
#[test]
fn drain_discover_peers_does_not_send_command() {
    // DiscoverPeers is handled locally (mDNS browse + buffer creation).
    // It should NOT send any CollabCommand to the network channel.
    // NOTE: MdnsManager::new() may fail on CI (no multicast), but that's
    // fine — the intent is still consumed (returns early with status msg).
    let mut editor = Editor::new();
    editor.collab.pending_intent = Some(CollabIntent::DiscoverPeers);
    let (tx, mut rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);

    // Intent must be consumed regardless of mDNS availability.
    assert!(
        editor.collab.pending_intent.is_none(),
        "DiscoverPeers intent should be consumed"
    );
    // No command should be sent to the collab task.
    assert!(
        rx.try_recv().is_err(),
        "DiscoverPeers should not send any CollabCommand"
    );
}

// ───────────────────────── ADR-037 §2b: live content-encryption wiring ─────────────
