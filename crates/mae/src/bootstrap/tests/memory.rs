//! Bootstrap tests: AI memory-context loading and budget-aware synthesis.

use super::super::*;

#[test]
fn load_memory_context_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let mem_dir = dir.path().join(".mae/memory");
    std::fs::create_dir_all(&mem_dir).unwrap();
    assert!(load_memory_context(dir.path()).is_none());
}

#[test]
fn load_memory_context_sorted_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let mem_dir = dir.path().join(".mae/memory");
    std::fs::create_dir_all(&mem_dir).unwrap();
    std::fs::write(mem_dir.join("1000_old.txt"), "old fact").unwrap();
    std::fs::write(mem_dir.join("2000_new.txt"), "new fact").unwrap();
    let result = load_memory_context(dir.path()).unwrap();
    assert!(result.starts_with("## Long-term Memory\n"));
    let new_pos = result.find("new fact").unwrap();
    let old_pos = result.find("old fact").unwrap();
    assert!(new_pos < old_pos, "newer entries should come first");
}

// --- synthesize_memory tests ---

#[test]
fn synthesize_memory_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(synthesize_memory(
        dir.path(),
        mae_ai::context_limits::ModelTier::Full,
        mae_ai::context_limits::ProviderHint::Claude,
        4000,
    )
    .is_none());
}

#[test]
fn synthesize_memory_small_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let mem_dir = dir.path().join(".mae/memory");
    std::fs::create_dir_all(&mem_dir).unwrap();
    std::fs::write(mem_dir.join("1000_fact.txt"), "always use snake_case").unwrap();
    std::fs::write(mem_dir.join("2000_fact.txt"), "the crate uses ropey").unwrap();
    let result = synthesize_memory(
        dir.path(),
        mae_ai::context_limits::ModelTier::Full,
        mae_ai::context_limits::ProviderHint::Claude,
        4000,
    )
    .unwrap();
    assert!(result.contains("## Project Memory"));
    assert!(result.contains("always use snake_case"));
    assert!(result.contains("the crate uses ropey"));
}

#[test]
fn synthesize_memory_exceeds_budget() {
    let dir = tempfile::tempdir().unwrap();
    let mem_dir = dir.path().join(".mae/memory");
    std::fs::create_dir_all(&mem_dir).unwrap();
    for i in 0..50 {
        let content = format!("always follow convention rule {}", i);
        std::fs::write(mem_dir.join(format!("{:04}_fact.txt", i)), content).unwrap();
    }
    let result = synthesize_memory(
        dir.path(),
        mae_ai::context_limits::ModelTier::Full,
        mae_ai::context_limits::ProviderHint::Claude,
        200, // tiny budget
    )
    .unwrap();
    assert!(result.len() <= 250); // budget + header
}

#[test]
fn synthesize_memory_compact_numbered() {
    let dir = tempfile::tempdir().unwrap();
    let mem_dir = dir.path().join(".mae/memory");
    std::fs::create_dir_all(&mem_dir).unwrap();
    std::fs::write(mem_dir.join("1000_fact.txt"), "always use bun").unwrap();
    let result = synthesize_memory(
        dir.path(),
        mae_ai::context_limits::ModelTier::Compact,
        mae_ai::context_limits::ProviderHint::Claude,
        4000,
    )
    .unwrap();
    // Compact tier → numbered list
    assert!(result.contains("1. always use bun"));
}

#[test]
fn synthesize_memory_deepseek_forces_numbered() {
    let dir = tempfile::tempdir().unwrap();
    let mem_dir = dir.path().join(".mae/memory");
    std::fs::create_dir_all(&mem_dir).unwrap();
    std::fs::write(mem_dir.join("1000_fact.txt"), "always use bun").unwrap();
    let result = synthesize_memory(
        dir.path(),
        mae_ai::context_limits::ModelTier::Full, // Full tier, but DeepSeek → numbered
        mae_ai::context_limits::ProviderHint::DeepSeek,
        4000,
    )
    .unwrap();
    assert!(result.contains("1. always use bun"));
}

#[test]
fn synthesize_memory_categories_ordered() {
    let dir = tempfile::tempdir().unwrap();
    let mem_dir = dir.path().join(".mae/memory");
    std::fs::create_dir_all(&mem_dir).unwrap();
    std::fs::write(mem_dir.join("1000_a.txt"), "bug: crash on startup").unwrap();
    std::fs::write(mem_dir.join("2000_b.txt"), "always use tabs").unwrap();
    let result = synthesize_memory(
        dir.path(),
        mae_ai::context_limits::ModelTier::Full,
        mae_ai::context_limits::ProviderHint::Claude,
        4000,
    )
    .unwrap();
    // Conventions should appear before bugs
    let conv_pos = result.find("always use tabs").unwrap();
    let bug_pos = result.find("crash on startup").unwrap();
    assert!(
        conv_pos < bug_pos,
        "conventions should appear before bugs: conv={}, bug={}",
        conv_pos,
        bug_pos
    );
}

#[test]
fn load_memory_context_cap_enforcement() {
    let dir = tempfile::tempdir().unwrap();
    let mem_dir = dir.path().join(".mae/memory");
    std::fs::create_dir_all(&mem_dir).unwrap();
    // Write enough files to exceed 8000 chars
    for i in 0..100 {
        let content = format!("fact number {} with padding {}", i, "x".repeat(100));
        std::fs::write(mem_dir.join(format!("{:04}_entry.txt", i)), content).unwrap();
    }
    let result = load_memory_context(dir.path()).unwrap();
    // Should be capped near 8000 + truncation message
    assert!(result.len() < 8100);
}
