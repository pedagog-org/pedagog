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
just kind-deploy       # apply deploy/overlays/kind (local-path storage, dev-mode jobs)
just kind-rebuild      # POST /internal/recipes/rebuild; `just kind-rebuild force=true`
just kind-upload <dir> # upload an assignment (add --wait to follow build and validation)
just kind-e2e          # end-to-end check of the assignment pipeline
just kind-down         # delete the cluster
```

The kind overlay generates dev-only secrets (password `pedagog`) and runs `jobs` in dev
mode: no build at startup, Kaniko reads the recipes mounted into the node, and
`kind-rebuild` is the trigger. Results land in the in-cluster registry
(`registry-service.pedagog-data.svc:5000`) and in the `os_builds` / `build_runs` tables.

Assignments: build the OS image first (`just kind-rebuild`), then
`just kind-upload recipes/examples/linked-list-python --wait` checks, packs and uploads
the example; `jobs` stores it in Garage, builds it with Kaniko, validates the reference
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

Build and push the `jobs` image, then point the overlay at it (substitute your
registry IP from `kubectl get svc registry-service -n pedagog-data`):

```sh
podman build -f crates/jobs/Containerfile -t <registry-ip>:5000/pedagog/jobs:dev .
podman push <registry-ip>:5000/pedagog/jobs:dev
```

Then edit for your target overlay:

- `deploy/overlays/{dev,prod}/kustomization.yaml` → `images:` `newName` = `<registry-ip>:5000/pedagog/jobs`
- **dev:** `deploy/overlays/dev/patches/jobs-dev.yaml` → set both host paths to your recipes checkout on the node
- **prod:** `deploy/overlays/prod/patches/jobs-prod.yaml` → `PEDAGOG_JOBS_IMAGE` = the same pushed ref

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

## Phase 5 — Assignment Builds

`jobs` takes assignment uploads, builds them with Kaniko and validates them; the defaults
suit a small cluster. Set these on the `pedagog-jobs` Deployment when they don't:

| Env var | Default | Notes |
| --- | --- | --- |
| `PEDAGOG_BLOCKED_CIDRS` | (none) | Extra ranges excluded from `mode: allow` egress; private, shared (`100.64.0.0/10`) and link-local ranges (which cover the cluster, node IPs and the LAN) are always excluded |
| `PEDAGOG_BUILD_CACHE` | `true` | Kaniko layer cache in `<registry>/pedagog/cache` |
| `PEDAGOG_ASSIGNMENT_BUILD_DEADLINE` | `1800` | Seconds |
| `PEDAGOG_VALIDATION_DEADLINE` | `600` | Seconds |
| `PEDAGOG_VALIDATION_CPU` / `_MEMORY` | `1` / `1Gi` | Per validation pod |
| `PEDAGOG_RUN_NAMESPACE` | `pedagog-grading` | Where validation and grading pods run |
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
- **Registry pulls:** validation pods pull the built image by its in-cluster name
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
- **Uploading:** until the web UI exists, upload from your machine through a
  port-forward: `kubectl -n pedagog-builds port-forward svc/pedagog-jobs 18081:8080`, then
  `hammer upload <dir> --wait` (with `HAMMER_RECIPES` set for the local check).

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
