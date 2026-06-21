# Single-node Nomad agent (server + client) for local routing work.
# Advertises a routable IP so a containerized Traefik can reach the API and
# alloc ports WITHOUT host networking — same shape as a real multi-node cluster.

data_dir  = "/tmp/pedagog-nomad"
bind_addr = "0.0.0.0"

advertise {
  # go-sockaddr templates: resolved to this host's private IP at startup.
  # No hardcoded address — each node auto-detects its own.
  http = "{{ GetPrivateIP }}"
  rpc  = "{{ GetPrivateIP }}"
  serf = "{{ GetPrivateIP }}"
}

server {
  enabled          = true
  bootstrap_expect = 1
}

client {
  enabled = true
  # Interface whose IP alloc ports advertise on; the default-route one.
  network_interface = "{{ GetDefaultInterfaces | attr \"name\" }}"
}

plugin "nomad-driver-podman" {
  config {
    # Rootless podman API socket for this user (uid is host-specific).
    socket_path = "unix:///run/user/1000/podman/podman.sock"
  }
}
