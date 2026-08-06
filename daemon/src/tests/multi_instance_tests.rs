//! Two `mae-daemon` instances on one host (staging + production).
//!
//! `assets/mae-daemon@.service` is a systemd TEMPLATE unit — `mae-daemon@staging`
//! and `mae-daemon@prod` are meant to run side by side, each with its own
//! `--config daemon-%i.toml`. These tests hold that promise to account:
//!
//!   1. an administrative subcommand must operate on the instance `--config`
//!      names, not on whichever instance owns the default config; and
//!   2. two instances must not share a single resource.
//!
//! Both were false until 2026-08. (1) was a plain bug. (2) is still a live trap
//! for any operator who assumes `data_dir` scopes everything — it does not, and
//! `identity_and_authorized_keys_do_not_follow_data_dir` pins the exact extent of
//! that so it cannot be rediscovered by an authorization leak in production.

use crate::cli::Cli;
use crate::config::DaemonConfig;
use std::path::PathBuf;

/// A config file as an operator would actually write one for a named instance.
/// `shared_identity = true` deliberately OMITS the identity/authorized_keys/
/// keystore overrides — the mistake being characterised.
fn write_instance_config(
    dir: &std::path::Path,
    name: &str,
    port: u16,
    shared_identity: bool,
) -> PathBuf {
    let root = dir.join(name);
    let identity = if shared_identity {
        String::new()
    } else {
        format!(
            "identity_dir = \"{root}/collab\"\n\
             authorized_keys = \"{root}/collab/authorized_keys\"\n\
             keystore = \"{root}/collab/trusted_keys\"\n",
            root = root.display(),
        )
    };
    let toml = format!(
        r#"
socket = "{root}/kb.sock"
data_dir = "{root}/data"

[collab]
enabled = true
bind = "127.0.0.1:{port}"

[collab.auth]
mode = "key"
{identity}
[oauth]
enabled = true
bind = "127.0.0.1:{oauth_port}"
"#,
        root = root.display(),
        oauth_port = port + 1000,
    );
    let path = dir.join(format!("daemon-{name}.toml"));
    std::fs::write(&path, toml).expect("write config");
    path
}

fn resolve(path: &std::path::Path) -> DaemonConfig {
    Cli::parse(vec![
        "mae-daemon".to_string(),
        "doctor".to_string(),
        "--config".to_string(),
        path.display().to_string(),
    ])
    .resolve_config()
    .expect("config resolves")
}

#[test]
fn a_subcommand_reads_the_instance_its_config_names() {
    // The regression: every subcommand called `DaemonConfig::load()` itself, so
    // `--config` was parsed only on the path those subcommands had already
    // returned from. The oracle is deliberately NOT "the call succeeded" — it is
    // that the values reported belong to the NAMED instance and are different
    // from the other instance's, which is the only thing that distinguishes a
    // fixed parser from one that silently read the default file.
    let tmp = tempfile::tempdir().expect("tempdir");
    let staging = write_instance_config(tmp.path(), "staging", 19473, false);
    let prod = write_instance_config(tmp.path(), "prod", 19475, false);

    let s = resolve(&staging);
    let p = resolve(&prod);

    assert_eq!(s.socket, tmp.path().join("staging/kb.sock"));
    assert_eq!(p.socket, tmp.path().join("prod/kb.sock"));
    assert_ne!(s.socket, p.socket);
    assert_eq!(s.collab.bind.port(), 19473);
    assert_eq!(p.collab.bind.port(), 19475);
    assert_eq!(s.oauth.bind.port(), 20473);
    assert_eq!(p.oauth.bind.port(), 20475);
}

#[test]
fn two_instances_do_not_share_any_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let staging = resolve(&write_instance_config(tmp.path(), "staging", 19473, false));
    let prod = resolve(&write_instance_config(tmp.path(), "prod", 19475, false));

    let conflicts = staging
        .instance_paths()
        .conflicts_with(&prod.instance_paths());
    assert!(
        conflicts.is_empty(),
        "staging and prod share resources they must own exclusively: {conflicts:?}"
    );

    // And the report is not vacuously empty because it examined nothing: every
    // resource kind must actually be present on both sides.
    let labels: Vec<_> = staging
        .instance_paths()
        .labelled()
        .into_iter()
        .map(|(l, _)| l)
        .collect();
    for expected in [
        "socket",
        "data_dir",
        "collab data_dir",
        "collab.bind",
        "oauth.bind",
        "identity_dir",
        "authorized_keys",
        "keystore",
    ] {
        assert!(
            labels.contains(&expected),
            "{expected} missing from the instance-resource report — a resource that \
             isn't reported can't be checked for collision. Reported: {labels:?}"
        );
    }
}

#[test]
fn conflicts_with_actually_detects_a_collision() {
    // Guard against the previous assertion passing because `conflicts_with`
    // never returns anything: an instance always collides with itself, and two
    // instances sharing exactly one port must report exactly that one.
    let tmp = tempfile::tempdir().expect("tempdir");
    let a = resolve(&write_instance_config(tmp.path(), "a", 19473, false));
    let self_conflicts = a.instance_paths().conflicts_with(&a.instance_paths());
    assert_eq!(
        self_conflicts.len(),
        a.instance_paths().labelled().len(),
        "an instance shares every resource with itself"
    );

    let mut b = resolve(&write_instance_config(tmp.path(), "b", 19475, false));
    b.collab.bind = a.collab.bind;
    let conflicts = a.instance_paths().conflicts_with(&b.instance_paths());
    assert_eq!(
        conflicts,
        vec![format!("collab.bind = {}", a.collab.bind)],
        "the one shared resource, and only it, must be reported"
    );
}

#[test]
fn identity_and_authorized_keys_do_not_follow_data_dir() {
    // THE TRAP, pinned. An operator who scopes an instance by `data_dir` (as the
    // shipped `mae-daemon@.service` template does — `--data-dir …/tenants/%i`)
    // reasonably expects everything to follow. Identity, authorized_keys and the
    // keystore do NOT: they default to the shared `$XDG_DATA_HOME/mae/collab/`.
    //
    // Consequence: `mae-daemon authorize --config daemon-staging.toml <key>`
    // writes into the SAME authorized_keys production reads, so authorising a
    // tester for staging authorises them for production. That default is not
    // changed here (relocating an existing operator's identity key would lose
    // access to every shared KB, irrecoverably) — it is asserted, so the
    // asymmetry is a documented property rather than a production surprise.
    let tmp = tempfile::tempdir().expect("tempdir");
    // This test's subject IS the ambient `default_collab_dir()` fallback, which
    // the effect sandbox otherwise refuses in a test build. Opting in is safe
    // and necessary here: the test only *resolves* paths and never writes to
    // them. Without the opt-in both sides resolve to `None` and the three
    // `assert_eq!`s below pass **vacuously** — None == None — which is exactly
    // the failure mode the explicit is-Some assertion guards against.
    let staging = resolve(&write_instance_config(tmp.path(), "staging", 19473, true));
    let prod = resolve(&write_instance_config(tmp.path(), "prod", 19475, true));

    // `identity_dir()` / `authorized_keys_path()` resolve the ambient
    // `default_collab_dir()` fallback lazily on EVERY call, so the opt-in has
    // to cover the assertions, not just construction. Safe here: the test only
    // resolves paths and never writes to them.
    mae_effect_sandbox::with_external_effects(|| {
        assert!(
            staging.collab.auth.identity_dir().is_some(),
            "identity_dir must actually resolve — otherwise the equality assertions \
         below compare None to None and prove nothing about the shared-path trap"
        );

        assert_ne!(
            staging.effective_data_dir(),
            prod.effective_data_dir(),
            "precondition: the two configs really do differ in data_dir"
        );
        assert_eq!(
            staging.collab.auth.identity_dir(),
            prod.collab.auth.identity_dir(),
            "identity_dir is shared despite differing data_dir — if this ever \
         becomes false, the trap is gone and DAEMON_ADMIN's warning should go too"
        );
        assert_eq!(
            staging.collab.auth.authorized_keys_path(),
            prod.collab.auth.authorized_keys_path(),
            "authorized_keys is shared despite differing data_dir"
        );
        assert_eq!(
            staging.collab.auth.keystore_path(),
            prod.collab.auth.keystore_path(),
            "keystore is shared despite differing data_dir"
        );

        // …and the collision report SAYS SO, which is the whole mitigation: an
        // operator who diffs two `--check-config` outputs sees it.
        let conflicts = staging
            .instance_paths()
            .conflicts_with(&prod.instance_paths());
        for expected in ["identity_dir", "authorized_keys", "keystore"] {
            assert!(
                conflicts.iter().any(|c| c.starts_with(expected)),
                "{expected} collision must be reported, got {conflicts:?}"
            );
        }
        // Everything genuinely scoped by data_dir must NOT be reported — otherwise
        // the report is noise and an operator learns to ignore it.
        for scoped in ["socket", "data_dir", "collab.bind", "oauth.bind"] {
            assert!(
                !conflicts.iter().any(|c| c.starts_with(scoped)),
                "{scoped} is per-instance and must not appear in {conflicts:?}"
            );
        }
    });
}

#[test]
fn the_shipped_instance_template_produces_two_non_colliding_instances() {
    // `assets/daemon-instance-config.toml` is what an operator copies to stand
    // up a second daemon, and it is the ONLY place the "set all eight, not four"
    // rule is expressed as something executable rather than prose. If it stops
    // parsing, or stops setting one of the three paths that don't follow
    // data_dir, the documented procedure silently produces two instances
    // sharing an authorized_keys — the exact failure the template exists to
    // prevent. So exercise it the way the docs say to use it.
    let template = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../assets/daemon-instance-config.toml"
    ))
    .expect(
        "assets/daemon-instance-config.toml must exist — it is referenced by \
             docs/DAEMON_ADMIN.md, assets/mae-daemon@.service and install.sh",
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let mut resolved = Vec::new();
    for (name, collab_port, oauth_port) in [("staging", 19473, 18443), ("prod", 19475, 18445)] {
        // Exactly the substitutions the template's own header instructs.
        let filled = template
            .replace("INSTANCE", name)
            .replace("/home/USER", &tmp.path().display().to_string())
            .replace("/run/user/1000", &tmp.path().display().to_string())
            .replace("127.0.0.1:9473", &format!("127.0.0.1:{collab_port}"))
            .replace("127.0.0.1:8443", &format!("127.0.0.1:{oauth_port}"));
        let path = tmp.path().join(format!("daemon-{name}.toml"));
        std::fs::write(&path, filled).expect("write filled template");
        resolved.push(resolve(&path));
    }

    let (staging, prod) = (&resolved[0], &resolved[1]);
    let conflicts = staging
        .instance_paths()
        .conflicts_with(&prod.instance_paths());
    assert!(
        conflicts.is_empty(),
        "two instances made from the SHIPPED template still share: {conflicts:?}"
    );

    // Specifically the three that don't follow data_dir — an empty conflict list
    // would also result from the template failing to set them at all (both
    // sides `None`, nothing to compare), so assert they are set and distinct.
    for (label, a, b) in [
        (
            "identity_dir",
            staging.collab.auth.identity_dir(),
            prod.collab.auth.identity_dir(),
        ),
        (
            "authorized_keys",
            staging.collab.auth.authorized_keys_path(),
            prod.collab.auth.authorized_keys_path(),
        ),
        (
            "keystore",
            staging.collab.auth.keystore_path(),
            prod.collab.auth.keystore_path(),
        ),
    ] {
        let (a, b) = (a.expect(label), b.expect(label));
        assert!(
            a.starts_with(tmp.path()) && b.starts_with(tmp.path()),
            "{label} must be inside the instance tree, not the shared XDG \
             default — got {a:?} / {b:?}"
        );
        assert_ne!(a, b, "{label} must differ between instances");
    }

    // The template must not ship the unauthenticated default it warns about.
    assert_eq!(
        staging.collab.auth.mode, "key",
        "the template sets mode explicitly because the code default is \"none\""
    );
}

#[test]
fn an_override_flag_beats_the_config_file() {
    // `mae-daemon@.service` passes BOTH `--config daemon-%i.toml` and
    // `--data-dir …/tenants/%i`; if the file also sets data_dir, the flag has to
    // win or the unit's per-instance scoping is a no-op.
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = write_instance_config(tmp.path(), "staging", 19473, false);
    let cli = Cli::parse(
        [
            "mae-daemon",
            "doctor",
            "--config",
            &cfg.display().to_string(),
            "--data-dir",
            "/srv/mae/tenants/staging",
            "--bind",
            "0.0.0.0:29473",
            "--socket",
            "/run/mae/staging.sock",
            "--oauth-bind",
            "0.0.0.0:28443",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>(),
    );
    let resolved = cli.resolve_config().expect("resolves");
    assert_eq!(
        resolved.data_dir,
        Some(PathBuf::from("/srv/mae/tenants/staging"))
    );
    assert_eq!(resolved.socket, PathBuf::from("/run/mae/staging.sock"));
    assert_eq!(resolved.collab.bind.to_string(), "0.0.0.0:29473");
    assert_eq!(resolved.oauth.bind.to_string(), "0.0.0.0:28443");
}
