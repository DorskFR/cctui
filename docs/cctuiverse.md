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
2. Send it to the other owner over a channel you trust.
3. They paste it into **New conversation** (the dialog recognises it and joins
   the new session) or into **Join with link…** on an existing session.
4. Both sessions receive a `<cctuiverse-linked>` turn and can talk. Both UIs
   show the same **safety code** (`xxxx-xxxx-xxxx-xxxx`); read it to each other
   if the invite travelled over a channel you do not trust.

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
| Max messages | a cap on messages this side sends | none |

## What the protocol guarantees

- A remote message only ever becomes a wrapped `peer` turn
  (`<cross-session-message … origin="remote">`). It cannot answer a permission
  prompt or a question, change settings or run a command.
- Bodies containing any cctui envelope tag (`<cross-session-message`,
  `<cctui-room`, `<cctuiverse`, `<system-reminder`, opening or closing) are
  refused, so a peer cannot forge an envelope.
- A link reaches exactly its bound session (or room). Unknown, closed, expired
  and badly signed links all get the same `404`.
- Either owner closes a link instantly. Closing deletes that side's private key
  and tells the peer.
- Only an owner's own credential (browser session or user token) creates, joins,
  changes or closes a link; an agent's machine key never can. An agent that
  reads an invite cannot use it.

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
seen before, are refused. Signatures survive a TLS-terminating ingress; if the
server is mounted under a sub-path, its `CCTUI_EXTERNAL_URL` must include it.

Outbound messages go through an outbox: delivered at once when possible,
otherwise retried after 5 s, 30 s, 2 min, 10 min and then hourly, and given up
after 24 h. The receiver is idempotent on the message id. Inbound bodies are
capped at 64 KiB, text at 32 KiB, and each link at 30 requests a minute.

Peer URLs (the inviter's, from the invite, and the joiner's, from the join
request) must be `https` and resolve to public addresses, checked again at
connect time; redirects are not followed.

## Configuration

| Variable | Effect |
|---|---|
| `CCTUI_EXTERNAL_URL` | The base URL invites carry and peers call back. Must be reachable by the other server. |
| `CCTUI_CCTUIVERSE=0` | Disable the feature: the routes answer `404` and the UI hides its entries. |
| `CCTUI_CCTUIVERSE_ALLOW_PRIVATE=1` | Allow `http` and private addresses for peers, for a LAN or a dev setup. |
| `CCTUI_TRUSTED_PROXY_HOPS` | Lets the join rate limit see the real client address behind an ingress. |

Private keys are stored encrypted with the vault key (`CCTUI_VAULT_KEY`).
