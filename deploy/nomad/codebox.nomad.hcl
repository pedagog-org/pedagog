# A codebox: one student session's sandbox container, fronted by a per-session
# /s/<session_id>/ route that Traefik discovers from Nomad.
#
# The control plane registers one job per assignment (image + quotas as -var), then
# dispatches one instance per student session (session_id as -meta):
#
#   nomad job run -var image=<registry/ref> -var cpu=<mhz> -var memory=<mb> \
#       deploy/nomad/codebox.nomad.hcl
#   nomad job dispatch -meta session_id=<id> codebox

# Built from the assignment's image and its quotas; supplied at `job run` time.
variable "image" {
  type        = string
  description = "OCI image for the assignment, built from the base and pushed to the registry."
  default     = "localhost/pedagog-base:dev"
}

variable "cpu" {
  type    = number
  default = 1000
}

variable "memory" {
  type    = number
  default = 1024
}

# Lock the egress firewall at boot (immutable for the session). Set false only
# for non-exam test/instructor sessions, which leave nft editable in-container.
variable "lock_firewall" {
  type    = bool
  default = true
}

job "codebox" {
  type = "batch"

  # A template, not a running job: dispatched once per session with its session_id.
  parameterized {
    meta_required = ["session_id"]
  }

  group "box" {
    count = 1

    # Nomad maps a dynamic host port to the editor's 8080 inside the container.
    network {
      port "http" {
        to = 8080
      }
    }

    # The container advertises its route as tags; Traefik (separate) reads them.
    service {
      name     = "codebox-${NOMAD_META_session_id}"
      provider = "nomad"
      port     = "http"

      tags = [
        "traefik.enable=true",
        "traefik.http.routers.s-${NOMAD_META_session_id}.rule=PathPrefix(`/s/${NOMAD_META_session_id}`)",
        "traefik.http.routers.s-${NOMAD_META_session_id}.entrypoints=web",
        "traefik.http.routers.s-${NOMAD_META_session_id}.middlewares=add-slash@file,strip-s-${NOMAD_META_session_id}",
        # The editor serves relative asset paths, so strip the route prefix.
        "traefik.http.middlewares.strip-s-${NOMAD_META_session_id}.stripprefix.prefixes=/s/${NOMAD_META_session_id}",
      ]

      # Route only to a ready instance.
      check {
        type     = "tcp"
        port     = "http"
        interval = "10s"
        timeout  = "2s"
      }
    }

    task "editor" {
      driver = "podman"

      config {
        image    = var.image
        ports    = ["http"]
        hostname = "pedagog"

        # Boot loads + locks the egress firewall: net_admin to load nft, setpcap
        # to drop it (and itself) from the bounding set, setuid/setgid for chpst.
        # Without these, `nft -f` fails and boot aborts. See rootfs/etc/runit/boot.
        cap_drop = ["all"]
        cap_add  = ["net_admin", "setpcap", "setuid", "setgid"]
      }

      # Locked by default; `-var lock_firewall=false` leaves egress editable
      # in-container for test/instructor sessions.
      env {
        PEDAGOG_FIREWALL_LOCK = var.lock_firewall ? "1" : "0"
      }

      resources {
        cpu    = var.cpu
        memory = var.memory
      }
    }
  }
}
