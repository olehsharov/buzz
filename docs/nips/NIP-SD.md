NIP-SD
======

Stream Drafts (Live Reply Streaming)
------------------------------------

`draft` `optional` `client`

**Depends on**: NIP-01 (basic event format), NIP-10 (thread markers), NIP-29 (`h`-tag channel scoping)

## Abstract

This NIP defines an ephemeral "ghost message" that shows an agent's reply in a
channel while the agent is still writing it. The writer publishes a sequence of
`kind:20003` stream-draft frames, each carrying the cumulative markdown
snapshot of the reply so far plus a coarse status (`thinking`, `tool`,
`writing`). The durable reply is still an ordinary `kind:9` message; drafts
are never stored and require no relay support beyond NIP-01 ephemeral fan-out
of `h`-scoped events.

## Motivation

An agent turn can take tens of seconds. Typing indicators (`kind:20002`) say
*that* an agent is working, not *what* it is about to say or which tool it is
running. Stream drafts let a client render the reply as it forms and show the
current tool, without changing the durable message model: a client that
ignores `kind:20003` loses nothing.

## Non-Goals

- Drafts are not messages. They are never stored, edited, reacted to, or
  threaded. Only the final `kind:9` is durable.
- Drafts never carry model reasoning. `thinking` frames are pure status.
- This NIP defines no relay behavior. Relays fan out `kind:20003` like any
  other `h`-scoped ephemeral event (membership-gated on publish and receive).

## Terminology

This document uses MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY as defined in
RFC 2119.

- **writer**: the pubkey producing the reply (normally an agent).
- **stream**: one reply being written, identified by a UUID v4 stream id.
- **frame**: one `kind:20003` event of a stream.
- **scope**: the place the reply will land — a channel (`h`) and, when the
  reply targets a thread, that thread's root.

## Kinds

| Kind | Name | Signer | Storage | Purpose |
|------|------|--------|---------|---------|
| `20003` | Stream Draft | writer | ephemeral (never stored) | Live snapshot of a reply being written |

## Event Format

```json
{
  "kind": 20003,
  "pubkey": "<writer pubkey>",
  "content": "<cumulative markdown snapshot, or empty>",
  "tags": [
    ["h", "<channel uuid>"],
    ["stream", "<uuid v4, one per reply stream>"],
    ["seq", "<n>"],
    ["e", "<root id>", "", "root"],
    ["e", "<parent id>", "", "reply"],
    ["status", "thinking|tool|writing|final|abandoned"],
    ["label", "<short tool title>"]
  ]
}
```

| Tag | Required | Meaning |
|-----|----------|---------|
| `h` | yes | Channel the reply is written into. |
| `stream` | yes | Stream id. Every frame of one reply, and the final `kind:9`, carry the same value. |
| `seq` | yes | Decimal frame number, starting at `1`, strictly increasing per stream. Gaps are allowed (frames may be dropped in transit). |
| `e` | when threaded | The NIP-10 markers the final reply will carry: `["e", root, "", "reply"]` for a direct reply to the thread root, or `root` + `reply` markers for a nested reply. Absent for a top-level reply. |
| `status` | yes | See below. |
| `label` | no | Only with `status` = `tool`: a short tool title, at most 80 characters. |

`content` is the **full** markdown text of the reply so far (a snapshot, not a
delta), at most 64 KiB. A writer that exceeds the cap truncates on a UTF-8
character boundary and appends `…`. `content` MAY be empty for `thinking` and
`tool` frames and MUST be empty for `final` and `abandoned`.

### Status values

| Status | Meaning |
|--------|---------|
| `thinking` | The writer is reasoning. Reasoning text is never published. |
| `tool` | The writer started a tool call; `label` names it. |
| `writing` | The writer is producing reply text; `content` is the snapshot. |
| `final` | The stream ended. If a reply was posted it is the `kind:9` carrying the same `stream` tag. |
| `abandoned` | The stream was discarded (cancelled, errored, or cut short). No reply is posted by the stream. |

Writers SHOULD re-send the current frame (with a new `seq`) at least every 5
seconds while a stream is active and no other frame is due, so a long-running
tool does not let clients expire the ghost.

## Final Reply

The durable reply is a normal `kind:9` in the same scope. When the writer
posts the reply as the outcome of the stream it MUST add
`["stream", <same uuid>]` to that `kind:9`. After the final `kind:9` (or when
discarding the stream) the writer sends one last frame with `status` = `final`
(reply posted, or the writer replied by other means) or `abandoned`, and empty
content.

## Client Behavior

- Track the latest frame per `(pubkey, stream)`. Drop any frame whose `seq` is
  less than or equal to the last one seen for that pair.
- Render the latest `content` as a provisional message from `pubkey` in the
  scope given by `h` and the `e` markers.
- Remove the ghost when any of these happens:
  - a frame with `status` `final` or `abandoned` arrives;
  - a `kind:9` from the same pubkey arrives in the same scope;
  - 15 seconds pass without a frame for that stream.
- Never persist drafts or treat them as messages (no unread counts, no
  notifications, no reactions).

## Writer Rate

Frames share the relay's WebSocket flood budget with every other event the
writer sends. Writers SHOULD coalesce text so a stream averages a few frames
per second (Buzz's harness: first frame immediately, then at most one frame
per 200 ms carrying at least 24 new characters or a status change, and at most
6 frames per second across all of one agent's concurrent streams).

## Relay Compatibility

No relay changes are required. A Buzz relay handles `kind:20003` on the generic
ephemeral path: it verifies the signature, requires the authenticated pubkey
to match, requires `MessagesWrite` scope, checks community fencing and channel
membership for the `h` tag, then fans the event out to the channel's current
members without storing it. Ephemeral kinds are exempt from the per-minute
message quota but count toward the per-pubkey WebSocket flood budget.
