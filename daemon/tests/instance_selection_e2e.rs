//! `mae-daemon <subcommand> --config <path>` must operate on the instance that
//! path names — end to end, through the real binary.
//!
//! The unit tests in `src/cli.rs` prove the PARSER sees `--config` after a
//! subcommand. They cannot prove the subcommand USES it: until 2026-08 every
//! administrative subcommand called `DaemonConfig::load()` itself and read
//! `~/.config/mae/daemon.toml` regardless. These tests run the binary and read
//! its stdout, so they fail against that code — which is the only reason to have
//! them in addition to the unit tests.
//!
//! No daemon is started: `doctor` and `--check-config` both exit after
//! reporting, so there is no port to contend for and these run in default CI.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Run the daemon binary with an isolated HOME/XDG so the "default config" it
/// would wrongly fall back to is a real, DIFFERENT config on disk — not an
/// absent file that happens to produce built-in defaults. A test whose control
/// case is "no config at all" cannot distinguish "read the right file" from
/// "read nothing and defaulted".
fn run(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env_remove("MAE_DAEMON_CONFIG")
        .output()
        .expect("run mae-daemon");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "mae-daemon {args:?} failed: {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status
    );
    stdout
}

fn instance_config(dir: &Path, name: &str, port: u16) -> PathBuf {
    let root = dir.join(name);
    std::fs::create_dir_all(&root).expect("mkdir instance root");
    let toml = format!(
        r#"
socket = "{root}/kb.sock"
data_dir = "{root}/data"

[collab]
enabled = true
bind = "127.0.0.1:{port}"

[collab.auth]
mode = "key"
identity_dir = "{root}/collab"
authorized_keys = "{root}/collab/authorized_keys"
keystore = "{root}/collab/trusted_keys"
"#,
        root = root.display(),
    );
    let path = dir.join(format!("daemon-{name}.toml"));
    std::fs::write(&path, toml).expect("write config");
    path
}

/// An isolated HOME whose DEFAULT config (`~/.config/mae/daemon.toml`) is a
/// third, distinctly-identifiable instance. Any subcommand that ignores
/// `--config` lands here, and says so in its output.
fn home_with_default_config(tmp: &Path) -> PathBuf {
    let home = tmp.join("home");
    let cfg_dir = home.join(".config/mae");
    std::fs::create_dir_all(&cfg_dir).expect("mkdir config dir");
    std::fs::write(
        cfg_dir.join("daemon.toml"),
        format!(
            r#"
socket = "{home}/DEFAULT-INSTANCE.sock"
data_dir = "{home}/default-data"

[collab]
enabled = true
bind = "127.0.0.1:19999"
"#,
            home = home.display(),
        ),
    )
    .expect("write default config");
    home
}

#[test]
fn doctor_reports_the_instance_named_by_config_not_the_default() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = home_with_default_config(tmp.path());
    let staging = instance_config(tmp.path(), "staging", 19473);
    let prod = instance_config(tmp.path(), "prod", 19475);

    // Control: with no --config, doctor genuinely reads the default file. If
    // this assertion ever fails the rest of the test proves nothing, because
    // "didn't mention the default" would no longer mean "used --config".
    let default_out = run(&home, &["doctor"]);
    assert!(
        default_out.contains("DEFAULT-INSTANCE.sock"),
        "control case: bare `doctor` must read ~/.config/mae/daemon.toml.\n{default_out}"
    );

    for (cfg, sock, port) in [
        (&staging, "staging/kb.sock", "19473"),
        (&prod, "prod/kb.sock", "19475"),
    ] {
        let out = run(&home, &["doctor", "--config", &cfg.display().to_string()]);
        assert!(
            out.contains(sock),
            "`doctor --config {}` must report {sock}.\n{out}",
            cfg.display()
        );
        assert!(
            out.contains(port),
            "`doctor --config {}` must report port {port}.\n{out}",
            cfg.display()
        );
        assert!(
            !out.contains("DEFAULT-INSTANCE.sock"),
            "`doctor --config {}` leaked the DEFAULT instance — this is the bug.\n{out}",
            cfg.display()
        );
    }
}

#[test]
fn check_config_validates_the_instance_named_by_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = home_with_default_config(tmp.path());
    let prod = instance_config(tmp.path(), "prod", 19475);

    // `key` mode with an empty authorized_keys is a genuine config error
    // (`check_collab`), so authorize one first — via the same `--config`, which
    // is itself part of what this test covers.
    let peer = mae_mcp::identity::Identity::load_or_generate(&tmp.path().join("peer"), "peer")
        .expect("generate peer identity");
    run(
        &home,
        &[
            "authorize",
            "--config",
            &prod.display().to_string(),
            &peer.public().to_line(),
            "tester",
        ],
    );

    let out = run(
        &home,
        &["--check-config", "--config", &prod.display().to_string()],
    );
    assert!(out.contains("prod/kb.sock"), "{out}");
    assert!(out.contains("19475"), "{out}");
    assert!(
        !out.contains("DEFAULT-INSTANCE.sock"),
        "--check-config validated the wrong instance.\n{out}"
    );
    assert!(out.contains("Config OK"), "{out}");
}

#[test]
fn identity_is_generated_in_the_instance_its_config_names() {
    // The sharpest case: `identity` has a SIDE EFFECT (it generates a keypair on
    // first run). Reading the wrong config writes a key into the wrong instance
    // — and since the write is idempotent-looking, nothing downstream reveals it.
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = home_with_default_config(tmp.path());
    let staging = instance_config(tmp.path(), "staging", 19473);

    let out = run(
        &home,
        &["identity", "--config", &staging.display().to_string()],
    );
    let expected = tmp.path().join("staging/collab/id_ed25519");
    assert!(
        out.contains(&expected.display().to_string()),
        "identity must be created under the named instance ({}).\n{out}",
        expected.display()
    );
    assert!(
        expected.exists(),
        "no key written at {}",
        expected.display()
    );
    assert!(
        !home.join(".local/share/mae/collab/id_ed25519").exists(),
        "identity leaked into the DEFAULT instance's collab dir"
    );
}

#[test]
fn authorize_writes_into_the_instance_its_config_names() {
    // `authorize` is the one where reading the wrong config is a SECURITY
    // outcome: the peer ends up trusted by the instance the operator did not
    // name. The oracle is the file on disk, not the command's exit status.
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = home_with_default_config(tmp.path());
    let staging = instance_config(tmp.path(), "staging", 19473);
    let prod = instance_config(tmp.path(), "prod", 19475);

    // A real key, generated rather than hand-picked (principle #14).
    let peer_dir = tmp.path().join("peer");
    let peer = mae_mcp::identity::Identity::load_or_generate(&peer_dir, "peer")
        .expect("generate peer identity");
    let line = peer.public().to_line();

    let out = run(
        &home,
        &[
            "authorize",
            "--config",
            &staging.display().to_string(),
            &line,
            "tester",
        ],
    );
    assert!(out.contains("Authorized"), "{out}");

    let staging_keys = tmp.path().join("staging/collab/authorized_keys");
    let prod_keys = tmp.path().join("prod/collab/authorized_keys");
    assert!(
        staging_keys.exists(),
        "authorize wrote nothing to staging ({})",
        staging_keys.display()
    );
    let contents = std::fs::read_to_string(&staging_keys).expect("read staging authorized_keys");
    assert!(
        contents.contains(peer.public().to_line().split_whitespace().nth(1).unwrap()),
        "the peer key is not in staging's authorized_keys:\n{contents}"
    );
    assert!(
        !prod_keys.exists(),
        "authorizing for staging must not touch production's authorized_keys"
    );
    assert!(
        !home
            .join(".local/share/mae/collab/authorized_keys")
            .exists(),
        "authorize leaked the peer key into the DEFAULT instance — this is the \
         security consequence of the bug"
    );

    // And the instance that was NOT named does not trust the peer.
    let listed = run(
        &home,
        &["authorized", "--config", &prod.display().to_string()],
    );
    assert!(
        listed.contains("(0)"),
        "production must have zero authorized keys.\n{listed}"
    );
    let listed_staging = run(
        &home,
        &["authorized", "--config", &staging.display().to_string()],
    );
    assert!(
        listed_staging.contains(&peer.fingerprint()),
        "staging must list the peer it just authorized.\n{listed_staging}"
    );
}

/// Like `instance_config` but WITHOUT the identity/authorized_keys/keystore
/// overrides — the config an operator writes when they assume `data_dir` scopes
/// everything.
fn instance_config_sharing_identity(dir: &Path, name: &str, port: u16) -> PathBuf {
    let root = dir.join(name);
    std::fs::create_dir_all(&root).expect("mkdir instance root");
    let toml = format!(
        r#"
socket = "{root}/kb.sock"
data_dir = "{root}/data"

[collab]
enabled = true
bind = "127.0.0.1:{port}"
"#,
        root = root.display(),
    );
    let path = dir.join(format!("daemon-{name}.toml"));
    std::fs::write(&path, toml).expect("write config");
    path
}

#[test]
fn doctor_compare_with_flags_a_shared_resource_and_exits_nonzero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = home_with_default_config(tmp.path());

    // Fully-scoped pair: must pass, and must EXIT ZERO so a deploy gate can use it.
    let a = instance_config(tmp.path(), "staging", 19473);
    let b = instance_config(tmp.path(), "prod", 19475);
    let out = run(
        &home,
        &[
            "doctor",
            "--config",
            &a.display().to_string(),
            "--compare-with",
            &b.display().to_string(),
        ],
    );
    assert!(
        out.contains("side-by-side: OK"),
        "a correctly-scoped pair must pass.\n{out}"
    );

    // The trap: two instances scoped only by data_dir + port. They still share
    // identity/authorized_keys/keystore, and the check must SAY so and FAIL.
    let c = instance_config_sharing_identity(tmp.path(), "shared-a", 19477);
    let d = instance_config_sharing_identity(tmp.path(), "shared-b", 19479);
    let out = Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args([
            "doctor",
            "--config",
            &c.display().to_string(),
            "--compare-with",
            &d.display().to_string(),
        ])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .output()
        .expect("run mae-daemon");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !out.status.success(),
        "a shared resource must be a non-zero exit so CI/deploy can gate on it.\n{stdout}"
    );
    for shared in ["identity_dir", "authorized_keys", "keystore"] {
        assert!(
            stdout.contains(shared),
            "{shared} collision must be named in the output.\n{stdout}"
        );
    }
    // …and the resources that ARE correctly scoped must not be reported, or the
    // warning is noise and gets ignored.
    for scoped in ["kb.sock", "19477", "19479"] {
        assert!(
            !stdout.contains(&format!("! {scoped}")),
            "{scoped} is per-instance and must not be flagged.\n{stdout}"
        );
    }
}

#[test]
fn a_typoed_config_path_fails_loudly() {
    // `DaemonConfig::load_from` warns to stderr and returns DEFAULTS, so a
    // mistyped path used to produce a confident, entirely wrong report about an
    // instance that does not exist.
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = home_with_default_config(tmp.path());
    let out = Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args(["doctor", "--config", "/nonexistent/daemon-typo.toml"])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .output()
        .expect("run mae-daemon");
    assert!(
        !out.status.success(),
        "a config path that does not exist must be an error, not a silent default"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not found"), "stderr was: {stderr}");
}
