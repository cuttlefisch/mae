# group_vars

Per-environment variables that are not host-specific.

**Secrets belong in Ansible Vault**, not here in plaintext:

```bash
ansible-vault create group_vars/mae_daemon/vault.yml
ansible-playbook -i inventory/production.yml site.yml --ask-vault-pass
```

The `mae_daemon` role needs no secrets to install and run the daemon — the
Ed25519 identity is generated on the host and never leaves it. Vault is for
things like OAuth client credentials if you enable that listener.
