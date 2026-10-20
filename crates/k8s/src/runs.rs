//! Test runs ending, as Kubernetes reports them: one watch on the run
//! namespace's test-run pods, instead of polling each run's Job. A run ends
//! when its main container terminates (its exit code says whether the helper
//! succeeded), or when its pod fails before that (e.g. an init container).

use futures::{Stream, StreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::runtime::{WatchStreamExt, watcher};

use crate::build::Builder;
use crate::validate::{ROLE_LABEL, RUN_ID_LABEL};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunKind {
    /// `id` is the build being validated.
    Validation,
    /// `id` is the grading run.
    Grading,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunEnded {
    pub kind: RunKind,
    pub id: i64,
    pub succeeded: bool,
    /// Why Kubernetes stopped it, when it did.
    pub stopped: Option<Stopped>,
}

/// Kubernetes ended a run itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// Its Job's deadline passed.
    Deadline,
    /// A container went over its memory limit.
    OutOfMemory,
    /// The pod was evicted for using more local disk than its limit.
    OutOfDisk,
}

/// Why a pod's run was stopped, from its status.
pub(crate) fn stopped_pod(pod: &Pod) -> Option<Stopped> {
    let status = pod.status.as_ref()?;
    let oom = status
        .container_statuses
        .iter()
        .chain(status.init_container_statuses.iter())
        .flatten()
        .filter_map(|c| c.state.as_ref()?.terminated.as_ref())
        .any(|t| t.reason.as_deref() == Some("OOMKilled"));
    // The kubelet's message for it: "Pod ephemeral local storage usage exceeds
    // the total limit of containers 1Gi." (or "Container ... exceeded its local
    // ephemeral storage limit").
    let disk = status.reason.as_deref() == Some("Evicted")
        && status
            .message
            .as_deref()
            .is_some_and(|m| m.contains("ephemeral"));
    if oom {
        Some(Stopped::OutOfMemory)
    } else if disk {
        Some(Stopped::OutOfDisk)
    } else if status.reason.as_deref() == Some("DeadlineExceeded") {
        Some(Stopped::Deadline)
    } else {
        None
    }
}

impl Builder {
    /// Call `handle` for every test run that ends, as it happens; returns
    /// only if the watch itself ends (it doesn't: errors restart it with
    /// backoff). It starts with the pods that already exist, so runs that ended
    /// while nobody watched are reported too. A pod may be reported more than
    /// once.
    pub async fn on_runs_ended<F, Fut>(&self, mut handle: F)
    where
        F: FnMut(RunEnded) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let mut ended = std::pin::pin!(self.runs_ended());
        while let Some(run) = ended.next().await {
            handle(run).await;
        }
    }

    fn runs_ended(&self) -> impl Stream<Item = RunEnded> + Send + 'static {
        let pods = self.runs.pods();
        watcher(pods, watcher::Config::default().labels(RUN_ID_LABEL))
            .default_backoff()
            .applied_objects()
            .filter_map(|pod| async move {
                match pod {
                    Ok(pod) => ended(&pod),
                    Err(e) => {
                        tracing::warn!(error = %e, "test-run watch error; retrying");
                        None
                    }
                }
            })
    }
}

/// Whether a test-run pod has ended, and how.
fn ended(pod: &Pod) -> Option<RunEnded> {
    let labels = pod.metadata.labels.as_ref()?;
    let kind = match labels.get(ROLE_LABEL)?.as_str() {
        "validate" => RunKind::Validation,
        "grade" => RunKind::Grading,
        _ => return None,
    };
    let id = labels.get(RUN_ID_LABEL)?.parse().ok()?;
    let status = pod.status.as_ref()?;
    let succeeded = match status.phase.as_deref() {
        Some("Succeeded") => true,
        Some("Failed") => false,
        // The phase lags the container: the main container ending is enough.
        _ => {
            let main = status.container_statuses.as_ref()?.first()?;
            main.state.as_ref()?.terminated.as_ref()?.exit_code == 0
        }
    };
    Some(RunEnded {
        kind,
        id,
        succeeded,
        stopped: if succeeded { None } else { stopped_pod(pod) },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    use k8s_openapi::api::core::v1::{
        ContainerState, ContainerStateRunning, ContainerStateTerminated, ContainerStatus, PodStatus,
    };
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn pod(role: &str, phase: &str, main: Option<ContainerState>) -> Pod {
        Pod {
            metadata: ObjectMeta {
                labels: Some(BTreeMap::from([
                    (ROLE_LABEL.to_string(), role.to_string()),
                    (RUN_ID_LABEL.to_string(), "42".to_string()),
                ])),
                ..Default::default()
            },
            status: Some(PodStatus {
                phase: Some(phase.into()),
                container_statuses: main.map(|state| {
                    vec![ContainerStatus {
                        name: "grade".into(),
                        state: Some(state),
                        ..Default::default()
                    }]
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn terminated(exit_code: i32) -> Option<ContainerState> {
        Some(ContainerState {
            terminated: Some(ContainerStateTerminated {
                exit_code,
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    #[test]
    fn running_out_of_memory_says_so() {
        let oom = Some(ContainerState {
            terminated: Some(ContainerStateTerminated {
                exit_code: 137,
                reason: Some("OOMKilled".into()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let ended = ended(&pod("grade", "Running", oom)).unwrap();
        assert_eq!(
            (ended.succeeded, ended.stopped),
            (false, Some(Stopped::OutOfMemory))
        );
        let mut late = pod("grade", "Failed", None);
        late.status.as_mut().unwrap().reason = Some("DeadlineExceeded".into());
        assert_eq!(
            super::ended(&late).unwrap().stopped,
            Some(Stopped::Deadline)
        );
    }

    #[test]
    fn evicted_for_disk_says_so() {
        let evicted = |message: &str| {
            let mut p = pod("grade", "Failed", None);
            let status = p.status.as_mut().unwrap();
            status.reason = Some("Evicted".into());
            status.message = Some(message.into());
            super::ended(&p).unwrap().stopped
        };
        assert_eq!(
            evicted(
                "Pod ephemeral local storage usage exceeds the total limit of containers 1Gi. "
            ),
            Some(Stopped::OutOfDisk)
        );
        assert_eq!(evicted("The node was low on resource: memory."), None);
    }

    #[test]
    fn a_run_ends_when_its_main_container_does() {
        let done = |kind, succeeded| {
            Some(RunEnded {
                kind,
                id: 42,
                succeeded,
                stopped: None,
            })
        };
        assert_eq!(
            ended(&pod("grade", "Running", terminated(0))),
            done(RunKind::Grading, true)
        );
        assert_eq!(
            ended(&pod("grade", "Running", terminated(1))),
            done(RunKind::Grading, false)
        );
        assert_eq!(
            ended(&pod("validate", "Succeeded", None)),
            done(RunKind::Validation, true)
        );
        assert_eq!(
            ended(&pod("grade", "Failed", None)),
            done(RunKind::Grading, false),
            "e.g. await-egress gave up"
        );
    }

    #[test]
    fn running_and_unrelated_pods_are_ignored() {
        let running = Some(ContainerState {
            running: Some(ContainerStateRunning::default()),
            ..Default::default()
        });
        assert_eq!(ended(&pod("grade", "Running", running)), None);
        assert_eq!(ended(&pod("grade", "Pending", None)), None);
        assert_eq!(ended(&pod("build", "Succeeded", None)), None);
    }
}
