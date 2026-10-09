# Agent hosts (`buzz host`)

An agent host is a machine you approve once from Buzz desktop. After that it
shows up in the desktop's "Where to run" list with an online dot, and the
desktop can deploy agents to it with one click. There is no SSH and no
per-host script: the host stays connected to your relay and takes commands
from you over it.

## Setting up a machine

On the machine:

```bash
curl -fsSL <url-of>/scripts/install-buzz-host.sh | bash -s -- --relay wss://your-relay.example
```

The installer:

1. detects the OS and CPU;
2. installs the `buzz` binaries:
   - **macOS:** links the binaries bundled in `/Applications/Buzz.app` into
     `~/.local/bin`, if that build already knows `buzz host`;
   - **otherwise:** downloads the static Sprig tarball (see
     [Sprig tarball](#sprig-tarball));
3. offers to install Node and `@agentclientprotocol/claude-agent-acp`, and checks
   for Claude Code (`claude`);
4. runs `buzz host up`.

`buzz host up` asks for the pairing URI. To get one, open Buzz desktop and go to
Settings → Machines → Add machine. Both screens then show the same pairing code.
Check that they match, click **Approve** in the desktop, and confirm on the
machine. The host then installs itself as a login service and keeps running.

### Sprig tarball

The tarball is built by `scripts/build-sprig.sh` and contains `sprig` plus
links named `buzz`, `buzz-host`, `buzz-acp`, `buzz-agent`, `buzz-dev-mcp`,
`git-credential-nostr` and `git-sign-nostr`. Choose where it comes from with
`--sprig-url` or `BUZZ_SPRIG_URL`; `{target}` in the URL is replaced with the
Rust target triple, for example `x86_64-unknown-linux-musl`.

The default URL,
`https://github.com/block/buzz/releases/latest/download/sprig-{target}.tar.gz`,
is a placeholder until release assets are published. To build your own
tarball:

```bash
TARGET=x86_64-unknown-linux-musl ./scripts/build-sprig.sh 0.1.0
```

## Commands

| Command | What it does |
|---|---|
| `buzz host up [--relay URL] [--uri URI] [--name NAME] [--yes] [--foreground]` | Pairs if needed. Then installs and starts the login service, or with `--foreground` runs the daemon in the terminal. |
| `buzz host pair <uri> [--relay URL] [--name NAME] [--yes]` | Pairing only. |
| `buzz host run` | Runs the daemon in the foreground. This is what the service runs. |
| `buzz host status [--json]` | Shows pairing, the host key, the owner, the relay, tools and each agent's state. |
| `buzz host forget [--yes]` | Stops all agents, removes the service and wipes all host state, including the host key. |
| `buzz host install-service` / `uninstall-service` | Adds or removes the login service. |

The Sprig link `buzz-host <cmd>` is the same as `buzz host <cmd>`.

`--relay` replaces the relay in the pairing URI. After pairing, the host uses
the relay URL from the desktop's grant.

## What runs where

State lives in `~/.config/buzz/host/` (override it with `BUZZ_HOST_HOME`).
Directories are 0700 and files are 0600.

| File | Contents |
|---|---|
| `host.key` | The host secret key H. It is generated on the machine and never leaves it. |
| `owner.json` | The owner pubkey O, the NIP-OA auth tag for H, and the relay URL. |
| `agents/<agent_pubkey>.json` | The agent's workdir and its full `buzz-acp` environment, including its nsec. |
| `seen.json` | Request ids handled in the last 10 minutes, with their acks. |
| `status.json`, `pids.json` | The last status snapshot, and on macOS the agents' process groups. |
| `logs/<agent_pubkey>.log` | Agent output. A log rotates to `.1` once it passes 10 MB. |

### Daemon

The daemon runs as a login service:

- **Linux:** the systemd user unit `buzz-host.service`, with
  `loginctl enable-linger` so it keeps running after you log out.
- **macOS:** `~/Library/LaunchAgents/xyz.buzz.host.plist`. It restarts only after
  a failure (`KeepAlive.SuccessfulExit=false`), so a forgotten host exits and
  stays down.

### Agents

Each agent is `buzz-acp` started with the environment from the deploy. A clean
exit (the relay `!shutdown`) leaves the agent stopped; a crash restarts it.

- **Linux:** one systemd user unit per agent, `buzz-host-agent@<agent_pubkey>`.
  - It uses `Restart=on-failure`, `RestartPreventExitStatus=0` and
    `StartLimitBurst=5` within 10 minutes.
  - It runs `buzz host exec-agent <pubkey>`, which replaces itself with
    `buzz-acp`.
  - The template is named differently from the SSH provider's `buzz-agent@`,
    so both can run on one machine.
- **macOS, or Linux without a systemd user manager:** the daemon supervises
  each agent as a child process.
  - Each agent runs in its own process group.
  - A crash is retried after 2 s, then 4 s, and so on, capped at 5 minutes.
  - After 10 crashes in a row the agent stays `failed` until the next deploy.
  - Process groups are recorded in `pids.json`, so a restarted daemon stops
    orphans before it starts fresh copies.

The search `PATH` for agents puts `~/.local/bin`, `~/.npm-global/bin`,
`~/.cargo/bin` and `~/.bun/bin` ahead of the inherited `PATH`, so tools
installed per user are found even under a service manager.

## Protocol (contract v1)

The host uses no new event kinds and needs no relay changes. Every message
travels over existing kinds.

### Identities

| Key | Who holds it | Role |
|---|---|---|
| H | The machine (`host.key`) | The host. The desktop issues it a NIP-OA auth tag with empty conditions, so H is an agent of O. A closed relay therefore admits H through O's membership, and H's NIP-42 AUTH records `agent_owner_pubkey(H) = O`. That record is what lets observer frames pass the relay's `is_agent_owner(H, O)` check. |
| O | The desktop user | The owner. |
| Agent keys | Unchanged | They reach H only inside an encrypted `host.deploy`, are stored 0600 and are never logged. |

### Pairing (NIP-AB, both payloads `PayloadType::Custom`)

The desktop is the source and shows the URI. The host is the target. One
session carries two payloads, using `PairingSession::send_return_payload`
(target) and `send_reply_payload` (source) in
`crates/buzz-core/src/pairing/session.rs`.

```text
Desktop (source)                         Host (target)
new_source → URI + code                  new_target(uri) → offer
handle_offer → code                      prints code
[Approve] confirm_sas  ───────────────►  handle_sas_confirm
                                         [user confirms] confirm_target_sas
handle_return_payload  ◄───────────────  send_return_payload(hello)
send_reply_payload(grant) ────────────►  handle_payload → verify grant
handle_complete        ◄───────────────  send_complete
```

```json
{"type":"buzz-host-hello","v":1,"host_pubkey":"<hex>","name":"<str>","os":"linux|macos","arch":"<str>","version":"<str>"}
{"type":"buzz-host-grant","v":1,"owner_pubkey":"<hex>","auth_tag":"[\"auth\",\"<O>\",\"\",\"<sig>\"]","relay_url":"wss://..."}
```

The host refuses a grant unless all of these hold:

- the auth tag verifies for H;
- the tag's signer equals `owner_pubkey`;
- the relay URL is `ws://` or `wss://`.

### Frames (kind 24200, NIP-44 content, `created_at` within ±300 s)

| Direction | Signer | Tags |
|---|---|---|
| Control (owner → host) | O | `[["p",H],["agent",H],["frame","control"]]` |
| Telemetry (host → owner) | H | `[["p",O],["agent",H],["frame","telemetry"]]` |

The host subscribes to `{"kinds":[24200],"#p":[H],"since":now-300}`.

The host drops a control frame in any of these cases:

- the signature is bad;
- the sender is not O;
- the `p`, `agent` and `frame` tags are not exactly as in the table;
- it is outside ±300 s.

A `request_id` that was already handled is not run again; the host resends the
cached ack. The ledger of handled ids is persisted, bounded to 1024 entries and
10 minutes.

### Control frames and their replies

| Control (`type`) | Fields | Reply |
|---|---|---|
| `host.deploy` | `request_id, agent_pubkey, agent_nsec, auth_tag, relay_url, workdir (null → ~/buzz-agents/<agent_pubkey>), env, launch{command,args,env,policy_env,owner_pubkey}`, plus optional `respond_to` and `respond_to_allowlist` | `host.ack` |
| `host.undeploy` | `request_id, agent_pubkey` | `host.ack` |
| `host.status` | `request_id` | `host.status` with the same `request_id` |
| `host.forget` | `request_id` | `host.ack`; then the host deletes `host.key` and exits 0 |

Telemetry payloads:

```json
{"type":"host.ack","request_id":"...","ok":true}
{"type":"host.ack","request_id":"...","ok":false,"error":"agent command \"goose\" is not installed on this host"}
{"type":"host.status","request_id":"...","name":"...","os":"linux","arch":"x86_64","version":"0.1.0",
 "agents":[{"agent_pubkey":"...","state":"running|stopped|failed","since":1760000000}],
 "claude":{"installed":true,"auth_ok":null},
 "tools":{"node":true,"claude_agent_acp":true,"buzz_acp":true}}
```

The host also sends `host.status` without a `request_id` when it connects and
every 5 minutes. It publishes kind:20001 `online` presence every 60 s; the
relay's presence TTL is 180 s.

### Deploy behavior

- **Environment.** The precedence matches `buzz-backend-ssh` and the Kubernetes
  provider: `launch.policy_env`, then `launch.env` (or the flat `env` when there
  is no `launch`), then keys the host sets itself.
  - The host always sets `BUZZ_RELAY_URL`, `BUZZ_PRIVATE_KEY`,
    `NOSTR_PRIVATE_KEY`, `BUZZ_AUTH_TAG`, `BUZZ_ACP_AGENT_OWNER`,
    `BUZZ_ACP_AGENT_COMMAND` (the command's basename),
    `BUZZ_ACP_AGENT_ARGS` and `BUZZ_ACP_MCP_COMMAND`, replacing any value sent.
  - `BUZZ_ACP_NO_PRESENCE` is refused.
- **Validation.** The host refuses a deploy in any of these cases:
  - `agent_nsec` does not belong to `agent_pubkey`;
  - `agent_pubkey` is not 64 lowercase hex characters;
  - `auth_tag` does not authorize the agent;
  - `buzz-acp` or the agent command is missing.

  None of these error messages contains a secret.
- **Folder.** `workdir` is the folder the agent runs in on the machine. The
  desktop sends the folder saved on the agent ("Folder on <machine>" when
  creating it, or **Save folder** in its edit dialog), or `null` when none is
  set. The host expands `~` to its own home, resolves a relative path against
  that home, creates the folder if it is missing, and starts the agent there;
  `null` means `~/buzz-agents/<agent_pubkey>`. The desktop only checks the
  path's shape (at most 300 characters, no line breaks or NUL), never whether
  it exists on the desktop's own computer. Changing the folder of a deployed
  agent saves it and redeploys the agent to the same machine; a failed
  redeploy keeps the new folder saved and pending, and the desktop retries it
  the next time the community loads. Moving an agent to another machine keeps
  its folder.
- **Idempotency.** A deploy for an agent that is already on the host rewrites its
  config and restarts it, giving one instance and one config file. A failed
  *first* deploy leaves no config behind.
- **Undeploy** stops the agent, removes its service and deletes its config. An
  unknown agent counts as success.
- **Forget** undeploys every agent and wipes the owner, the ledger, status, pids
  and logs. Once the ack has been sent, it publishes
  `offline` presence, deletes `host.key` and exits.

### Stopping an agent

Stopping an agent is unchanged. The desktop sends the relay `!shutdown`, and
`buzz-acp` exits 0 and stays down. `host.undeploy` is for removing an agent from
a machine, not for stopping it.

## Testing

```bash
cargo test -p buzz-host
# End-to-end against a local relay (see TESTING.md for running one):
BUZZ_HOST_E2E_RELAY=ws://localhost:3030 BUZZ_HOST_E2E_BIN=target/debug/buzz \
BUZZ_HOST_E2E_ACP=target/debug/buzz-acp \
  cargo test -p buzz-host --test e2e_local_relay -- --ignored --nocapture
```
