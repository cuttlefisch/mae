# Deploying mae-daemon

Ansible role and playbooks for running one or more `mae-daemon` instances
(KB persistence + collaborative-editing hub) on a Linux server.

The VM itself is expected to be provisioned by Terraform; this role takes over
from a bare host with SSH access and a Python interpreter.

---

## What this deploys

```
/opt/mae/
├── current -> releases/0.14.93     # switching this symlink IS the deploy
├── releases/
│   ├── 0.14.93/bin/mae-daemon
│   └── 0.14.92/bin/mae-daemon      # retained for rollback
├── DEPLOYED                        # version, timestamp, who, verified?
└── INSTANCES                       # what this host runs

/etc/mae/daemon-<instance>.toml     # root-owned, service-readable
/var/lib/mae/<instance>/            # data, WAL, and collab/ (0700, holds the key)
/run/mae/<instance>.sock            # KB socket — never expose off-host
```

One `systemd` unit per instance: `mae-daemon@staging`, `mae-daemon@prod`.

---

## Quick start

```bash
cd deploy/ansible

cp inventory/staging.yml.example inventory/staging.yml
$EDITOR inventory/staging.yml          # hostname, version, instances

ansible-playbook -i inventory/staging.yml site.yml --check --diff   # dry run
ansible-playbook -i inventory/staging.yml site.yml                  # apply
```

Staging and production are **separate inventory files**, deliberately. The cost
of a wrong `-i` should be "no such host", not "deployed to production".

### Useful invocations

```bash
# Health check only. Changes nothing; run it any time.
ansible-playbook -i inventory/production.yml verify.yml

# Re-render config without touching the binary
ansible-playbook -i inventory/staging.yml site.yml --tags mae_config

# Validate the configuration without contacting the host at all
ansible-playbook -i inventory/staging.yml site.yml --tags mae_preflight --check
```

---

## Rolling back

The previous release is still unpacked, so a rollback is a version change:

```bash
ansible-playbook -i inventory/production.yml site.yml -e mae_daemon_version=0.14.92
```

The `current` symlink repoints, units restart, and the verification gate asserts
the running binary reports `0.14.92`. Nothing is re-downloaded.

`mae_daemon_keep_releases` (default 3) controls how far back you can go. It can
never be 0 — preflight refuses that, since it would prune the running release.

---

## Safety checks

Preflight refuses, before writing or downloading anything:

| Refused | Why |
|---|---|
| Unpinned / non-semver version | A deploy that follows "latest" is not reproducible, and a rollback needs to know what it is rolling back *from* |
| No instances defined | Would install a binary and start nothing |
| Instance name outside `[a-z0-9-]{1,32}` | It becomes a unit name, a directory and a filename — this also forecloses path traversal |
| Duplicate instance names | Two entries with one name silently share a data directory |
| Two instances on one port | Only the first to start would bind |
| Non-loopback bind without `auth_mode: key` | mae-daemon's own default is `none`, which accepts **any** client that reaches the port; `psk` is plaintext on the wire |
| `verify_checksum: false` alone | Installing an unverified binary requires `allow_unverified: true` as well, so it cannot happen by typo |
| `keep_releases: 0` | Would prune the running release |
| Non-Linux / non-systemd host | There is no launchd or Windows-service equivalent of this unit |
| < 500 MB free | A full disk on a host running a WAL-backed store is how a corrupted store happens |

Each of these is covered by a test that asserts it *fires*:

```bash
make test-deploy          # or: ./deploy/ansible/tests/run-tests.sh -v
```

Those tests exist because a deployment role's safety checks only ever run on the
day someone makes the mistake — a check that quietly stopped matching looks
exactly like one that works. They caught a real defect during development: the
per-instance settings merge had its precedence backwards, so `auth_mode: none`
was being overwritten by the default `key` and the most important security check
never fired.

---

## Verification gate

`mae_daemon_verify: true` (the default) runs after every deploy and **fails the
play** if any of these do not hold:

- every unit is `running` **and** `enabled` (started-but-not-enabled survives
  until the first reboot);
- `mae-daemon --version` reports the version we deployed — this catches a
  symlink that did not move and a rollback that silently did not take, neither
  of which `systemctl start` returning 0 would reveal;
- `doctor --config <instance>` reports *that instance's* socket, which is what
  proves `--config` is honoured at all;
- the collab port is bound, not merely configured;
- `doctor --compare-with` finds no shared resource between any pair of instances;
- each KB socket exists and is a socket.

---

## What must be unique per instance

**Eight** resources. `data_dir` scopes only four of them — `identity_dir`,
`authorized_keys` and `keystore` default to a *shared* `$XDG_DATA_HOME/mae/collab/`
regardless. Two instances separated only by data dir and ports therefore read one
`authorized_keys`, and authorising a peer for staging authorises it for
production.

This role sets all eight explicitly per instance, so it cannot happen here. See
`docs/DAEMON_ADMIN.md` §1 for the full table and for the manual procedure.

---

## Network exposure

**This role does not manage firewall rules.** It refuses an unauthenticated
non-loopback bind, but "authenticated" is not "safe to expose":

- The **KB Unix socket** has no authentication at all, by design — trust is
  filesystem permissions. It must never leave the host.
- The **collab TCP port** in `key` mode is Ed25519 mTLS with an explicit
  allow-list. That is genuinely safe to expose, but you still want a firewall or
  a VPN in front of it.
- The recommended shape is loopback + a reverse proxy or WireGuard, which is
  what the example inventories use.

---

## Onboarding a user

```bash
# On the server, per instance:
sudo -u mae /opt/mae/current/bin/mae-daemon identity \
  --config /etc/mae/daemon-prod.toml            # prints the fingerprint

# The user, on their machine:
mae --collab-identity                            # prints their public key line

# Back on the server:
sudo -u mae /opt/mae/current/bin/mae-daemon authorize \
  --config /etc/mae/daemon-prod.toml "mae-ed25519 AAAA… alice"
```

Then KB sharing proceeds as in `docs/KB_SHARING.md`: the user runs `kb-join`, the
KB owner runs `kb-approve … editor`.

> `--config` is load-bearing on every one of those commands. On mae < 0.15 it was
> **ignored** by all administrative subcommands, so `authorize` wrote into
> whichever instance owned the default config. Deploy 0.15+ before running a
> two-instance host.

---

## Secrets

The role needs none. Each instance's Ed25519 identity is generated on the host
and never leaves it — which also means **it is not in your backups unless you put
it there.** Losing it loses access to every KB that instance shares or has
joined, with no recovery in this version.

Back up `/var/lib/mae/<instance>/collab/id_ed25519` somewhere the deploy pipeline
cannot reach.

If you enable the OAuth listener, its client credentials belong in Ansible Vault
— see `ansible/group_vars/README.md`.

---

## Logging

`ansible.cfg` sets `log_path = ./ansible-deploy.log`, so every run leaves a full
transcript. The host also carries `/opt/mae/DEPLOYED` (version, timestamp,
operator, whether the checksum was verified) so "what is actually running here"
is answerable during an incident without reaching CI.

Per-instance runtime logs go to the journal, tagged:

```bash
journalctl -u mae-daemon@prod -f
journalctl -t mae-daemon@prod --since "1 hour ago"
```

---

## Requirements

- **Control node**: `ansible-core` >= 2.15.
- **Target**: Linux with systemd; EL9, Ubuntu 22.04/24.04 and Debian 12 are the
  declared platforms.
- No external Ansible collections (`ansible.builtin` only).

## Related

- `docs/DAEMON_ADMIN.md` — the manual operator runbook this role automates
- `docs/KB_SHARING.md` — membership, roles, and the join/approve flow
- `SECURITY.md` — the security posture, including what the tiers do not protect
