//! Grading runs: a Job from the assignment's image that fetches a submission
//! into `/pedagog/submission` and runs the test helper's launcher on it,
//! locked down like validation. Also what the admission gate reads: how many
//! test runs are still waiting for a node.

use std::time::Duration;

use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{Pod, ResourceQuota};
use kube::api::ListParams;
use pedagog_core::recipe::assignment::Isolation;

use crate::build::{Builder, JobKey, Outcome};
use crate::validate::{Egress, NETWORK_LABEL, TEST_RUN_LABEL, TestRun, run_policy, test_run_job};

const APP_LABEL: &str = "pedagog-grade";
const ROLE: &str = "grade";
pub const SUBMISSION_DIR: &str = "/pedagog/submission";

#[derive(Clone, Debug)]
pub struct GradingRequest {
    pub run_id: i64,
    pub image: String,
    /// Runs `await-egress` and `fetch-context` (`pedagog/jobs-init`).
    pub init_image: String,
    /// Presigned GET of the submission's `.tgz`, and its tree hash.
    pub submission_url: String,
    pub tree_hash: String,
    /// Presigned PUT for results.yaml.
    pub results_url: String,
    pub isolation: Isolation,
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
            run_id: req.run_id,
            container: "grade",
            image: &req.image,
            init_image: &req.init_image,
            code: SUBMISSION_DIR,
            fetch: Some((&req.submission_url, &req.tree_hash)),
            isolation: req.isolation,
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

/// One test run's CPU and memory requests (and limits: test runs set both
/// the same), as Kubernetes quantities (`1`, `500m`, `1Gi`).
#[derive(Clone, Debug)]
pub struct RunSize {
    pub cpu: String,
    pub memory: String,
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

    /// Whether one more test run of each of `sizes` would fit every
    /// ResourceQuota in the run namespace (no quota: it fits). Releasing a run
    /// that doesn't fit would leave its Job without a pod, retrying with backoff.
    pub async fn run_fits(&self, sizes: &[RunSize]) -> kube::Result<bool> {
        let quotas = self
            .runs
            .resource_quotas()
            .list(&ListParams::default())
            .await?;
        Ok(quotas
            .items
            .iter()
            .all(|q| sizes.iter().all(|s| fits(q, s))))
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

fn fits(quota: &ResourceQuota, size: &RunSize) -> bool {
    let Some(status) = &quota.status else {
        return true;
    };
    let (Some(hard), used) = (&status.hard, status.used.clone().unwrap_or_default()) else {
        return true;
    };
    let needs = [
        ("requests.cpu", &size.cpu),
        ("limits.cpu", &size.cpu),
        ("cpu", &size.cpu),
        ("requests.memory", &size.memory),
        ("limits.memory", &size.memory),
        ("memory", &size.memory),
        ("pods", &"1".to_owned()),
        ("count/pods", &"1".to_owned()),
    ];
    needs.iter().all(|(key, need)| {
        let Some(limit) = hard.get(*key).and_then(|q| quantity(&q.0)) else {
            return true;
        };
        let in_use = used.get(*key).and_then(|q| quantity(&q.0)).unwrap_or(0.0);
        quantity(need).is_none_or(|need| in_use + need <= limit + 1e-9)
    })
}

/// A Kubernetes quantity in base units (cores, bytes, counts): `3500m`,
/// `0.5`, `4Gi`, `128M`, `2`.
fn quantity(text: &str) -> Option<f64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, suffix) = text.split_at(split);
    let scale = match suffix {
        "" => 1.0,
        "m" => 1e-3,
        "k" => 1e3,
        "M" => 1e6,
        "G" => 1e9,
        "T" => 1e12,
        "Ki" => 1024.0,
        "Mi" => 1024.0 * 1024.0,
        "Gi" => 1024.0 * 1024.0 * 1024.0,
        "Ti" => 1024.0_f64.powi(4),
        _ => return None,
    };
    Some(number.parse::<f64>().ok()? * scale)
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
            run_id: 9,
            image: "reg:5000/pedagog/assignments/ll:abc".into(),
            init_image: "localhost/pedagog/jobs-init:dev".into(),
            submission_url: "http://garage/submissions/ll/t.tgz?sig".into(),
            tree_hash: "t".into(),
            results_url: "http://garage/results?sig".into(),
            isolation: Isolation::Process,
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
        assert_eq!(labels[crate::validate::RUN_ID_LABEL], "9");
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

    fn quota(hard: &[(&str, &str)], used: &[(&str, &str)]) -> ResourceQuota {
        use k8s_openapi::api::core::v1::ResourceQuotaStatus;
        use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
        let map = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), Quantity((*v).to_owned())))
                .collect()
        };
        ResourceQuota {
            status: Some(ResourceQuotaStatus {
                hard: Some(map(hard)),
                used: Some(map(used)),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_run_fits_only_within_every_limit() {
        let size = RunSize {
            cpu: "1".into(),
            memory: "1Gi".into(),
        };
        let hard = [("requests.cpu", "3500m"), ("requests.memory", "4Gi")];
        assert!(fits(
            &quota(&hard, &[("requests.cpu", "2"), ("requests.memory", "2Gi")]),
            &size
        ));
        assert!(
            !fits(
                &quota(&hard, &[("requests.cpu", "3"), ("requests.memory", "3Gi")]),
                &size
            ),
            "3 + 1 CPU > 3.5"
        );
        assert!(
            !fits(
                &quota(
                    &hard,
                    &[("requests.cpu", "0"), ("requests.memory", "3584Mi")]
                ),
                &size
            ),
            "memory"
        );
        assert!(fits(&quota(&hard, &[]), &size), "nothing used yet");
        assert!(fits(&ResourceQuota::default(), &size), "no status yet");
    }

    #[test]
    fn quantities_parse_in_base_units() {
        assert_eq!(quantity("3500m"), Some(3.5));
        assert_eq!(quantity("1"), Some(1.0));
        assert_eq!(quantity("0.5"), Some(0.5));
        assert_eq!(quantity("4Gi"), Some(4.0 * 1024.0 * 1024.0 * 1024.0));
        assert_eq!(quantity("128M"), Some(128e6));
        assert_eq!(
            quantity("1e3"),
            None,
            "exponents aren't used by our manifests"
        );
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
