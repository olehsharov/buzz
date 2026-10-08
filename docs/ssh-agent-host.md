# Running managed agents on an SSH host

`scripts/buzz-backend-ssh` is a Buzz Desktop backend provider (see
[remote-agents.md](remote-agents.md), protocol version 1) that runs a managed
agent on any Linux machine you can reach over SSH. Each agent runs `buzz-acp`
under a systemd user unit, `buzz-agent@<id>`, on that host.

## Host requirements

- Linux with systemd user services. Run `loginctl enable-linger <user>` so
  units keep running after you log out.
- `python3`.
- `buzz-acp`, `buzz-dev-mcp`, and the agent command (for example `claude-agent-acp`
  or `goose`) on the host's `PATH`. The provider looks in `~/.local/bin`,
  `~/.npm-global/bin`, `~/.cargo/bin`, `/usr/local/bin`, `/usr/bin`, and `/bin`.
- Whatever credentials the agent command needs (for example, a logged-in
  Claude Code or a provider API key in the agent's env).

## Desktop setup

1. Make the host reachable non-interactively with a key: `ssh -o BatchMode=yes
   <host> true` must succeed. An alias from `~/.ssh/config` is fine.
2. Install the provider where the desktop discovers it, as an executable named
   `buzz-backend-<id>`:

   ```bash
   install -m 755 scripts/buzz-backend-ssh ~/.local/bin/buzz-backend-ssh
   ```

3. Choose the host. Either set it in the provider's `host` field when you pick
   this backend for an agent, or export `BUZZ_SSH_AGENT_HOST` in the
   environment the desktop launches providers with. There is no default.
   `workdir` (default `~`) is the agent's working directory on the host.

## What deploy does

The provider builds the harness environment, including the agent's private
key, and sends it to the host over SSH **stdin**, never on the command line. On
the host it:

- writes `~/.config/buzz/agents/<id>.json` (mode 0600) with the workdir and env;
- installs `~/.local/bin/buzz-agent-run` and
  `~/.config/systemd/user/buzz-agent@.service`;
- runs `systemctl --user daemon-reload`, `enable`, and `restart` for the unit.

`<id>` is the first 16 hex characters of the SHA-256 of the agent's key, so the
handle is stable per identity without revealing the key. Logs go to
`~/buzz-agents/<id>.log`.

`scripts/buzz-agent-run` and `scripts/buzz-agent@.service` are reference copies
of the two files deploy installs, useful for inspecting or setting up a host by
hand. The provider writes them itself, with the host's `PATH` expanded.

Stopping uses the relay `!shutdown` the desktop already sends: `buzz-acp` exits
0, and `Restart=on-failure` keeps the unit down. To remove an agent from the
host, run `systemctl --user disable --now buzz-agent@<id>` and delete its
`~/.config/buzz/agents/<id>.json`.

## Security notes

- The agent JSON files on the host hold private keys. Never copy or commit them.
- `BUZZ_ACP_NO_PRESENCE` is refused: presence is the only signal that a remote
  agent is alive.
- Errors come back in-band as `{"ok": false, "error": ...}` with exit code 0,
  matching `buzz-backend-kubernetes`.
