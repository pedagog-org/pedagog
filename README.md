# Pedagog

A system for administering **browser-based coding exams**. Students connect from a locked-down
browser to an ephemeral, restricted container running **VS Code in the browser** (code-server), do
their work, and submit. A control plane orchestrates the containers, timing, auth, submissions, and
archival.

Each student session runs as an isolated **rootless podman** container, scheduled by **Nomad** and
routed by a single **Traefik** edge proxy under a per-session `/s/<session_id>/` path. Security and
student isolation are first-class concerns throughout.

> Status: early/greenfield. The design history lives in [`docs/`](docs/) (paired
> `NN-prompt-*` / `NN-design-*` files, in chronological order).

## How it's laid out

| Path | What's there |
|---|---|
| `docs/` | Design + prompt history — start here to understand decisions. |
| `images/base/` | The base container image (code-server, locked down) students run in. |
| `deploy/` | Running it locally: Nomad agent + job, Traefik edge proxy. See [`deploy/README.md`](deploy/README.md). |

## Getting started

You'll need a Linux host with **podman** (rootless), **Nomad**, and the **nomad-driver-podman**
plugin. Quick version:

```bash
# 1. podman + its rootless API socket (Nomad's driver talks to this).
sudo apt-get install -y podman        # or your distro's package
systemctl --user enable --now podman.socket

# 2. Nomad (binary on PATH).
mkdir -p ~/.local/bin
curl -fsSL https://releases.hashicorp.com/nomad/2.0.3/nomad_2.0.3_linux_amd64.zip -o /tmp/nomad.zip
unzip -o /tmp/nomad.zip -d ~/.local/bin

# 3. The podman task driver for Nomad.
mkdir -p ~/pedagog-tools/plugins
curl -fsSL https://releases.hashicorp.com/nomad-driver-podman/0.6.4/nomad-driver-podman_0.6.4_linux_amd64.zip -o /tmp/driver.zip
unzip -o /tmp/driver.zip -d ~/pedagog-tools/plugins

# 4. Build the base image.
podman build -t pedagog-base:dev images/base
```

Then follow [`deploy/README.md`](deploy/README.md) to start Nomad + Traefik and bring up a session.

> In-depth, OS-specific installation instructions will come later. The above is the minimal path on
> a Debian/Ubuntu-style host.
