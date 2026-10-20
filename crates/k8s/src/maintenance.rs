//! Maintenance in the data namespace: the registry's garbage collection (a Job
//! on the registry's volume, on its node) and volume capacities.

use std::collections::BTreeMap;
use std::time::Duration;

use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Affinity, Container, PersistentVolumeClaim, PersistentVolumeClaimVolumeSource, PodAffinity,
    PodAffinityTerm, PodSpec, PodTemplateSpec, ResourceRequirements, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use kube::Api;
use kube::api::{DeleteParams, ListParams, LogParams, PostParams, PropagationPolicy};

use crate::KubeClient;

const GC_JOB: &str = "registry-gc";
const REGISTRY_DATA: &str = "/var/lib/registry";
/// The line the GC Job ends with: `disk <df -Pk line>`.
pub const DISK_MARKER: &str = "disk ";
const POLL: Duration = Duration::from_secs(5);

/// The registry's garbage collection.
pub struct RegistryGc {
    /// The registry's own image (it has the `registry` binary and `df`).
    pub image: String,
    /// The registry's PVC.
    pub pvc: String,
    /// The registry pod's `app` label, to run on its node (the PVC is
    /// ReadWriteOnce).
    pub registry_app: String,
    pub deadline: Duration,
}

/// How a GC Job ended, with its log.
#[derive(Debug)]
pub struct GcRun {
    pub succeeded: bool,
    pub logs: String,
}

pub struct Maintenance {
    client: KubeClient,
}

impl Maintenance {
    pub fn new(client: KubeClient) -> Self {
        Self { client }
    }

    /// Run garbage collection now: replace any earlier GC Job, then wait for it.
    pub async fn registry_gc(&self, gc: &RegistryGc) -> kube::Result<GcRun> {
        let jobs = self.client.jobs();
        let dp = DeleteParams {
            propagation_policy: Some(PropagationPolicy::Background),
            ..Default::default()
        };
        match jobs.delete(GC_JOB, &dp).await {
            Ok(_) => {}
            Err(kube::Error::Api(e)) if e.code == 404 => {}
            Err(e) => return Err(e),
        }
        for _ in 0..12 {
            if jobs.get_opt(GC_JOB).await?.is_none() {
                break;
            }
            tokio::time::sleep(POLL).await;
        }
        jobs.create(&PostParams::default(), &gc_job(gc)).await?;
        let give_up = tokio::time::Instant::now() + gc.deadline + Duration::from_secs(60);
        loop {
            let status = jobs.get_opt(GC_JOB).await?.and_then(|j| j.status);
            let (ok, failed) = status.map_or((0, 0), |s| {
                (s.succeeded.unwrap_or(0), s.failed.unwrap_or(0))
            });
            if ok > 0 || failed > 0 || tokio::time::Instant::now() >= give_up {
                return Ok(GcRun {
                    succeeded: ok > 0,
                    logs: self.logs().await,
                });
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// A PVC's capacity (its status, else its request), as a quantity.
    pub async fn pvc_capacity(&self, name: &str) -> kube::Result<Option<String>> {
        let pvcs: Api<PersistentVolumeClaim> =
            Api::namespaced(self.client.client.clone(), self.client.namespace());
        Ok(pvcs.get_opt(name).await?.and_then(|p| {
            let status = p.status.and_then(|s| s.capacity);
            let request = p.spec.and_then(|s| s.resources).and_then(|r| r.requests);
            status
                .or(request)
                .and_then(|m| m.get("storage").map(|q| q.0.clone()))
        }))
    }

    async fn logs(&self) -> String {
        let pods = self.client.pods();
        let lp = ListParams::default().labels(&format!("batch.kubernetes.io/job-name={GC_JOB}"));
        let Ok(list) = pods.list(&lp).await else {
            return String::new();
        };
        for pod in list {
            if let Some(name) = pod.metadata.name
                && let Ok(logs) = pods.logs(&name, &LogParams::default()).await
            {
                return logs;
            }
        }
        String::new()
    }
}

fn gc_job(gc: &RegistryGc) -> Job {
    let labels = BTreeMap::from([("app".to_string(), GC_JOB.to_string())]);
    let small = BTreeMap::from([
        ("cpu".to_string(), Quantity("100m".into())),
        ("memory".to_string(), Quantity("128Mi".into())),
    ]);
    let script = format!(
        "registry garbage-collect --delete-untagged /etc/docker/registry/config.yml \
         && echo \"{DISK_MARKER}$(df -Pk {REGISTRY_DATA} | tail -1)\""
    );
    let pod = PodSpec {
        restart_policy: Some("Never".into()),
        affinity: Some(Affinity {
            pod_affinity: Some(PodAffinity {
                required_during_scheduling_ignored_during_execution: Some(vec![PodAffinityTerm {
                    label_selector: Some(LabelSelector {
                        match_labels: Some(BTreeMap::from([(
                            "app".to_string(),
                            gc.registry_app.clone(),
                        )])),
                        ..Default::default()
                    }),
                    topology_key: "kubernetes.io/hostname".into(),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }),
        containers: vec![Container {
            name: "gc".into(),
            image: Some(gc.image.clone()),
            command: Some(vec!["/bin/sh".into(), "-c".into(), script]),
            volume_mounts: Some(vec![VolumeMount {
                name: "data".into(),
                mount_path: REGISTRY_DATA.into(),
                ..Default::default()
            }]),
            resources: Some(ResourceRequirements {
                requests: Some(small.clone()),
                limits: Some(small),
                ..Default::default()
            }),
            ..Default::default()
        }],
        volumes: Some(vec![Volume {
            name: "data".into(),
            persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                claim_name: gc.pvc.clone(),
                ..Default::default()
            }),
            ..Default::default()
        }]),
        ..Default::default()
    };
    Job {
        metadata: ObjectMeta {
            name: Some(GC_JOB.into()),
            labels: Some(labels.clone()),
            ..Default::default()
        },
        spec: Some(JobSpec {
            backoff_limit: Some(0),
            ttl_seconds_after_finished: Some(86_400),
            active_deadline_seconds: Some(gc.deadline.as_secs() as i64),
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..Default::default()
                }),
                spec: Some(pod),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gc_runs_beside_the_registry_on_its_volume() {
        let job = gc_job(&RegistryGc {
            image: "registry:2".into(),
            pvc: "registry-pvc".into(),
            registry_app: "registry".into(),
            deadline: Duration::from_secs(900),
        });
        let spec = job.spec.unwrap();
        assert_eq!(spec.backoff_limit, Some(0));
        let pod = spec.template.spec.unwrap();
        let term = &pod
            .affinity
            .unwrap()
            .pod_affinity
            .unwrap()
            .required_during_scheduling_ignored_during_execution
            .unwrap()[0];
        assert_eq!(term.topology_key, "kubernetes.io/hostname");
        assert_eq!(
            term.label_selector.clone().unwrap().match_labels.unwrap()["app"],
            "registry"
        );
        assert_eq!(
            pod.volumes.unwrap()[0]
                .persistent_volume_claim
                .clone()
                .unwrap()
                .claim_name,
            "registry-pvc"
        );
        let script = &pod.containers[0].command.clone().unwrap()[2];
        assert!(script.starts_with("registry garbage-collect --delete-untagged"));
        assert!(script.contains("df -Pk /var/lib/registry"));
    }
}
