# Session Identity: `session_id` vs Container Token

> Salvaged from the archived `feat/m1-code-server` prototype (tag `archive/m1-code-server`).
> Sources: `docs/08-design-dynamic-routing.md` §2–3, `docs/02-design-exam-system.md` §14
> (d81001c).

Two distinct values, which early designs conflated:

| Value | Visibility | Appears in | Role |
| --- | --- | --- | --- |
| **`session_id`** | Public, opaque, stable | URL `/s/<session_id>/`, routing, dashboard, logs, audit | **Identifies** a session. Carries no authority. |
| **container token** | Secret, high-entropy | Only the control plane and that session's daemon (injected as a secret, mounted read-only to the daemon user) | **Authenticates** the session's control channel, in both directions. |

- Because `session_id` is public, it can be used directly as the route; no separate
  `route_id` is needed. It should still be high-entropy enough that sessions can't be
  enumerated.
- Model them as two newtypes: `SessionId` (may be displayed and logged) and
  `ContainerToken` (never `Display`, never logged, never in a URL, label or tag).

## The token authenticates both directions

The token is shared by exactly the control plane and one session's daemon, so it is a bearer
credential for both directions:

- **daemon → control plane:** heartbeat, submit, archive. Proves "I am session X's daemon".
- **control plane → daemon:** end the session, update the deadline. Proves "I am the control
  plane, authorized for session X".

Properties:

- **Blast radius:** a leaked token compromises one session, not the fleet. There is no global
  control-plane credential on the wire.
- **Defense in depth:** restrict who can reach the daemon's control port to the control plane
  only (on Kubernetes, a NetworkPolicy ingress rule; the prototype used nftables), *and*
  require the token, so a network misconfiguration alone can't issue commands.
- **Transport:** use TLS on the private network so the bearer token can't be sniffed.
- **Addressing:** the control port is private and never exposed through Traefik. Traefik
  stays the only public tier.
