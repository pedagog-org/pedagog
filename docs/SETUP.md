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
| `k8s-openapi` feature (crate) | `v1_31` — must track the k3s minor above |

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
