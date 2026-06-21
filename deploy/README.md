# deploy — running the routing locally

Per-session coding sandboxes (`codebox`) are scheduled by **Nomad** on **rootless podman** and
fronted by a single **Traefik** edge proxy. Each session gets its own `/s/<session_id>/` route that
Traefik discovers from Nomad's service registry — no static route config, nothing to edit when
sessions come and go.

## Layout

| Path | What it is |
|---|---|
| `nomad/agent.hcl` | The Nomad agent config (server + client, single node). Committed so the routable-IP setup is reproducible. |
| `nomad/codebox.nomad.hcl` | The per-session job: one container running the editor, advertising its route via Traefik tags. |
| `traefik/traefik.yml` | The edge proxy config. One standalone instance for the whole cluster; not part of any job. |
| `traefik/dynamic.yml` | Shared Traefik middlewares (e.g. `add-slash`) referenced by every session router as `@file`. |

## How it fits together

```
                 browser
                    │  http://<host>:8080/s/<session_id>/
                    ▼
        ┌───────────────────────┐
        │   Traefik (container)  │   own network namespace (pedagog-edge),
        │   binds :8080          │   NO host networking
        └───────────┬───────────┘
            │                 ▲
   discover │ tags+IP:port    │ proxy to backend
   (poll)   │                 │ <node-IP>:<dynport>
            ▼                 │
   host.containers.internal   │            ┌──────────────────────────────┐
            │ :4646           └────────────│  codebox alloc (podman)       │
            ▼                              │  editor on :8080 → dyn host   │
   ┌────────────────────────┐  schedules  │  port, advertised as          │
   │  Nomad agent (host)     │────────────▶│  <node-IP>:<dynport>          │
   │  server + client        │             └──────────────────────────────┘
   │  advertises {{GetPrivateIP}}
   └────────────────────────┘
```

Two addresses make this work without `--network host`:

1. **Nomad advertises a routable IP, not loopback.** `agent.hcl` uses go-sockaddr templates
   (`{{ GetPrivateIP }}`), so the registry hands Traefik a real `<node-IP>:<dynport>` backend that
   is reachable from another network namespace. No hardcoded IP — each node auto-detects its own.
2. **Traefik reaches the Nomad API by DNS.** It dials `host.containers.internal:4646` (podman's
   host-gateway name), so it needs no host networking and no literal address either.

This is the same shape as a real multi-node cluster: Traefik talks to nodes purely by IP/DNS over a
network. Adding more nodes later changes nothing about Traefik — they just auto-advertise their own
IPs.

## Two-level model

| Level | Command | What varies |
|---|---|---|
| Per **assignment** | `nomad job run -var image=… -var cpu=… -var memory=… nomad/codebox.nomad.hcl` | image (from the registry) + quotas |
| Per **student session** | `nomad job dispatch -meta session_id=… codebox` | the session id |

`run` registers the template; `dispatch` launches one instance of it. See
`docs/08-design-dynamic-routing.md` for the full design.

---

## Setup (one time)

### 1. podman (rootless) + its API socket

Nomad's podman driver talks to the rootless podman API socket.

```bash
sudo apt-get install -y podman          # or your distro's package
systemctl --user enable --now podman.socket
ls -l "$XDG_RUNTIME_DIR/podman/podman.sock"   # should exist
```

> The socket path is per-user: `/run/user/<uid>/podman/podman.sock`. If your uid is not 1000,
> update `socket_path` in `nomad/agent.hcl`.

### 2. Nomad

```bash
# Grab the binary (adjust version/arch) and put it on PATH.
mkdir -p ~/.local/bin
curl -fsSL https://releases.hashicorp.com/nomad/2.0.3/nomad_2.0.3_linux_amd64.zip -o /tmp/nomad.zip
unzip -o /tmp/nomad.zip -d ~/.local/bin
nomad version
```

### 3. nomad-driver-podman plugin

```bash
mkdir -p ~/pedagog-tools/plugins
curl -fsSL https://releases.hashicorp.com/nomad-driver-podman/0.6.4/nomad-driver-podman_0.6.4_linux_amd64.zip -o /tmp/driver.zip
unzip -o /tmp/driver.zip -d ~/pedagog-tools/plugins
```

### 4. The base image

```bash
podman build -t pedagog-base:dev images/base
```

---

## Run

```bash
# 1. Nomad agent (server+client) with the committed config. Runs as your user (rootless).
nomad agent -plugin-dir="$HOME/pedagog-tools/plugins" -config=deploy/nomad/agent.hcl

# Point the CLI at the advertised (non-loopback) API address.
export NOMAD_ADDR="http://$(nomad node status -json 2>/dev/null | grep -o '"HTTPAddr":"[^"]*"' | head -1 | cut -d'"' -f4)"
# (or just: export NOMAD_ADDR=http://<your-LAN-ip>:4646)

# 2. The edge proxy — its OWN network, no host networking. Publishes :8080.
podman network create pedagog-edge        # one time
podman run -d --name pedagog-traefik \
  --network pedagog-edge \
  --add-host host.containers.internal:host-gateway \
  -p 8080:8080 \
  -v "$PWD/deploy/traefik/traefik.yml:/etc/traefik/traefik.yml:ro" \
  -v "$PWD/deploy/traefik/dynamic.yml:/etc/traefik/dynamic.yml:ro" \
  docker.io/traefik:v3.1

# 3. Register the assignment's job, then dispatch a session.
nomad job run deploy/nomad/codebox.nomad.hcl
nomad job dispatch -meta session_id=test codebox
```

## Acceptance

1. Within ~5s, `http://localhost:8080/s/test/` loads the editor — assets resolve and the terminal
   (WebSocket) works.
2. Stop the dispatched alloc → `/s/test/` returns 404 (route removed).
3. Dispatch `session_id=test2` → `/s/test2/` appears independently (multi-session routing).

## Results — PASS (2026-06-21)

Verified with Nomad 2.0.3 + nomad-driver-podman 0.6.4 (rootless), Traefik 3.1, podman 4.9.3, on a
single node advertising `192.168.0.20` via `{{ GetPrivateIP }}`, **without `--network host`**:
Traefik (on `pedagog-edge`) reached the API via `host.containers.internal`, the backend advertised
`192.168.0.20:<dynport>`, `/s/demo/` and `/s/demo2/` each served HTTP 200 independently, and
`/s/demo/` returned 404 after its alloc was stopped while `/s/demo2/` stayed up.
