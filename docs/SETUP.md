# Pedagog — Setup Guide

## Prerequisites

- All nodes running a supported Linux distribution (Raspberry Pi OS or Debian/Ubuntu)
- `kubectl` installed on your local machine
- `open-iscsi` installed and enabled on every node (required by Longhorn):
  ```sh
  # Debian/Ubuntu/RPi OS
  sudo apt-get install -y open-iscsi
  sudo systemctl enable --now iscsid

  # Fedora/RHEL
  sudo dnf install -y iscsi-initiator-utils
  sudo systemctl enable --now iscsid
  ```
- Network connectivity between all nodes
- Firewalld disabled (or pod traffic allowed) — firewalld blocks pod-to-pod traffic by default:
  ```sh
  # Fedora/RHEL/Asahi Linux
  sudo systemctl disable --now firewalld

  # Alternative: allow only pod traffic (10.42.0.0/16 is k3s's default pod CIDR;
  # change this if you override --cluster-cidr at install time)
  sudo firewall-cmd --permanent --zone=trusted --add-interface=cni0
  sudo firewall-cmd --permanent --zone=trusted --add-source=10.42.0.0/16
  sudo firewall-cmd --reload
  ```
- An unused IP range on the cluster subnet for MetalLB (prod only — see Phase 2)
- A Cloudflare API token with `Zone:DNS:Edit` permission for your domain's zone

---

## Local Development

Working on the Rust crates doesn't need the cluster. For the crates that touch
Postgres (`store`, `jobs`), bring up a local dev database in podman:

```sh
just db-up       # start postgres:16-alpine (db/user 'pedagog', localhost:5432)
just db-prepare  # apply migrations + refresh the committed .sqlx/ offline cache
just db-shell    # psql into it
just db-down     # stop it (keeps the named volume)
```

`DATABASE_URL` defaults to `postgres://pedagog:pedagog@localhost:5432/pedagog`.
CI compiles offline via the committed `.sqlx/` cache (`SQLX_OFFLINE=true`); only
regenerating that cache needs a live DB.

### Web UI

The web UI (`crates/web`) is a Leptos app compiled to WebAssembly and built with
[trunk](https://trunkrs.dev); the wasm target comes with `rust-toolchain.toml`.

```sh
brew install trunk   # or: cargo install trunk --locked
just kind-api        # in one terminal: the api (and Mailpit, GlitchTip) from kind
just web-dev         # in another: http://localhost:3000, /api/ proxied to the api
just web-build       # the static files, into target/web
```

### Local cluster (kind)

To exercise in-cluster builds without the Pi cluster, run the platform on a local
[kind](https://kind.sigs.k8s.io/) cluster inside the podman machine. This is not k3s
(different Kubernetes version and storage), so the real cluster still needs its own run.

One-time setup (macOS):

```sh
brew install kind kubectl
podman machine stop && podman machine set --rootful && podman machine start
```

Rootful mode switches podman to a separate container store; containers created before
the switch are still reachable with `podman --connection podman-machine-default …`.

```sh
just kind-up           # (re)create the cluster, recipes/ mounted into the node
just kind-load-jobs    # build the jobs image with podman, load it into the node
just kind-load-api     # the same for the api
just kind-deploy       # apply deploy/overlays/kind (local-path storage, dev-mode jobs, Mailpit)
just kind-rebuild      # POST /internal/recipes/rebuild; `just kind-rebuild force=true`
just kind-add-admin <email>   # the first admin
just kind-login <email> [name] # log a persona in with the code from Mailpit (target/kind/<name>)
just kind-as <name> <hammer args>  # hammer as that persona
just kind-api          # port-forward the api (:18082) and Mailpit (:18025) until interrupted
just kind-e2e          # end-to-end check through the api
just kind-down         # delete the cluster
```

The kind overlay generates dev-only secrets (password `pedagog`) and runs `jobs` in dev
mode: no build at startup, Kaniko reads the recipes mounted into the node, and
`kind-rebuild` is the trigger. Results land in the in-cluster registry
(`registry-service.pedagog-data.svc:5000`) and in the `os_builds` / `build_runs` tables.

Assignments: build the OS image first (`just kind-rebuild`), make yourself admin and
staff and create a course (`just kind-add-admin you@kind.local`, `just kind-login
you@kind.local me`, `just kind-as me users role you@kind.local staff`, `just kind-as me
courses create demo "Demo"`), then `just kind-as me upload
recipes/examples/linked-list-python -c demo --wait` checks, packs and uploads the
example; `jobs` stores it in Garage, builds it with Kaniko, validates the reference
solution, and `--wait` prints the verdict. `kind-deploy` also runs `just kind-registry`,
which lets the node pull images from the in-cluster registry (validation pods run the
built image); clusters created by `kind-up` already have containerd's per-registry config
enabled, and older ones get it (with a containerd restart) on the first run.

---

## Pinned Versions

| Component | Version |
| --- | --- |
| k3s | v1.31.4+k3s1 |
| Longhorn | v1.7.2 |
| MetalLB | v0.14.9 |
| cert-manager | v1.16.2 |
| Traefik | v3.2.3 |
| Postgres | 16-alpine |
| Registry | 2 |
| `k8s-openapi` feature (crate) | `v1_32` — nearest offered by k8s-openapi 0.28 (kube 4.2); compatible with k3s 1.31 for the stable Job/Pod/ConfigMap APIs |

---

## Phase 0 — k3s Install

### Server node (first node)

```sh
curl -sfL https://get.k3s.io | INSTALL_K3S_VERSION=v1.31.4+k3s1 sh -s - \
  --disable=traefik \
  --disable=local-storage
```

> **Apple Silicon / 16K page kernels (e.g. Asahi Linux):** Add `--flannel-backend=host-gw`
> to the command above. The default VXLAN backend breaks pod-to-pod networking on kernels
> with a 16K page size.

Note the node token for joining agents:

```sh
cat /var/lib/rancher/k3s/server/node-token
```

### Agent nodes (RPi cluster only)

Get the server's IP address:

```sh
ip addr show | grep 'inet ' | grep -v 127.0.0.1
```

Run on each additional node, substituting the server IP and token from above:

```sh
curl -sfL https://get.k3s.io | INSTALL_K3S_VERSION=v1.31.4+k3s1 \
  K3S_URL=https://<server-ip>:6443 \
  K3S_TOKEN=<node-token> \
  sh -
```

### Configure kubectl

**On the server node itself:**

```sh
mkdir -p ~/.kube && sudo cat /etc/rancher/k3s/k3s.yaml > ~/.kube/config
```

**On a separate local machine:**

Copy `/etc/rancher/k3s/k3s.yaml` from the server node to `~/.kube/config` and replace
`127.0.0.1` with the server node's IP.

---

## Phase 1 — Secrets

Copy `.env.example` to `.env` and fill in the values:

```sh
cp .env.example .env
# edit .env with your values
```

Then create the secrets:

```sh
set -a && source .env && set +a

kubectl create namespace cert-manager
kubectl create secret generic cloudflare-api-token \
  --from-literal=api-token="$CF_API_TOKEN" \
  --namespace=cert-manager

kubectl create namespace pedagog-data
kubectl create secret generic postgres-credentials \
  --from-literal=password="$POSTGRES_PW" \
  --namespace=pedagog-data

# jobs service reads its DATABASE_URL from a secret in its own namespace
kubectl create namespace pedagog-builds
kubectl create secret generic pedagog-jobs-db \
  --from-literal=url="postgres://pedagog:${POSTGRES_PW}@postgres-service.pedagog-data.svc:5432/pedagog" \
  --namespace=pedagog-builds

# The api reads its DATABASE_URL from its own namespace too
kubectl create namespace pedagog-system
kubectl create secret generic pedagog-api-db \
  --from-literal=url="postgres://pedagog:${POSTGRES_PW}@postgres-service.pedagog-data.svc:5432/pedagog" \
  --namespace=pedagog-system
# Only if the mail relay wants a login:
[ -n "$SMTP_USER" ] && kubectl create secret generic pedagog-api-smtp \
  --from-literal=user="$SMTP_USER" --from-literal=password="$SMTP_PASSWORD" \
  --namespace=pedagog-system

# GlitchTip (error tracking): its database role's password and URL, Django's
# secret key, the mail relay, and its superuser (the bootstrap Job creates both)
kubectl create secret generic glitchtip-db \
  --from-literal=password="$GLITCHTIP_DB_PW" \
  --from-literal=url="postgres://glitchtip:${GLITCHTIP_DB_PW}@postgres-service.pedagog-data.svc:5432/glitchtip" \
  --namespace=pedagog-data
kubectl create secret generic glitchtip-secrets \
  --from-literal=secret-key="$(openssl rand -hex 32)" \
  --from-literal=email-url="$GLITCHTIP_EMAIL_URL" \
  --from-literal=admin-email="$GLITCHTIP_ADMIN_EMAIL" \
  --from-literal=admin-password="$GLITCHTIP_ADMIN_PW" \
  --namespace=pedagog-data

# Garage object store: cluster RPC secret (64 hex chars) and admin API token
kubectl create secret generic garage-secrets \
  --from-literal=rpc-secret="$(openssl rand -hex 32)" \
  --from-literal=admin-token="$(openssl rand -hex 32)" \
  --namespace=pedagog-data

# The platform's S3 key (Garage format: GK + 24 hex chars, 64-hex secret). The
# bootstrap Job imports it; jobs reads it from its own namespace.
GARAGE_KEY_ID="GK$(openssl rand -hex 12)"; GARAGE_KEY_SECRET="$(openssl rand -hex 32)"
for ns in pedagog-data pedagog-builds; do
  kubectl create secret generic garage-s3-key \
    --from-literal=access-key-id="$GARAGE_KEY_ID" \
    --from-literal=secret-access-key="$GARAGE_KEY_SECRET" \
    --namespace="$ns"
done
```

---

## Before Applying Overlays

Edit the ClusterIssuer for your target overlay and set your contact email and domain:

```yaml
# deploy/overlays/dev/cluster-issuer.yaml
# deploy/overlays/prod/cluster-issuer.yaml
spec:
  acme:
    email: <your-email>   # Let's Encrypt sends expiry notifications here
```

Build and push the `jobs` images, then point the overlay at them (substitute your
registry IP from `kubectl get svc registry-service -n pedagog-data`). `pedagog/jobs`
is the service, with the recipes baked in; `pedagog/jobs-init` (~66 MB) has only its
binary, for the initContainers of every test run and build, so a node pulls the big
image only for builds:

```sh
podman build -f crates/jobs/Containerfile -t <registry-ip>:5000/pedagog/jobs:dev .
podman build -f crates/jobs/Containerfile --target init -t <registry-ip>:5000/pedagog/jobs-init:dev .
podman push <registry-ip>:5000/pedagog/jobs:dev
podman push <registry-ip>:5000/pedagog/jobs-init:dev
```

Then edit for your target overlay:

- `deploy/overlays/{dev,prod}/kustomization.yaml` → `images:` `newName` = `<registry-ip>:5000/pedagog/jobs`
- **dev:** `deploy/overlays/dev/patches/jobs-dev.yaml` → set both host paths to your recipes checkout on the node, and `PEDAGOG_JOBS_IMAGE` / `PEDAGOG_JOBS_INIT_IMAGE` to the pushed refs
- **prod:** `deploy/overlays/prod/patches/jobs-prod.yaml` → `PEDAGOG_JOBS_IMAGE` / `PEDAGOG_JOBS_INIT_IMAGE` = the same pushed refs

---

## Phase 2 — Storage and Load Balancing

### Dev

Dev uses k3s's built-in ServiceLB (Klipper) for `LoadBalancer` services — no MetalLB needed.
Only Longhorn is required in this phase:

```sh
kubectl apply -k deploy/base/longhorn
kubectl wait --for=condition=ready pod \
  --selector=app=longhorn-manager \
  --namespace=longhorn-system \
  --timeout=300s
```

### Prod

Prod uses MetalLB for stable L2 LoadBalancer IPs. Before applying, edit
`deploy/overlays/prod/metallb-pool.yaml` and replace the placeholder with unused IPs
on your subnet (carve them out of your router's DHCP pool):

```yaml
spec:
  addresses:
    - 192.168.1.210-192.168.1.220  # replace with your range
```

Then apply:

```sh
kubectl apply -k deploy/base/longhorn
kubectl wait --for=condition=ready pod \
  --selector=app=longhorn-manager \
  --namespace=longhorn-system \
  --timeout=300s

kubectl apply -k deploy/base/metallb
kubectl wait --for=condition=ready pod \
  --selector=app=metallb,component=controller \
  --namespace=metallb-system \
  --timeout=120s
```

---

## Phase 3 — Platform Infrastructure

Apply twice: the first pass registers cert-manager CRDs; the second applies resources
that depend on them (ClusterIssuer).

```sh
# dev
kubectl apply -k deploy/overlays/dev
kubectl apply -k deploy/overlays/dev

# or prod
kubectl apply -k deploy/overlays/prod
kubectl apply -k deploy/overlays/prod
```

---

## Phase 4 — Configure Registry Access on Each Node

Get the registry's external IP:

```sh
kubectl get svc registry-service -n pedagog-data
```

- **Dev:** Klipper assigns the node's own IP (e.g. `192.168.1.23`).
- **Prod:** MetalLB assigns an IP from the pool configured in Phase 2.

On **each k3s node**, create or update `/etc/rancher/k3s/registries.yaml`,
substituting the actual IP:

```yaml
mirrors:
  "<registry-ip>:5000":
    endpoint:
      - "http://<registry-ip>:5000"
configs:
  "<registry-ip>:5000":
    tls:
      insecure_skip_verify: true
```

Then restart k3s on each node:

```sh
# server node
sudo systemctl restart k3s

# agent nodes
sudo systemctl restart k3s-agent
```

To push images to the registry from the host machine, configure podman to treat it as
insecure. Create `/etc/containers/registries.conf.d/pedagog-registry.conf`:

```toml
[[registry]]
location = "<registry-ip>:5000"
insecure = true
```

---

## Phase 5 — Assignment Builds and Grading

`jobs` takes assignment uploads, builds them with Kaniko and validates them, then takes
submissions and grades them; the defaults suit a small cluster. Set these on the `pedagog-jobs` Deployment when they don't:

| Env var | Default | Notes |
| --- | --- | --- |
| `PEDAGOG_BLOCKED_CIDRS` | (none) | Extra ranges excluded from `mode: allow` egress; private, shared (`100.64.0.0/10`) and link-local ranges (which cover the cluster, node IPs and the LAN) are always excluded |
| `PEDAGOG_BUILD_CACHE` | `true` | Kaniko layer cache in `<registry>/pedagog/cache` |
| `PEDAGOG_ASSIGNMENT_BUILD_DEADLINE` | `1800` | Seconds |
| `PEDAGOG_VALIDATION_DEADLINE` | `600` | Seconds |
| `PEDAGOG_VALIDATION_CPU` / `_MEMORY` | `1` / `1Gi` | Per validation pod |
| `PEDAGOG_RUN_NAMESPACE` | `pedagog-grading` | Where validation and grading pods run |
| `PEDAGOG_GRADING_DEADLINE` | `600` | Seconds |
| `PEDAGOG_GRADING_CPU` / `_MEMORY` | `1` / `1Gi` | Per grading pod |
| `PEDAGOG_MAX_RUNNING_TESTS` | (none) | Optional ceiling on test runs at once; otherwise the namespace's quota and the scheduler decide |
| `PEDAGOG_MAX_SUBMISSION_BYTES` / `PEDAGOG_MAX_SUBMISSION_FILES` | 50 MiB / 1000 | Submission ceiling; an assignment's `max_size` can only lower it |
| `PEDAGOG_MAX_UPLOAD_BYTES` / `PEDAGOG_MAX_UNPACKED_BYTES` / `PEDAGOG_MAX_UPLOAD_FILES` | 50 MiB / 200 MiB / 10 000 | Upload limits |

- **Test-run namespace:** validation (and grading) pods run in `pedagog-grading`, with
  their own quota (`deploy/base/grading/resource-quota.yaml`: three 1-CPU runs at once by
  default; raise it with your nodes). They start as root with a few added capabilities
  (see ARCHITECTURE.md, Assignment Uploads), so the namespace enforces Pod Security
  `baseline`; `restricted` would refuse them.
- **Network policies:** `jobs` installs the test-run policies in `pedagog-grading` itself
  at startup (RBAC in `deploy/base/grading/rbac.yaml`); the cluster's CNI must enforce
  NetworkPolicy (k3s does). Each run waits until its policies are in force before
  starting (`await-egress`, which probes CoreDNS on port 9153: keep the CoreDNS metrics
  port enabled, as k3s does by default).
- **Registry pulls:** validation and grading pods pull the built image by its in-cluster name
  (`registry-service.pedagog-data.svc:5000/...`) through each node's containerd, which
  can't resolve cluster DNS. Add that name to each node's
  `/etc/rancher/k3s/registries.yaml` (Phase 4), pointing at the registry's IP, and
  restart k3s:

  ```yaml
  mirrors:
    "registry-service.pedagog-data.svc:5000":
      endpoint:
        - "http://<registry-ip>:5000"
  ```
- **Uploading:** through the api (Phase 6): `hammer upload <dir> -c <course> --wait`
  (with `HAMMER_RECIPES` set for the local check). Add `--regrade [--notify changed]` to
  regrade every student's latest submission once the new build is current.
- **Submitting:** `hammer submit <assignment> <files|dir|archive> --wait` prints the
  receipt, then the results. Only assignments on the `upload` platform take submissions
  for now.
- **Capacity:** test runs wait in jobs' queue until there's room; if submissions sit
  queued, look at `pedagog-grading`'s quota (`kubectl -n pedagog-grading describe
  resourcequota`) and pending pods before raising anything.
- **Disk:** images are the biggest consumer: assignment images in the registry's volume,
  and every image a node has pulled in its image store.
  - Put the image store on an SSD, not the SD card, on Pis (k3s:
    `/var/lib/rancher/k3s/agent/containerd`; symlink or mount it before installing).
  - Let the kubelet remove unused images before the disk fills, e.g. in
    `/etc/rancher/k3s/config.yaml`:

    ```yaml
    kubelet-arg:
      - image-gc-high-threshold=80
      - image-gc-low-threshold=70
      - eviction-hard=nodefs.available<10%,imagefs.available<10%
    ```

  - The registry is cleaned nightly (03:00 UTC, `PEDAGOG_REGISTRY_CLEANUP_AT` on jobs);
    to clean it now, `kubectl -n pedagog-builds port-forward svc/pedagog-jobs 18081:8080`
    and `curl -X POST localhost:18081/internal/registry/cleanup`, then
    `kubectl -n pedagog-data logs job/registry-gc`.
  - Disk warnings (over 80%) and errors (over 90%) reach GlitchTip (Phase 7): a node's
    disk or image store (`pedagog-disk` DaemonSet; if a node's image store lives
    elsewhere, patch its `images` hostPath), the registry's volume, Postgres's.

## Phase 6 — The api and Login

The api is the public entry point: people log in with a code sent to their email.

1. **Image:** build and push `pedagog/api` like the jobs images
   (`podman build -f crates/api/Containerfile -t <registry-ip>:5000/pedagog/api:<tag> .`),
   and set it in the overlay's `images:`.
2. **Hostname and certificate:** in `deploy/base/api/ingress.yaml` (or an overlay
   patch), replace `REPLACE-WITH-HOSTNAME`; cert-manager's `letsencrypt` issuer gets
   the certificate. Set `PEDAGOG_PUBLIC_URL` to `https://<hostname>` (links in emails).
3. **Mail:** set on the `pedagog-api` Deployment:

   | Env var | Default | Notes |
   | --- | --- | --- |
   | `PEDAGOG_SMTP_HOST` | (required) | The relay |
   | `PEDAGOG_SMTP_TLS` | `none` | `none` (a trusted relay), `starttls` or `tls` |
   | `PEDAGOG_SMTP_PORT` | 25 / 587 / 465 | By TLS mode |
   | `PEDAGOG_SMTP_USER` / `_PASSWORD` | (none) | From the `pedagog-api-smtp` Secret (Phase 1), only if the relay wants a login |
   | `PEDAGOG_MAIL_FROM_NAME` / `_ADDRESS` | `Pedagog` / (required) | The sender |
   | `PEDAGOG_MAIL_REPLY_TO` | (none) | Optional |
   | `PEDAGOG_MAIL_PER_SECOND` | 5 | Notices sent per second per replica (keep under the relay's limit) |
   | `PEDAGOG_JOBS_URL` | `http://pedagog-jobs.pedagog-builds.svc:8080` | Where uploads and submissions go |
   | `PEDAGOG_MAX_UPLOAD_BYTES` | 50 MiB | The largest assignment archive passed on (match `jobs`') |
   | `PEDAGOG_WEB_DIR` | `/usr/share/pedagog/web` (in the image) | The web UI's files, served outside `/api`; unset: the api alone |

4. **The first admin:**

   ```sh
   kubectl -n pedagog-system exec deploy/pedagog-api -- /usr/local/bin/pedagog-api add-admin you@ufl.edu
   hammer login --api https://<hostname> --email you@ufl.edu
   hammer whoami
   ```

Then let an instructor create their course (or create it for them with
`--instructor`), and fill it:

```sh
hammer users role prof@ufl.edu staff                    # admin: prof may create courses
hammer courses create cop3502-f26 "COP3502 (Fall 2026)" # prof: and teaches it
hammer use cop3502-f26                                  # or -c on each command, or a course.yml
hammer members import canvas-gradebook.csv              # students; --sync also drops missing ones
hammer members add ta@ufl.edu --role grader
hammer members list
hammer members show gator                               # part of an email or name
hammer upload ./linked-list --wait                      # build and validate
hammer assignments list
hammer dates set linked-list --open always --due 2026-09-08T23:59:00-04:00
hammer visibility show linked-list                      # what students see of each test
hammer visibility set linked-list --error hidden         # e.g. on an exam
hammer results list linked-list                         # one row per student who submitted
hammer results show linked-list gator                   # one student, in full
hammer results release linked-list
hammer submit linked-list ./my-solution --wait          # a staff submission
hammer submit linked-list ./fixed --for albert.gator@ufl.edu --at last
hammer submissions linked-list --for albert.gator@ufl.edu
```

A course repo can carry a `course.yml` at its root (`course: cop3502-f26`, optionally
`api: https://<hostname>`); every command run inside it uses that course.

Admins find people with `hammer users list --role staff`, `hammer users show <email or
name>`, and every course with `hammer courses list --all --search cop3502`.

**Upgrading:** `api` and `jobs` share the database's migrations, and each refuses a
database with migrations it doesn't know. Deploy them together (the same tag in one
`kubectl apply -k`); rolling back across a migration means reverting it first
(`sqlx migrate revert`, or the migration's `.down.sql`).

On kind: `just kind-load-api`, `just kind-deploy`, `just kind-add-admin <email>`, then
`just kind-api` port-forwards the api (http://localhost:18082) and Mailpit's inbox
(http://localhost:18025), where the login code arrives.

---

## Phase 7 — Error Tracking (GlitchTip)

`api` and `jobs` report every warning and error (and panic) to GlitchTip, which groups
repeats and emails alerts. It runs in `pedagog-data` as one process, with its database
in our Postgres; nothing leaves the cluster.

1. **Hostname:** in `deploy/base/glitchtip/` (or an overlay patch), replace
   `REPLACE-WITH-GLITCHTIP-HOSTNAME` (ingress and `GLITCHTIP_DOMAIN`, e.g.
   `glitchtip.<domain>`) and `REPLACE-WITH-SENDER-ADDRESS`. Apply; the
   `glitchtip-bootstrap` Job creates its role, database and superuser (from Phase 1's
   Secrets; re-runnable: `kubectl -n pedagog-data delete job glitchtip-bootstrap`, then
   apply again).
2. **Projects:** sign in at `https://<glitchtip host>` as the superuser, create an
   organization and a project per service (`api`, `jobs`; platform Rust), and copy each
   project's DSN (Settings → Client Keys). Under Alerts, add an email alert.
3. **DSNs:** give each service its DSN with the host swapped for the internal Service,
   so events stay in the cluster:

   ```sh
   # https://<key>@<glitchtip host>/1  ->  http://<key>@glitchtip.pedagog-data.svc:8000/1
   kubectl -n pedagog-system create secret generic pedagog-api-sentry --from-literal=dsn=<api DSN>
   kubectl -n pedagog-builds create secret generic pedagog-jobs-sentry --from-literal=dsn=<jobs DSN>
   kubectl -n pedagog-system rollout restart deploy/pedagog-api
   kubectl -n pedagog-builds rollout restart deploy/pedagog-jobs
   ```

   Each logs `reporting warnings and errors to PEDAGOG_SENTRY_DSN's host` at start.
   Without the Secret (`PEDAGOG_SENTRY_DSN` unset), nothing is reported; a malformed DSN
   stops the service from starting.

Events are kept 90 days (`GLITCHTIP_RETENTION_DAYS`). It needs about 256–512 MiB.

On kind: the overlay sets dev secrets (superuser `admin@pedagog.dev` / `pedagog-dev`) and
mails Mailpit; `just kind-api` also forwards GlitchTip (http://localhost:18000).

---

## Verification

Run these checks after all phases complete.

### Longhorn

```sh
kubectl get pods -n longhorn-system
```

All pods should be `Running`. Access the Longhorn UI via port-forward:

```sh
kubectl port-forward svc/longhorn-frontend 8080:80 -n longhorn-system
```

Open `http://localhost:8080` — all nodes should show as healthy.

### MetalLB (prod only)

```sh
kubectl get svc -A | grep LoadBalancer
```

Both `traefik-service` (pedagog-system) and `registry-service` (pedagog-data) should show
an assigned external IP (not `<pending>`).

### Traefik

```sh
kubectl get pods -n pedagog-system
kubectl get svc traefik-service -n pedagog-system
```

Pod should be `Running`. Verify port-forward health check:

```sh
kubectl port-forward svc/traefik-service 9000:9000 -n pedagog-system
curl http://localhost:9000/ping  # should return: OK
```

### cert-manager

```sh
kubectl get clusterissuer letsencrypt
```

`READY` should be `True`. If not, check events:

```sh
kubectl describe clusterissuer letsencrypt
```

### Postgres

```sh
kubectl get pods -n pedagog-data -l app=postgres
```

Pod should be `Running`. Verify connectivity:

```sh
kubectl exec -it deploy/postgres -n pedagog-data -- psql -U pedagog -c '\l'
```

### Registry

```sh
kubectl get pods -n pedagog-data -l app=registry
```

Pod should be `Running`. Verify from within the cluster:

```sh
kubectl run registry-test --image=busybox --restart=Never -- \
  wget -qO- http://registry-service.pedagog-data.svc.cluster.local:5000/v2/
kubectl delete pod registry-test
```

Should return `{}`.

### Garage

```sh
kubectl get pods -n pedagog-data -l app=garage
kubectl logs -n pedagog-data job/garage-bootstrap
```

The pod should be `Running` and ready, and the bootstrap log should end with
`garage bootstrap complete` (it applies the single-node layout, imports the
`garage-s3-key` key and creates the `pedagog` bucket; re-running it is safe). Garage
reports unhealthy until that layout exists, so the pod only turns ready after bootstrap.
