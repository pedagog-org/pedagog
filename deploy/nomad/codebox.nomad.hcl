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
      }

      resources {
        cpu    = var.cpu
        memory = var.memory
      }
    }
  }
}
