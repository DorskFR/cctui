# cctuiverse: linking a session to a session on another cctui

A cctuiverse link joins **one** of your sessions to **one** session on another
cctui instance (or to another user's session on the same instance). Once
linked, the remote session is an ordinary peer: `CctuiPeers` lists it as
`remote:<id>` with relation `remote`, `CctuiSend` writes to it, and
`CctuiHistory` reads its transcript if its owner shares it. A room can also
invite a remote session in; the room's server stays authoritative and fans
posts out to every member.

There is no standing trust between instances. Every link has its own Ed25519
keypair on each side, and only an owner creates or accepts one.

## Linking two sessions

1. In the session menu pick **Invite external session…** and choose the label
   the other side will see. You get a link like
   `https://a.example/cctuiverse/join#v1.<link>.<token>.<fingerprint>`, valid
   for 10 minutes and usable once.
2. Send it to the other owner over a channel you trust. **The invite is a
   bearer secret:** for those 10 minutes, anyone who holds it can join (with a
   cctui or with a hand-written client), not only the person you meant. Do not
   paste it where an agent, a bot or a log can read it.
3. They paste it into **New conversation** (the dialog recognises it and joins
   the new session) or into **Join with link…** on an existing session.
4. Both sessions receive a `<cctuiverse-linked>` turn and can talk (if a
   session is not running yet, the turn is delivered as soon as it is). Both
   UIs show the same **safety code** (`xxxx-xxxx-xxxx-xxxx`). Compare it over a
   second channel: it is how you detect that someone else used the invite
   first.

**Invite to room…** in a room's menu works the same way and adds the remote
session to the room.

The part after `#` never reaches a server log. Opening the link in a browser
only explains what to do with it; nothing joins without an owner pasting it into
their own cctui.

## Settings (per link, editable from the link chip)

| Setting | Values | Default |
|---|---|---|
| Inbound | `deliver` each message as a turn · `hold` until you release it | `deliver` |
| Outbound | `tool` (agent uses `CctuiSend`) · `auto` (forward each turn's final reply) · `both` | `tool` |
| Review outbound | approve each outgoing message first | off |
| Share transcript | let the peer read this session with `CctuiHistory` | off |
| Expiry | a date, or never | 24 h after linking |
| Max messages | a cap on messages this side sends | 100 |

Two sessions that both auto-forward answer each other's every turn. The
default cap of 100 messages per side, the 24 h expiry and the inbound limit of
10 messages a minute per session link bound such a loop (there is no smarter
loop detection yet); raise or clear the cap only for a
conversation you are watching. A room's own links cannot hold inbound posts:
they go straight to every member.

## What the protocol guarantees

- A remote message only ever becomes a wrapped `peer` turn
  (`<cross-session-message … origin="remote">`). It cannot answer a permission
  prompt or a question, change settings or run a command.
- Bodies containing any cctui envelope tag (`<cross-session-message`,
  `<cctui-room`, `<cctuiverse`, `<system-reminder`, opening or closing) are
  refused, so a peer cannot forge an envelope.
- A link reaches exactly its bound session (or room). Unknown, closed, expired
  and badly signed links all get the same `404`.
- Either owner closes a link instantly. Closing deletes that side's private key,
  drops every message still held or awaiting review, and tells the peer.
- Only an owner's own credential (browser session or user token) creates, joins,
  changes or closes a link through cctui; machine, dispatcher and ephemeral
  keys never can. This does not make the invite safe to show an agent: the
  invite itself is enough to join (see above).
- A remote room member sees the room's messages from the moment it joined,
  and members by session name only.

Your session keeps its own permission mode, tools and credentials. The peer can
ask your agent for anything your agent may already do; set the session to `ask`
or turn on outbound review if you want a check on that.

## Wire protocol

Servers call each other under `/cctuiverse/v1` (`join`, `links/{id}/messages`,
`links/{id}/close`, `links/{id}/history`, `links/{id}/room`). Every request is
signed with the sending link's key, following an RFC 9421 subset:
`Content-Digest`, `Signature-Input` over `@method`, `@path` and
`content-digest` with `created`, `nonce`, `keyid` and `alg="ed25519"`, and
`Signature`. Requests more than 60 s off the receiver's clock, and any nonce
seen before, are refused. `@authority` is not covered: the receiving link's id
is in the signed `@path`, and the `keyid` must be that link's peer, so a
signature made for one link cannot be replayed against any other link or
server. Signatures survive a TLS-terminating ingress; if the
server is mounted under a sub-path, its `CCTUI_EXTERNAL_URL` must include it.

Outbound messages go through an outbox: delivered at once when possible,
otherwise retried after 5 s, 30 s, 2 min, 10 min and then hourly, and given up
after 24 h. A single `404` from the peer fails only that message, because the
receiver answers `404` for clock skew or a redeploy too; after 5 in a row,
spread over at least 10 minutes and with no success in between, this side
treats the link as gone and closes it. Close notices, including a joiner
withdrawing from a handshake it gave up on, are retried the same way for up to
an hour, and the closing side keeps its key only until the notice settles.
The receiver is idempotent on the message id, and a delivery that failed
because the session was offline is redelivered on the sender's retry. Inbound
bodies are capped at 64 KiB and text at 32 KiB. Each caller address is
throttled before any lookup; each link then accepts 10 messages a minute (60
for room links, which carry every member's posts) and 10 reads a minute,
counted only once the signature verified. Close requests are not counted.

Peer URLs (the inviter's, from the invite, and the joiner's, from the join
request) must be `https` and resolve to public addresses, checked again at
connect time; redirects are not followed.

## Configuration

| Variable | Effect |
|---|---|
| `CCTUI_EXTERNAL_URL` | The base URL invites carry and peers call back. Must be reachable by the other server. |
| `CCTUI_CCTUIVERSE=0` | Disable the feature: the routes answer `404` and the UI hides its entries. |
| `CCTUI_CCTUIVERSE_ALLOW_PRIVATE=1` | Allow `http` and private addresses for peers, for a LAN or a dev setup. |
| `CCTUI_TRUSTED_PROXY_HOPS` | Lets the cctuiverse rate limits see the real client address behind an ingress. Set it: at `0` every caller behind the ingress shares one bucket. |

Private keys are stored encrypted with the vault key (`CCTUI_VAULT_KEY`).

## Known limitations

- **No loop detection.** Two sessions that both auto-forward keep answering
  each other until the message cap (100 per side by default), the inbound
  limit or the expiry stops them.
- **Shared rate limit behind a proxy.** With `CCTUI_TRUSTED_PROXY_HOPS=0`
  behind an ingress, every caller shares one per-address bucket, so one noisy
  client can slow down all cross-instance traffic on the server.
- **A message parked by the agent's harness can be lost.** When a session has
  a question or permission prompt open, the Claude Code daemon holds incoming
  peer messages in memory until it closes. A daemon restart in that window
  loses them, although both servers already recorded them as delivered, and a
  prompt that ends without the usual signal can keep them parked.
- **The "you are linked" turn can arrive late.** If the session is offline it is
  retried every few seconds; in rare races a first message from the peer can
  land before it, and a server crash at the wrong moment can lose it.
- **Holding is not retroactive.** A message whose first delivery failed because
  the session was offline is delivered on the sender's retry even if you switched
  the link to hold in between.
- **Joining briefly holds the invite row.** The inviting server checks the
  joiner's address while holding a lock on the invite; concurrent joins of the
  same invite wait for it.
- **Early test databases.** Migration 171 changed during development; a database
  that applied an early version of it must be recreated.
