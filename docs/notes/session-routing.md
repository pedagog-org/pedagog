# Per-Session Routing (`/s/<session_id>/`)

> Salvaged from the archived `feat/m1-code-server` prototype (tag `archive/m1-code-server`),
> which validated this on Nomad + Traefik 3.1. The Nomad-specific parts are dropped; the
> routing behaviour carries over to Traefik on Kubernetes.
> Sources: `docs/08-design-dynamic-routing.md`, `deploy/traefik/dynamic.yml` (6e44533,
> 01cb165).

## Shape

Each interactive session is served at `/s/<session_id>/` through Traefik. Routes come from
service discovery, not hand-written config: the control plane decides the `session_id`, the
orchestrator registers the running container, and Traefik builds the router. When the
session ends, the route disappears with it. On Kubernetes the equivalent is a per-session
`IngressRoute` (or Service plus labels) owned by the session's resources and garbage-collected
with them.

## Gotchas verified in the prototype

- **`stripPrefix` is required.** code-server emits relative asset paths, so the backend
  must see `/`, not `/s/<id>/`.
- **Bare `/s/<id>` must redirect to `/s/<id>/`.** code-server answers `/` with a relative
  redirect (`./?folder=…`). Without the trailing slash it resolves to `/s/` and the session
  id is lost. One shared middleware, ordered before `stripPrefix` on every session router:

  ```yaml
  http:
    middlewares:
      add-slash:
        redirectRegex:
          # Exact /s/<id> only: sub-paths and already-slashed URLs don't loop.
          regex: "^(https?://[^/]+/s/[^/]+)$"
          replacement: "${1}/"
  ```

- **WebSockets need no extra config.** Traefik proxies the upgrade (101 verified).
- **`exposedByDefault: false`.** Only services that opt in get a route.

## Security

- **Rule injection:** `session_id` is interpolated into a `PathPrefix(...)` rule. Validate a
  strict URL-safe charset (in a `SessionId` newtype) before it reaches any router definition;
  a backtick or quote could otherwise rewrite the rule.
- **The path is not an authority.** `/s/<session_id>/` stays behind ForwardAuth (SEB) and the
  session cookie. See [session-identity.md](session-identity.md).

## Lifecycle

- **Discovery delay:** Traefik picks up a new route on its next refresh. Before redirecting
  the browser, the control plane should poll until the route is live, or show a "starting…"
  state.
- **Rescheduling:** a replacement container for the same session re-registers the same
  route, so the student just reconnects.
- **Health check:** gate the route on code-server's port being ready (left open in the
  prototype).
