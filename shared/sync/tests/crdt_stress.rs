//! Pure CRDT stress tests (no network).
//!
//! Exercises TextSync under high load: many clients, rapid undo/redo,
//! UTF-16 edge cases, and large reconcile operations.
//!
//! Run: cargo test -p mae-sync --test crdt_stress -- --nocapture

use mae_sync::text::TextSync;
use rand::Rng;

/// FNV-1a 32-bit hash matching production `compute_client_id`.
/// Produces client_ids safe for yrs v1 wire format.
fn test_client_id(pid: u32, buf_idx: u32) -> u64 {
    let mut h: u32 = 0x811c_9dc5;
    for b in pid.to_le_bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    for b in buf_idx.to_le_bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    if h == 0 {
        1
    } else {
        h as u64
    }
}

#[test]
fn stress_100_clients_single_doc() {
    const NUM_CLIENTS: u64 = 100;
    const EDITS_PER_CLIENT: usize = 100;

    let mut rng = rand::rng();

    // Create 100 TextSync instances, each with a unique client_id.
    // Use test_client_id to produce realistic 32-bit hashed IDs.
    let mut clients: Vec<TextSync> = (0..NUM_CLIENTS)
        .map(|id| TextSync::with_client_id("", test_client_id(id as u32 + 1000, 0)))
        .collect();

    // Collect all encoded states after edits for cross-merge.
    let mut all_updates: Vec<Vec<Vec<u8>>> = Vec::with_capacity(NUM_CLIENTS as usize);

    for client in clients.iter_mut() {
        let mut updates = Vec::new();
        for _ in 0..EDITS_PER_CLIENT {
            let content = client.content();
            let len = content.chars().count();
            let pos = if len == 0 {
                0
            } else {
                rng.random_range(0..=len)
            };
            let ch = (b'a' + rng.random_range(0..26u8)) as char;
            let update = client.insert(pos as u32, &ch.to_string());
            updates.push(update);
        }
        all_updates.push(updates);
    }

    // Each client should have exactly EDITS_PER_CLIENT chars.
    for client in &clients {
        assert_eq!(
            client.content().chars().count(),
            EDITS_PER_CLIENT,
            "client should have {EDITS_PER_CLIENT} chars before merge"
        );
    }

    // Create a reference doc that merges all states.
    let mut reference = TextSync::with_client_id("", test_client_id(9999, 0));
    for client in &clients {
        let state = client.encode_state();
        reference.apply_update(&state).unwrap();
    }

    let ref_content = reference.content();
    assert!(!ref_content.is_empty(), "merged doc should not be empty");
    // Total chars = 100 clients * 100 inserts = 10,000
    assert_eq!(
        ref_content.chars().count(),
        (NUM_CLIENTS as usize) * EDITS_PER_CLIENT,
        "merged doc should contain all edits"
    );
    // Valid UTF-8 (guaranteed by String, but let's be explicit).
    assert!(std::str::from_utf8(ref_content.as_bytes()).is_ok());

    // Now apply all states to each client and verify convergence.
    for i in 0..clients.len() {
        for j in 0..clients.len() {
            if i != j {
                let state = clients[j].encode_state();
                clients[i].apply_update(&state).unwrap();
            }
        }
    }

    // All clients should now have the same content as the reference.
    for (idx, client) in clients.iter().enumerate() {
        assert_eq!(
            client.content(),
            ref_content,
            "client {idx} diverged from reference after full merge"
        );
    }
}

#[test]
fn stress_interleaved_undo_redo() {
    const TOTAL_EDITS: usize = 1000;
    let mut rng = rand::rng();
    let mut ts = TextSync::with_client_id("", test_client_id(2000, 0));
    ts.enable_undo();

    let mut edit_count = 0u64;
    let mut undo_count = 0u64;
    let mut redo_count = 0u64;

    for i in 0..TOTAL_EDITS {
        let roll: f64 = rng.random();
        if roll < 0.3 && edit_count > 0 {
            // 30% chance: undo
            let result = ts.undo();
            if result.success {
                undo_count += 1;
            }
        } else if roll < 0.4 && undo_count > redo_count {
            // 10% chance: redo (only if there's something to redo)
            let result = ts.redo();
            if result.success {
                redo_count += 1;
            }
        } else {
            // Insert a character
            let content = ts.content();
            let len = content.chars().count();
            let pos = if len == 0 {
                0
            } else {
                rng.random_range(0..=len)
            };
            let ch = (b'A' + (i % 26) as u8) as char;
            ts.insert(pos as u32, &ch.to_string());
            edit_count += 1;
        }

        // Content should always be valid UTF-8.
        let content = ts.content();
        assert!(
            std::str::from_utf8(content.as_bytes()).is_ok(),
            "invalid UTF-8 at iteration {i}"
        );
    }

    // Final state should be valid.
    let final_content = ts.content();
    assert!(
        std::str::from_utf8(final_content.as_bytes()).is_ok(),
        "final content is not valid UTF-8"
    );

    eprintln!(
        "stress_interleaved_undo_redo: edits={edit_count}, undos={undo_count}, redos={redo_count}, final_len={}",
        final_content.len()
    );
}

#[test]
fn stress_utf16_edge_cases() {
    let mut ts = TextSync::with_client_id("", test_client_id(3000, 0));

    // Insert emoji (4-byte UTF-8, surrogate pair in UTF-16).
    let update_emoji = ts.insert(0, "🎉");
    assert!(!update_emoji.is_empty());
    assert_eq!(ts.content(), "🎉");

    // Insert CJK after emoji.
    ts.insert(1, "你好世界");
    assert_eq!(ts.content(), "🎉你好世界");

    // Insert combining character sequence (e + combining acute = é).
    ts.insert(5, "e\u{0301}");
    assert_eq!(ts.content(), "🎉你好世界e\u{0301}");

    // Insert ASCII at various positions around multi-byte chars.
    ts.insert(0, "A");
    assert_eq!(ts.content(), "A🎉你好世界e\u{0301}");
    ts.insert(2, "B");
    assert_eq!(ts.content(), "A🎉B你好世界e\u{0301}");

    // Delete emoji (1 char, but 2 UTF-16 code units).
    ts.delete(1, 1);
    assert_eq!(ts.content(), "AB你好世界e\u{0301}");

    // Delete CJK range.
    ts.delete(2, 2); // 你好
    assert_eq!(ts.content(), "AB世界e\u{0301}");

    // Delete combining sequence (e + combining acute = 2 chars).
    ts.delete(4, 2);
    assert_eq!(ts.content(), "AB世界");

    // More complex: mix of emoji, CJK, ASCII in one string.
    let mut ts2 = TextSync::with_client_id("", test_client_id(3001, 0));
    let mixed = "Hello🌍世界café";
    ts2.insert(0, mixed);
    assert_eq!(ts2.content(), mixed);

    // Delete from the middle spanning multi-byte boundaries.
    // "Hello🌍世界café" — delete 🌍世界 (3 chars starting at pos 5)
    ts2.delete(5, 3);
    assert_eq!(ts2.content(), "Hellocafé");

    // Test reconcile_to with multi-byte chars.
    let mut ts3 = TextSync::with_client_id("abc", test_client_id(3002, 0));
    let update = ts3.reconcile_to("🎉你好");
    assert!(!update.is_empty());
    assert_eq!(ts3.content(), "🎉你好");

    // Reconcile back to ASCII.
    let update2 = ts3.reconcile_to("hello world");
    assert!(!update2.is_empty());
    assert_eq!(ts3.content(), "hello world");

    // Reconcile to empty.
    let update3 = ts3.reconcile_to("");
    assert!(!update3.is_empty());
    assert_eq!(ts3.content(), "");

    // Reconcile from empty to emoji-heavy string.
    let emoji_str = "🎉🎊🎈🎁🎂🎄🎃🎇🎆🎍";
    let update4 = ts3.reconcile_to(emoji_str);
    assert!(!update4.is_empty());
    assert_eq!(ts3.content(), emoji_str);

    // All content is valid UTF-8.
    assert!(std::str::from_utf8(ts.content().as_bytes()).is_ok());
    assert!(std::str::from_utf8(ts2.content().as_bytes()).is_ok());
    assert!(std::str::from_utf8(ts3.content().as_bytes()).is_ok());
}

#[test]
fn stress_rapid_reconcile_convergence() {
    let mut rng = rand::rng();
    let mut ts_a = TextSync::with_client_id("initial", test_client_id(4000, 0));
    let state = ts_a.encode_state();
    let mut ts_b = TextSync::from_state_with_client_id(&state, test_client_id(4001, 0)).unwrap();

    for round in 0..100 {
        // Each side reconciles to a different random target.
        let target_a: String = (0..rng.random_range(5..50))
            .map(|_| (b'a' + rng.random_range(0..26u8)) as char)
            .collect();
        let target_b: String = (0..rng.random_range(5..50))
            .map(|_| (b'A' + rng.random_range(0..26u8)) as char)
            .collect();

        let update_a = ts_a.reconcile_to(&target_a);
        let update_b = ts_b.reconcile_to(&target_b);

        // Cross-apply updates.
        if !update_a.is_empty() {
            ts_b.apply_update(&update_a).unwrap();
        }
        if !update_b.is_empty() {
            ts_a.apply_update(&update_b).unwrap();
        }

        // After cross-apply, both should have the same content.
        assert_eq!(
            ts_a.content(),
            ts_b.content(),
            "divergence at round {round}"
        );

        // Content should be valid UTF-8.
        assert!(std::str::from_utf8(ts_a.content().as_bytes()).is_ok());
    }

    eprintln!(
        "stress_rapid_reconcile_convergence: final content len = {}",
        ts_a.content().len()
    );
}

#[test]
fn stress_empty_and_large_reconcile() {
    // Test 1: empty -> 10k chars.
    let mut ts = TextSync::with_client_id("", test_client_id(5000, 0));
    let large: String = (0..10_000)
        .map(|i| (b'a' + (i % 26) as u8) as char)
        .collect();

    let update = ts.reconcile_to(&large);
    assert!(!update.is_empty());
    assert_eq!(ts.content(), large);
    assert_eq!(ts.content().chars().count(), 10_000);

    // Test 2: 10k chars -> empty.
    let update2 = ts.reconcile_to("");
    assert!(!update2.is_empty());
    assert_eq!(ts.content(), "");

    // Test 3: 10k chars -> slightly different 10k chars.
    let mut ts2 = TextSync::with_client_id("", test_client_id(5001, 0));
    let large_a: String = (0..10_000)
        .map(|i| (b'a' + (i % 26) as u8) as char)
        .collect();
    ts2.reconcile_to(&large_a);
    assert_eq!(ts2.content().chars().count(), 10_000);

    // Change ~1% of characters (every 100th char).
    let large_b: String = large_a
        .chars()
        .enumerate()
        .map(|(i, ch)| if i % 100 == 0 { 'Z' } else { ch })
        .collect();

    let update3 = ts2.reconcile_to(&large_b);
    assert!(!update3.is_empty());
    assert_eq!(ts2.content(), large_b);
    assert_eq!(ts2.content().chars().count(), 10_000);

    // The update should be small (only ~100 changes, not a full 10k rewrite).
    // We can't assert exact size, but it should be significantly smaller than
    // the full state encoding.
    let full_state = ts2.encode_state();
    eprintln!(
        "stress_empty_and_large_reconcile: diff update = {} bytes, full state = {} bytes",
        update3.len(),
        full_state.len()
    );

    // Test 4: Apply the reconcile update to another doc and verify convergence.
    let mut ts3 = TextSync::with_client_id("", test_client_id(5002, 0));
    ts3.reconcile_to(&large_a); // Start with the same initial content.
    ts3.apply_update(&update3).unwrap();
    // They won't necessarily be equal because ts3 started independently,
    // but applying the full state should converge them.
    let state2 = ts2.encode_state();
    let ts4 = TextSync::from_state(&state2).unwrap();
    assert_eq!(ts4.content(), large_b);
}
