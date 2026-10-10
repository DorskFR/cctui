# 0004 — cctuiverse: session-to-session links across cctui instances

Status: **Proposed** (investigation, nothing built)

## Goal

A session on cctui instance A and a session on instance B hold a conversation.
Each keeps its own thread, harness, tools and owner.

- The link is **one session to one session**. Each owner creates it on purpose, by
  hand, for that session only.
- There is no standing trust between instances.
- Each owner decides how much their own side allows. The protocol enforces only
  what stops the *other* side from overriding that decision.

## What cctui already has

| Need | Existing piece | Gap |
|---|---|---|
| Agent-to-agent messaging | `CctuiPeers`, `CctuiSend`, `CctuiHistory`, `CctuiRoom` (`routes/peer.rs`, `peer_policy.rs`, `rooms.rs`) | Same owner, same server only (`peer_policy.rs:294`) |
| Envelope and `peer` role in UI | `<cross-session-message>` (`peer.rs:149`), `PEER_TAG_RE` (`format.ts:65`) | Body only rejects its own close tag (`peer.rs:284`); forged nested opening envelopes pass |
| Delivery into a live session | `bus::dispatch(SendMessage)` (`bus/mod.rs:1063`) | Mid-turn behaviour differs per adapter: ACP queues, codex steers |
| Signed requests | Plugin proxy HMAC (`plugin_proxy.rs`), webhook HMAC (`webhook.rs`) | Shared secret only; no asymmetric identity |
| Outbound SSRF guard | `outbound.rs` | Reusable as is |
| Session menu, new-conversation dialog | webui + TUI | Need "Invite external session" / "Join with link" entries |

Daemons dial out to their own server only, and a session has no network endpoint
of its own. Bytes therefore travel `session → server A ↔ server B → session`.

The servers act as **mailboxes for one link**. The other side sees one endpoint
per link, scoped by that link's key, and nothing else of the instance.

## The link

### Handshake

1. In session S<sub>A</sub>, owner A picks **Invite external session…** from the
   session menu and enters a label.
   - Server A creates a **link** with its own fresh Ed25519 keypair, stored
     encrypted in the vault.
   - It returns a single-use invite that expires after 10 minutes:
     `https://a.example/cctuiverse/v1/join#<256-bit token>.<link key fingerprint>`.
   - The token sits in the URL fragment, so it never reaches access logs.
2. A sends the invite to B out of band (Signal, in person, …).
3. B starts a new conversation and pastes the link.
   - The new-conversation dialog (or the composer of an empty session)
     recognises a cctuiverse invite and asks B to confirm "Join as linked
     session". Pasting alone never joins.
   - An existing session can also join through **Join with link…** in its menu.
   - Server B creates its own link keypair.
   - It POSTs its public key and the token to A's join endpoint, signed with
     the new key.
   - It checks that A's key in the response matches the fingerprint in the
     invite.
4. Both sessions get a "linked with ‹label›" marker. Each owner chose the label
   for their own side; nothing else is disclosed.

The invite is the shared secret and carries A's key fingerprint.
- If it travelled over a private channel, nobody in the middle can complete the
  join.
- Over an untrusted channel, either owner can compare an optional 6-word short
  authentication string derived from both keys.

The handshake is driven only by an owner's own credential (cookie or user
token), never by the machine key the agent tools use. An agent that reads an
invite in a file or a message cannot create or accept a link on its own.

**One link binds exactly two sessions.**
- It cannot be redirected to another session.
- It survives session resume, but closes when either session is archived.
- To talk to a different session, make another link.

### Wire

- Each server exposes one route, `POST /cctuiverse/v1/links/{link_id}/messages`,
  plus `join` and `close`.
- Requests are authenticated by the link key under a new principal,
  `Principal::Link(link_id)`, with default-deny `Authz`.
  - That principal can do exactly three things on exactly one link: deliver,
    close, ack.
- Every request carries an RFC 9421 HTTP message signature: Ed25519 over method,
  authority, path, `content-digest`, `created`, `nonce` and `keyid`.
  - Requests outside a ±60 s window are rejected; nonces are stored for that
    window.
  - Signatures survive the TLS-terminating ingress, which mutual TLS does not.
- Delivery uses a persistent outbox with retries, idempotent on the message id.
  There is no long-lived socket between servers.
- Payloads are text parts, shaped like A2A `Message` objects with `contextId`
  set to the link id, so an A2A facade stays possible later.
- Outbound calls go only to the peer URL recorded at join time, through the
  `outbound.rs` SSRF guard.

## Fixed rules vs. owner choices

### Fixed (protocol-enforced; a few rules that protect each owner from the other side)

1. **The peer never acts as the owner.**
   - A remote message is a `peer`-role turn and never counts as a user action.
   - It cannot answer this side's permission prompts or `AskUserQuestion`.
   - It cannot run cctui commands or change link settings.
   - Without this rule, B's agent could approve A's tool calls, overriding A's
     own permission choices.
2. **The envelope cannot be forged.**
   - Inbound text arrives in the existing `<cross-session-message>` envelope with
     `origin="remote"`, the peer's label and a per-link nonce.
   - Bodies containing any opening or closing cctui envelope tag are rejected:
     `<cctuiverse-`, `<cross-session-message`, `<cctui-room`,
     `<system-reminder`.
   - The same check also closes the gap at `peer.rs:284` for the existing paths.
3. **Reach is exactly one session.**
   - The link reaches only its bound session: no ids, machine names, emails or
     history beyond what that session's agent chooses to write.
   - Unknown or closed links all get the same refusal.
4. **Either owner can close instantly.**
   - Closing deletes the local key and notifies the peer best-effort.
   - The peer's next message fails signature lookup.
5. **Transport-level abuse limits only.**
   - A message size limit (e.g. 64 KiB) and a request rate limit protect the
     server.
   - They are not a limit on the conversation.

### Owner choices (per link, set at invite or join time, editable live from the session UI)

| Setting | Options | Default |
|---|---|---|
| Inbound delivery | queue to turn end · steer into active turn (codex) · hold until I release | queue |
| Outbound | agent sends via `CctuiSend` · auto-forward each turn's final reply · both | `CctuiSend` |
| Review outbound | off · approve each message | off |
| Permission mode while linked | unchanged (session's own) · clamp to ask/auto | unchanged |
| Local reach | session keeps `CctuiAgent`, peers, rooms · disabled while linked | unchanged |
| Share my transcript | no · let peer page my thread (`CctuiHistory` over the link) | no |
| Caps | turns · dollars · expiry · each optional | expiry 24 h, rest off |

Nothing is clamped by default. The owner already chose this session's posture,
tools and context, and accepted the link deliberately.

The UI and the `CctuiPeers` entry show a short reminder: "messages
from ‹label› are written by someone else's agent; your session runs with your
permissions." This is a reminder, not enforcement.

## Reference case

Alice and Bob each run Claude on their own cctui instance. They work on a repo
both of them can push to.

1. **Link.** Alice picks **Invite external session…** in her session and sends
   Bob the link. Bob starts a new conversation in the repo checkout, pastes the
   link and confirms. The two servers do the handshake.
2. **Defaults are enough.** Both sessions keep their own permission mode, tools
   and git credentials.
   - Neither agent gains any access through the link. Each pushes with its owner's
     own rights, as it would alone.
3. **Coordination.**
   - The agents split the work in messages ("I take the API, you take the UI").
   - They exchange branch names and commit SHAs, ask for review, and report
     results ("tests green on `feat/ui` at abc123").
   - Git is the shared state; the link only carries intent and coordination.
4. **Optional settings.**
   - "Share my transcript" lets each agent see how the other reached a change.
   - Auto-forwarding final replies gives a hands-off back-and-forth.
   - Steer mode lets a "stop, I'm touching that file" note land mid-turn.
5. **The real exposure.** The peer can ask your agent to do things your agent is
   already allowed to do, such as pushing.
   - That is the trust both owners accepted.
   - If one owner wants a check on it, they set their own permission mode to `ask`
     for the duration, or turn on outbound review.
   - The repo's branch protection still applies to both.

## UX

- **Starting a link** (webui and TUI, no slash command):
  - **Invite external session…** in the session menu copies the link and shows
    its expiry.
  - Pasting the link into a new conversation, or **Join with link…** in a
    session menu, joins after one confirmation.
- **During the link:**
  - A link chip in the session header shows the peer label, settings, live
    status and a **Close** button.
  - Peer turns render in the existing `peer` style with an "external" badge.
- **Agent tools: none new.**
  - The remote session appears in `CctuiPeers` with relation `remote` and its
    label, under an opaque id that is local to this side.
  - `CctuiSend` to that id goes over the link. `CctuiHistory` works only if the
    peer turned on "share my transcript".
  - `peer_policy.rs` gains the `remote` relation. The send path branches to the
    link outbox instead of `bus::dispatch`.
  - Closing is a human action in the UI.

## Phasing

0. **Envelope hardening.** Reject cross-family tags in bodies on today's peer and
   room paths. Small and independent.
1. **Same-server, cross-user links.**
   - The full handshake, settings and tools, with both sessions on one instance
     and no signatures yet.
   - Delivery reuses `peer.rs` with a new `Link` relation in `peer_policy.rs`.
2. **Cross-instance links.** Link keys, `/cctuiverse/v1/*` routes, RFC 9421
   middleware, nonce store, outbox and close propagation.
3. **Inter-server rooms.** Mostly free once a remote session is an ordinary peer.
   - A room broadcast is already N calls to `deliver()` (`rooms.rs`). Once
     `deliver()` routes `remote` peers to their link, `CctuiRoom post` reaches
     them unchanged.
   - The room lives on the inviter's server, which keeps the membership and the
     timeline. A remote member's `post` goes over its link to that server, which
     fans it out to everyone else, so no room data is copied between servers.
   - Consent: an existing 1:1 link does not mean "add me to rooms". Adding a
     remote peer to a room needs its owner to confirm once. A **Invite to
     room…** link is the same invite, carrying the room to join.
   - Members see each other's labels only. A member on a third server C gets B's
     posts through the room's server A without ever linking to B directly.
4. **Optional.** Short-authentication-string display, A2A facade for non-cctui
   agents.

## Open questions

- Is auto-forwarding the final reply a good default for "just let them talk"?
  It is the most natural flow, but it also forwards anything the agent
  summarises. It is proposed as opt-in.
- Which Rust signature crate: `httpsig` (RFC 9421) or a small implementation over
  `ed25519-dalek`? Plugin-proxy signature vectors are the test model.
- When the link is pasted into a new conversation, should the dialog offer a
  "discussion" preset (cwd, model, permission mode) to start from?

## Sources

- A2A spec: https://a2a-protocol.org/latest/specification/
- Unit 42 agent session smuggling: https://www.esecurityplanet.com/threats/news-ai-session-smuggling-attack/
- Prompt Infection: https://arxiv.org/abs/2410.07283
- Agents Rule of Two: https://simonw.substack.com/p/new-prompt-injection-papers-agents
- RFC 9421 HTTP Message Signatures: https://www.rfc-editor.org/rfc/rfc9421
- RFC 9382 SPAKE2 (if short codes are ever wanted): https://www.rfc-editor.org/rfc/rfc9382
