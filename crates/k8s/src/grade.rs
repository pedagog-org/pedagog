//! Grading runs: a Job from the assignment's image that fetches a submission
//! into `/pedagog/submission` and runs the test helper's launcher on it,
//! locked down like validation. Also what the admission gate reads: how many
//! test runs are still waiting for a node.

use std::time::Duration;

use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Pod;
use kube::api::ListParams;

use crate::build::{Builder, JobKey, Outcome};
use crate::validate::{Egress, NETWORK_LABEL, TEST_RUN_LABEL, TestRun, run_policy, test_run_job};

const APP_LABEL: &str = "pedagog-grade";
const ROLE: &str = "grade";
pub const SUBMISSION_DIR: &str = "/pedagog/submission";

#[derive(Clone, Debug)]
pub struct GradingRequest {
    pub image: String,
    /// Runs `await-egress` and `fetch-context`.
    pub jobs_image: String,
    /// Presigned GET of the submission's `.tgz`, and its tree hash.
    pub submission_url: String,
    pub tree_hash: String,
    /// Presigned PUT for results.yaml.
    pub results_url: String,
    /// `process` or `in-process`.
    pub isolation: String,
    pub egress: Egress,
    pub deadline: Duration,
    pub cpu: String,
    pub memory: String,
}

pub(crate) fn grading_job(name: &str, req: &GradingRequest) -> Job {
    test_run_job(
        name,
        &TestRun {
            app: APP_LABEL,
            role: ROLE,
            container: "grade",
            image: &req.image,
            jobs_image: &req.jobs_image,
            code: SUBMISSION_DIR,
            fetch: Some((&req.submission_url, &req.tree_hash)),
            isolation: &req.isolation,
            results_url: &req.results_url,
            egress: &req.egress,
            deadline: req.deadline,
            cpu: &req.cpu,
            memory: &req.memory,
        },
    )
}

/// Test runs that don't have a node yet: Jobs whose pod hasn't been created
/// (e.g. the namespace's quota is full) and pods the scheduler hasn't placed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Backlog {
    pub waiting: usize,
    /// Unfinished test-run Jobs (waiting or running).
    pub unfinished: usize,
}

impl Builder {
    /// Create the grading Job for `key` unless it exists, with its CIDR policy
    /// if the run has CIDR allowances.
    pub async fn ensure_grading(
        &self,
        key: &JobKey,
        req: &GradingRequest,
    ) -> kube::Result<Outcome> {
        let name = key.name();
        if self
            .client_for(key.kind)
            .jobs()
            .get_opt(&name)
            .await?
            .is_some()
        {
            return Ok(Outcome::Skipped);
        }
        let Some(created) = self.create_job(key.kind, &grading_job(&name, req)).await? else {
            return Ok(Outcome::Skipped);
        };
        if let Egress::Cidrs(cidrs) = &req.egress {
            let uid = created.metadata.uid.unwrap_or_default();
            self.apply_policy(&run_policy(&name, &uid, cidrs)).await?;
        }
        Ok(Outcome::Created)
    }

    /// How many test runs are waiting for a node, across the run namespace.
    pub async fn run_backlog(&self) -> kube::Result<Backlog> {
        let jobs = self
            .runs
            .jobs()
            .list(&ListParams::default().labels(&format!("{TEST_RUN_LABEL}=true")))
            .await?;
        let pods = self
            .runs
            .pods()
            .list(
                &ListParams::default()
                    .labels(NETWORK_LABEL)
                    .fields("status.phase=Pending"),
            )
            .await?;
        Ok(backlog(&jobs.items, &pods.items))
    }
}

fn backlog(jobs: &[Job], pending_pods: &[Pod]) -> Backlog {
    let unfinished: Vec<&Job> = jobs
        .iter()
        .filter(|j| {
            let s = j.status.as_ref();
            s.and_then(|s| s.succeeded).unwrap_or(0) == 0
                && s.and_then(|s| s.failed).unwrap_or(0) == 0
        })
        .collect();
    let podless = unfinished
        .iter()
        .filter(|j| j.status.as_ref().and_then(|s| s.active).unwrap_or(0) == 0)
        .count();
    let unscheduled = pending_pods
        .iter()
        .filter(|p| p.spec.as_ref().and_then(|s| s.node_name.as_ref()).is_none())
        .count();
    Backlog {
        waiting: podless + unscheduled,
        unfinished: unfinished.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use k8s_openapi::api::batch::v1::JobStatus;
    use k8s_openapi::api::core::v1::PodSpec;

    use crate::validate::EGRESS_PROBE;

    fn request(egress: Egress) -> GradingRequest {
        GradingRequest {
            image: "reg:5000/pedagog/assignments/ll:abc".into(),
            jobs_image: "localhost/pedagog/jobs:dev".into(),
            submission_url: "http://garage/submissions/ll/t.tgz?sig".into(),
            tree_hash: "t".into(),
            results_url: "http://garage/results?sig".into(),
            isolation: "process".into(),
            egress,
            deadline: Duration::from_secs(600),
            cpu: "1".into(),
            memory: "1Gi".into(),
        }
    }

    #[test]
    fn grading_pod_waits_fetches_then_tests_the_submission() {
        let job = grading_job("grade-ll-9", &request(Egress::Deny));
        assert_eq!(job.metadata.labels.clone().unwrap()[TEST_RUN_LABEL], "true");
        let template = job.spec.unwrap().template;
        let labels = template.metadata.unwrap().labels.unwrap();
        assert_eq!(labels["pedagog.io/role"], "grade");
        assert_eq!(labels[NETWORK_LABEL], "deny");
        let pod = template.spec.unwrap();
        let init = pod.init_containers.unwrap();
        assert_eq!(
            init[0].args.clone().unwrap()[..2],
            ["await-egress", EGRESS_PROBE]
        );
        assert_eq!(
            init[1].args.clone().unwrap(),
            [
                "fetch-context",
                "http://garage/submissions/ll/t.tgz?sig",
                SUBMISSION_DIR,
                "t"
            ]
        );
        assert_eq!(
            init[1].security_context.clone().unwrap().run_as_user,
            Some(65534)
        );
        let main = &pod.containers[0];
        assert_eq!(
            main.command.clone().unwrap(),
            [
                "/pedagog/bin/run-tests",
                "--code",
                SUBMISSION_DIR,
                "--isolation",
                "process"
            ]
        );
        let mount = &main.volume_mounts.clone().unwrap()[0];
        assert_eq!(
            (mount.mount_path.as_str(), mount.read_only),
            (SUBMISSION_DIR, Some(true))
        );
        assert_eq!(pod.volumes.unwrap()[0].name, mount.name);
        assert_eq!(pod.automount_service_account_token, Some(false));
    }

    fn job(active: i32, succeeded: i32) -> Job {
        Job {
            status: Some(JobStatus {
                active: Some(active),
                succeeded: Some(succeeded),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn pending(node: Option<&str>) -> Pod {
        Pod {
            spec: Some(PodSpec {
                node_name: node.map(str::to_owned),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn backlog_counts_runs_without_a_node() {
        let jobs = [job(0, 0), job(1, 0), job(1, 0), job(0, 1)];
        let pods = [pending(None), pending(Some("pi-2"))];
        assert_eq!(
            backlog(&jobs, &pods),
            Backlog {
                waiting: 2,
                unfinished: 3
            },
            "one Job without a pod, one unscheduled pod; a placed pod still initializing doesn't count"
        );
        assert_eq!(backlog(&[], &[]), Backlog::default());
    }
}
