# Infrastructure — Kustomize Setup

**Date:** 2026-07-07
**Status:** Pending review

---

## Rationale

Establish the foundational infrastructure for Pedagog using Kubernetes (k3s) and
Kustomize. The goal is a single `kubectl apply -k` command that brings up the full
platform for both local dev and production (RPi cluster), with differences expressed
as Kustomize overlays rather than scripts or duplicated manifests.

---

## Alternatives Considered

- **Helm** — more templating power, but more ceremony and an extra tool to install.
  Kustomize is built into `kubectl` and sufficient for this use case.
- **Raw YAML** — no way to express dev/prod differences without duplication or
  scripts.
- **Managed k8s (EKS, GKE, etc.)** — not viable for the RPi starting point.

---

## Open Questions

- **k3s version** — needs to be pinned for reproducibility.

---

## Rollback Plan

- `kubectl delete -k deploy/overlays/dev` (or `prod`) removes all platform resources.
- Longhorn has its own cleanup script that removes its CRDs and data.
- k3s provides `/usr/local/bin/k3s-uninstall.sh` to remove the cluster entirely.
- Postgres and registry PVCs must be deleted manually if data cleanup is needed.

---

## Directory Structure

```text
deploy/
  base/
    kustomization.yaml
    namespaces/
      pedagog-system.yaml
      pedagog-data.yaml
    traefik/
      crds.yaml
      rbac.yaml
      deployment.yaml
      service.yaml
      ingress-class.yaml
    cert-manager/
      kustomization.yaml       # references upstream cert-manager install manifest
      cluster-issuer.yaml      # base ClusterIssuer (patched per overlay)
    postgres/
      deployment.yaml
      service.yaml
      pvc.yaml
    registry/
      deployment.yaml
      service.yaml             # LoadBalancer via MetalLB (stable IP for registries.yaml)
      pvc.yaml
      network-policy.yaml
    longhorn/
      kustomization.yaml       # references upstream Longhorn manifest (longhorn-system)
    metallb/
      kustomization.yaml       # references upstream MetalLB manifest (metallb-system)
      ip-pool.yaml             # IPAddressPool + L2Advertisement (patched per overlay)
  overlays/
    dev/
      kustomization.yaml
      patches/
        longhorn-replicas.yaml        # replication.count: 1
        postgres-storage.yaml         # 2Gi PVC
        registry-storage.yaml         # 5Gi PVC
        metallb-pool.yaml             # local IP range for dev
      cluster-issuer-dev.yaml         # dev.pedagog.app, Let's Encrypt staging
    prod/
      kustomization.yaml
      patches/
        longhorn-replicas.yaml        # replication.count: 3
        postgres-storage.yaml         # 50Gi PVC
        registry-storage.yaml         # 100Gi PVC
        metallb-pool.yaml             # RPi network IP range for prod
      cluster-issuer-prod.yaml        # pedagog.app, Let's Encrypt production
```

---

## Component Decisions

### k3s

- Single binary, same version on dev and all RPi nodes.
- k3s's built-in Traefik is **disabled** at install time (`--disable=traefik`) — we
  manage Traefik ourselves.
- k3s's built-in local-path storage provisioner is **disabled** (`--disable=local-storage`) — Longhorn handles all storage.
- Each node's `/etc/rancher/k3s/registries.yaml` points to the registry's MetalLB
  LoadBalancer IP (stable across pod rescheduling, no node pinning needed).

### Namespaces

| Namespace | Contents |
| --- | --- |
| `pedagog-system` | Traefik, cert-manager, API, jobs (platform services) |
| `pedagog-data` | Postgres, cluster registry |
| `longhorn-system` | Longhorn (default, not overridden — upgrade path risk avoided) |
| `metallb-system` | MetalLB (default) |
| `pedagog-<course-id>` | Dynamically created at runtime per course; student containers and submission runner pods |

### Traefik

- Deployed as a `Deployment` in `pedagog-system`.
- Exposes a `LoadBalancer` Service on ports 80 and 443 (k3s ServiceLB handles this
  on bare metal).
- An `IngressClass` named `traefik` is registered as the default.
- TLS termination via cert-manager-issued certificates.

### cert-manager + TLS

- cert-manager installed from its upstream manifest (pinned version), namespaced to
  `pedagog-system`.
- A `ClusterIssuer` using the DNS-01 challenge with Cloudflare is configured per
  overlay:
  - Dev: `dev.pedagog.app`, Let's Encrypt **staging** (avoids rate limits during
    development).
  - Prod: `pedagog.app`, Let's Encrypt **production**.
- The Cloudflare API token is stored in a Secret in `pedagog-system` and referenced
  by the ClusterIssuer. This Secret is created manually once before first deploy and
  is not committed to the repo.

### Postgres

- Single-replica `Deployment` in `pedagog-data`.
- A `PersistentVolumeClaim` backed by Longhorn.
- Storage size patched per overlay: 2Gi dev, 50Gi prod.
- Credentials stored in a Secret created manually before first deploy (see Setup
  Instructions in `docs/SETUP.md`).

### Cluster Registry

- `registry:2` image, single-replica `Deployment` in `pedagog-data`.
- No auth. Access restricted by a `NetworkPolicy` allowing ingress only from:
  - Pods in `pedagog-system` (the `jobs` service pushes built images).
  - The MetalLB LoadBalancer IP (for k3s node image pulls).
- A `LoadBalancer` Service via MetalLB — stable IP regardless of which node the pod
  runs on. Configured in each node's `/etc/rancher/k3s/registries.yaml`.
- A `PersistentVolumeClaim` backed by Longhorn: 5Gi dev, 100Gi prod.

### Longhorn

- Installed via its upstream Kustomize manifest in `longhorn-system` (default — not
  overridden to avoid upgrade path issues).
- `StorageClass` named `longhorn` set as the default.
- Replica count patched per overlay: 1 (dev), 3 (prod).

### MetalLB

- Installed via its upstream Kustomize manifest in `metallb-system` (default).
- Provides `LoadBalancer` Services on bare-metal k3s (RPi and local dev).
- An `IPAddressPool` and `L2Advertisement` are configured per overlay:
  - Dev: a local IP range (e.g. a single unused IP on the dev machine's subnet).
  - Prod: a range of IPs on the RPi network.
- Used by Traefik (ports 80/443) and the cluster registry.

---

## Step-by-Step Implementation

1. Create the `deploy/` directory tree.
2. Write namespace manifests (`pedagog-system.yaml`, `pedagog-data.yaml`).
3. Write MetalLB kustomization (upstream reference) and base `IPAddressPool`.
4. Write Longhorn kustomization (upstream reference, `longhorn-system`).
5. Write Traefik manifests — CRDs, RBAC, Deployment, Service, IngressClass.
6. Write cert-manager kustomization (upstream reference) and base ClusterIssuer.
7. Write Postgres manifests — Deployment, Service, PVC.
8. Write registry manifests — Deployment, LoadBalancer Service, PVC, NetworkPolicy.
9. Write `base/kustomization.yaml` referencing all of the above.
10. Write `overlays/dev/kustomization.yaml` and patches (storage sizes, replica counts,
    MetalLB IP pool, ClusterIssuer for `dev.pedagog.app`).
11. Write `overlays/prod/kustomization.yaml` and patches (storage sizes, replica counts,
    MetalLB IP pool, ClusterIssuer for `pedagog.app`).
12. Document setup steps in `docs/SETUP.md` (see Deploy Order below).

---

## Deploy Order

Documented in `docs/SETUP.md` and followed on every fresh cluster.

### Phase 0 — k3s install

```sh
# Install k3s with built-in Traefik and local-path storage disabled
curl -sfL https://get.k3s.io | INSTALL_K3S_VERSION=<pinned> sh -s - \
  --disable=traefik \
  --disable=local-storage
```

Repeat on each node (agent nodes join the server via `K3S_URL` + `K3S_TOKEN`).

Configure `/etc/rancher/k3s/registries.yaml` on each node once the registry
LoadBalancer IP is known (after Phase 2).

### Phase 1 — Secrets (before any workloads)

```sh
# Cloudflare API token for cert-manager DNS-01
kubectl create secret generic cloudflare-api-token \
  --from-literal=api-token=<token> \
  -n pedagog-system

# Postgres credentials
kubectl create secret generic postgres-credentials \
  --from-literal=password=<password> \
  -n pedagog-data
```

### Phase 2 — Storage + load balancing (CRDs + controllers first)

```sh
kubectl apply -k deploy/base/longhorn
kubectl wait --for=condition=ready pod -l app=longhorn-manager \
  -n longhorn-system --timeout=300s

kubectl apply -k deploy/base/metallb
kubectl wait --for=condition=ready pod -l app=metallb \
  -n metallb-system --timeout=120s
```

### Phase 3 — Platform infrastructure

```sh
# First pass: registers CRDs (cert-manager, Traefik)
kubectl apply -k deploy/overlays/dev

# Second pass: applies resources that depend on those CRDs (ClusterIssuer, IngressClass)
kubectl apply -k deploy/overlays/dev
```

---

## Verification Checklist

After deploy, confirm each component is healthy:

- **Longhorn** — Longhorn UI accessible; dashboard shows all nodes healthy and storage
  available
- **MetalLB** — `kubectl get svc -A` shows `LoadBalancer` services with assigned
  external IPs (not `<pending>`)
- **Traefik** — Traefik pod running in `pedagog-system`; LoadBalancer IP assigned on
  ports 80/443; a test `Ingress` routes to a dummy pod
- **cert-manager** — `ClusterIssuer` shows `Ready: True`; a test `Certificate`
  resource issues successfully via DNS-01 (check with `kubectl describe certificate`)
- **Postgres** — pod running in `pedagog-data`; reachable from within cluster via
  `kubectl exec` + `psql`
- **Registry** — pod running in `pedagog-data`; LoadBalancer IP assigned; a test
  `docker push` from within the cluster succeeds; a push from outside the cluster is
  rejected by the NetworkPolicy
