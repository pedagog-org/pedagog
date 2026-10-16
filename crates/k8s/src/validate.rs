//! Validation runs: a Job from a built assignment image that runs the pytest
//! helper's launcher against the baked reference solution, plus the network
//! policies that confine it.
//!
//! The pod starts as root (the launcher drops to `grader`/`runner` at once)
//! with only the capabilities the launcher uses, no service account token, and
//! egress limited by label: DNS and Garage always; the internet only for
//! `mode: allow`; CIDR rules through a per-run policy owned by the Job.

use std::collections::BTreeMap;
use std::time::Duration;

use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Capabilities, Container, EnvVar, PodSpec, PodTemplateSpec, ResourceRequirements,
    SeccompProfile, SecurityContext,
};
use k8s_openapi::api::networking::v1::{
    IPBlock, NetworkPolicy, NetworkPolicyEgressRule, NetworkPolicyPeer, NetworkPolicyPort,
    NetworkPolicySpec,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta, OwnerReference};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{Patch, PatchParams};

use crate::build::{Builder, JobKey, Outcome};

pub const ROLE_LABEL: &str = "pedagog.io/role";
pub const NETWORK_LABEL: &str = "pedagog.io/network";
const ROLE: &str = "validate";
const APP_LABEL: &str = "pedagog-assignment-validate";
const RUN_TESTS: &str = "/pedagog/bin/run-tests";
const SOLUTION_DIR: &str = "/pedagog/solution";
const TTL_SECONDS: i32 = 3600;
/// What the launcher needs: drop to grader/runner (SETUID, SETGID), copy the
/// grader-only solution and hand it to runner (DAC_OVERRIDE, CHOWN, FOWNER),
/// and kill runner's leftover processes (KILL).
const CAPABILITIES: [&str; 6] = [
    "SETUID",
    "SETGID",
    "CHOWN",
    "FOWNER",
    "DAC_OVERRIDE",
    "KILL",
];

/// Where a validation pod may connect, beyond DNS and Garage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Egress {
    Deny,
    Internet,
    /// `(cidr, ports)`; empty ports means all ports.
    Cidrs(Vec<(String, Vec<u16>)>),
}

impl Egress {
    fn label(&self) -> &'static str {
        match self {
            Egress::Deny => "deny",
            Egress::Internet => "allow",
            Egress::Cidrs(_) => "rules",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ValidationRequest {
    pub image: String,
    /// Presigned PUT for results.yaml.
    pub results_url: String,
    /// `process` or `in-process`.
    pub isolation: String,
    pub egress: Egress,
    pub deadline: Duration,
    pub cpu: String,
    pub memory: String,
}

pub(crate) fn validation_job(name: &str, req: &ValidationRequest) -> Job {
    let labels = BTreeMap::from([
        ("app".to_string(), APP_LABEL.to_string()),
        (ROLE_LABEL.to_string(), ROLE.to_string()),
        (NETWORK_LABEL.to_string(), req.egress.label().to_string()),
    ]);
    let limits = BTreeMap::from([
        ("cpu".to_string(), Quantity(req.cpu.clone())),
        ("memory".to_string(), Quantity(req.memory.clone())),
    ]);
    let container = Container {
        name: "validate".into(),
        image: Some(req.image.clone()),
        image_pull_policy: Some("IfNotPresent".into()),
        command: Some(vec![
            RUN_TESTS.into(),
            "--code".into(),
            SOLUTION_DIR.into(),
            "--isolation".into(),
            req.isolation.clone(),
        ]),
        env: Some(vec![EnvVar {
            name: "PEDAGOG_RESULTS_URL".into(),
            value: Some(req.results_url.clone()),
            ..Default::default()
        }]),
        resources: Some(ResourceRequirements {
            requests: Some(limits.clone()),
            limits: Some(limits),
            ..Default::default()
        }),
        security_context: Some(SecurityContext {
            run_as_user: Some(0),
            run_as_non_root: Some(false),
            allow_privilege_escalation: Some(false),
            capabilities: Some(Capabilities {
                drop: Some(vec!["ALL".into()]),
                add: Some(CAPABILITIES.iter().map(|c| (*c).to_string()).collect()),
            }),
            seccomp_profile: Some(SeccompProfile {
                type_: "RuntimeDefault".into(),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let pod = PodSpec {
        containers: vec![container],
        restart_policy: Some("Never".into()),
        automount_service_account_token: Some(false),
        enable_service_links: Some(false),
        ..Default::default()
    };
    Job {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            labels: Some(BTreeMap::from([("app".to_string(), APP_LABEL.to_string())])),
            ..Default::default()
        },
        spec: Some(JobSpec {
            backoff_limit: Some(0),
            ttl_seconds_after_finished: Some(TTL_SECONDS),
            active_deadline_seconds: Some(req.deadline.as_secs() as i64),
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

fn selector(pairs: &[(&str, &str)]) -> LabelSelector {
    LabelSelector {
        match_labels: Some(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        ),
        ..Default::default()
    }
}

fn port(protocol: &str, number: u16) -> NetworkPolicyPort {
    NetworkPolicyPort {
        protocol: Some(protocol.into()),
        port: Some(IntOrString::Int(i32::from(number))),
        ..Default::default()
    }
}

fn policy(name: &str, pod: LabelSelector, egress: Vec<NetworkPolicyEgressRule>) -> NetworkPolicy {
    NetworkPolicy {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            labels: Some(BTreeMap::from([("app".to_string(), APP_LABEL.to_string())])),
            ..Default::default()
        },
        spec: Some(NetworkPolicySpec {
            pod_selector: Some(pod),
            policy_types: Some(vec!["Ingress".into(), "Egress".into()]),
            // No ingress rules: nothing may connect to a validation pod.
            ingress: None,
            egress: Some(egress),
        }),
    }
}

/// Never part of "the internet": private, shared (CGNAT, Tailscale),
/// link-local and unique-local ranges, which cover the cluster's own ranges,
/// node IPs and the LAN.
const PRIVATE_CIDRS: [&str; 7] = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "100.64.0.0/10",
    "169.254.0.0/16",
    "fc00::/7",
    "fe80::/10",
];

/// The policies every validation pod gets, selected by label: `validate-base`
/// (DNS and Garage, no ingress) and `validate-internet` (public addresses
/// only, minus `blocked_cidrs`) for pods labeled `allow`.
pub fn base_policies(blocked_cidrs: &[String]) -> Vec<NetworkPolicy> {
    let dns = NetworkPolicyEgressRule {
        to: Some(vec![NetworkPolicyPeer {
            namespace_selector: Some(selector(&[("kubernetes.io/metadata.name", "kube-system")])),
            pod_selector: Some(selector(&[("k8s-app", "kube-dns")])),
            ..Default::default()
        }]),
        ports: Some(vec![port("UDP", 53), port("TCP", 53)]),
    };
    let garage = NetworkPolicyEgressRule {
        to: Some(vec![NetworkPolicyPeer {
            namespace_selector: Some(selector(&[("kubernetes.io/metadata.name", "pedagog-data")])),
            pod_selector: Some(selector(&[("app", "garage")])),
            ..Default::default()
        }]),
        ports: Some(vec![port("TCP", 3900)]),
    };
    let internet = NetworkPolicyEgressRule {
        to: Some(
            ["0.0.0.0/0", "::/0"]
                .iter()
                .map(|all| NetworkPolicyPeer {
                    ip_block: Some(IPBlock {
                        cidr: (*all).to_string(),
                        except: Some(excluded(blocked_cidrs, all.contains(':'))),
                    }),
                    ..Default::default()
                })
                .collect(),
        ),
        ports: None,
    };
    vec![
        policy(
            "validate-base",
            selector(&[(ROLE_LABEL, ROLE)]),
            vec![dns, garage],
        ),
        policy(
            "validate-internet",
            selector(&[(ROLE_LABEL, ROLE), (NETWORK_LABEL, "allow")]),
            vec![internet],
        ),
    ]
}

/// The private ranges plus `blocked`, for one address family.
fn excluded(blocked: &[String], v6: bool) -> Vec<String> {
    let mut out: Vec<String> = PRIVATE_CIDRS.iter().map(|c| (*c).to_owned()).collect();
    for c in blocked {
        if !out.contains(c) {
            out.push(c.clone());
        }
    }
    out.retain(|c| c.contains(':') == v6);
    out
}

/// A run's CIDR allowances, owned by its Job so it's deleted with it.
pub(crate) fn run_policy(
    job_name: &str,
    job_uid: &str,
    cidrs: &[(String, Vec<u16>)],
) -> NetworkPolicy {
    let egress = cidrs
        .iter()
        .map(|(cidr, ports)| NetworkPolicyEgressRule {
            to: Some(vec![NetworkPolicyPeer {
                ip_block: Some(IPBlock {
                    cidr: cidr.clone(),
                    except: None,
                }),
                ..Default::default()
            }]),
            ports: (!ports.is_empty()).then(|| ports.iter().map(|p| port("TCP", *p)).collect()),
        })
        .collect();
    let mut p = policy(
        &format!("{}-net", &job_name[..job_name.len().min(59)]),
        selector(&[("batch.kubernetes.io/job-name", job_name)]),
        egress,
    );
    p.metadata.owner_references = Some(vec![OwnerReference {
        api_version: "batch/v1".into(),
        kind: "Job".into(),
        name: job_name.into(),
        uid: job_uid.into(),
        controller: Some(true),
        block_owner_deletion: Some(true),
    }]);
    p
}

impl Builder {
    /// Create the validation Job for `key` unless it exists (reconcile adopts
    /// it), with its CIDR policy if the run has CIDR allowances.
    pub async fn ensure_validation(
        &self,
        key: &JobKey,
        req: &ValidationRequest,
    ) -> kube::Result<Outcome> {
        let name = key.name();
        if self.client.jobs().get_opt(&name).await?.is_some() {
            return Ok(Outcome::Skipped);
        }
        let Some(created) = self.create_job(&validation_job(&name, req)).await? else {
            return Ok(Outcome::Skipped);
        };
        if let Egress::Cidrs(cidrs) = &req.egress {
            let uid = created.metadata.uid.unwrap_or_default();
            let policy = run_policy(&name, &uid, cidrs);
            self.apply_policy(&policy).await?;
        }
        Ok(Outcome::Created)
    }

    /// Install (or update) the label-selected policies every validation pod gets.
    pub async fn apply_validation_policies(&self, blocked_cidrs: &[String]) -> kube::Result<()> {
        for policy in base_policies(blocked_cidrs) {
            self.apply_policy(&policy).await?;
        }
        Ok(())
    }

    async fn apply_policy(&self, policy: &NetworkPolicy) -> kube::Result<()> {
        let name = policy.metadata.name.clone().unwrap_or_default();
        self.client
            .network_policies()
            .patch(
                &name,
                &PatchParams::apply("pedagog-jobs").force(),
                &Patch::Apply(policy),
            )
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(egress: Egress) -> ValidationRequest {
        ValidationRequest {
            image: "reg:5000/pedagog/assignments/ll:abc".into(),
            results_url: "http://garage/results?sig".into(),
            isolation: "process".into(),
            egress,
            deadline: Duration::from_secs(600),
            cpu: "1".into(),
            memory: "1Gi".into(),
        }
    }

    #[test]
    fn validation_pod_is_locked_down() {
        let job = validation_job("assignment-validate-ll-7", &request(Egress::Deny));
        let spec = job.spec.unwrap();
        assert_eq!(spec.active_deadline_seconds, Some(600));
        assert_eq!(spec.backoff_limit, Some(0));
        let template = spec.template;
        let labels = template.metadata.unwrap().labels.unwrap();
        assert_eq!(labels[ROLE_LABEL], "validate");
        assert_eq!(labels[NETWORK_LABEL], "deny");
        let pod = template.spec.unwrap();
        assert_eq!(pod.automount_service_account_token, Some(false));
        let c = &pod.containers[0];
        assert_eq!(
            c.command.clone().unwrap(),
            [
                "/pedagog/bin/run-tests",
                "--code",
                "/pedagog/solution",
                "--isolation",
                "process"
            ]
        );
        assert_eq!(c.env.clone().unwrap()[0].name, "PEDAGOG_RESULTS_URL");
        let sc = c.security_context.clone().unwrap();
        assert_eq!(sc.run_as_user, Some(0));
        assert_eq!(sc.allow_privilege_escalation, Some(false));
        let caps = sc.capabilities.unwrap();
        assert_eq!(caps.drop.unwrap(), ["ALL"]);
        assert_eq!(caps.add.unwrap(), CAPABILITIES);
        assert_eq!(
            c.resources.clone().unwrap().limits.unwrap()["memory"],
            Quantity("1Gi".into())
        );
    }

    #[test]
    fn egress_modes_label_the_pod() {
        for (egress, label) in [
            (Egress::Internet, "allow"),
            (Egress::Cidrs(vec![("10.0.0.0/8".into(), vec![])]), "rules"),
        ] {
            let job = validation_job("j", &request(egress));
            let labels = job.spec.unwrap().template.metadata.unwrap().labels.unwrap();
            assert_eq!(labels[NETWORK_LABEL], label);
        }
    }

    #[test]
    fn base_policies_allow_dns_and_garage_and_public_internet() {
        let blocked = vec![
            "203.0.113.0/24".to_string(),
            "10.0.0.0/8".to_string(),
            "2001:db8::/32".to_string(),
        ];
        let [base, internet] = base_policies(&blocked).try_into().unwrap();
        let base_spec = base.spec.unwrap();
        assert!(base_spec.ingress.is_none());
        assert_eq!(base_spec.egress.unwrap().len(), 2, "DNS and Garage");
        let rules = internet.spec.unwrap().egress.unwrap();
        let peers = rules[0].to.clone().unwrap();
        let v4 = peers[0].ip_block.clone().unwrap();
        assert_eq!(v4.cidr, "0.0.0.0/0");
        assert_eq!(
            v4.except.unwrap(),
            [
                "10.0.0.0/8",
                "172.16.0.0/12",
                "192.168.0.0/16",
                "100.64.0.0/10",
                "169.254.0.0/16",
                "203.0.113.0/24"
            ],
            "private ranges always, extra blocked ranges once"
        );
        let v6 = peers[1].ip_block.clone().unwrap();
        assert_eq!(v6.cidr, "::/0");
        assert_eq!(
            v6.except.unwrap(),
            ["fc00::/7", "fe80::/10", "2001:db8::/32"]
        );
    }

    #[test]
    fn private_ranges_are_excluded_without_configuration() {
        let [_, internet] = base_policies(&[]).try_into().unwrap();
        let peers = internet.spec.unwrap().egress.unwrap()[0]
            .to
            .clone()
            .unwrap();
        assert_eq!(peers[0].ip_block.clone().unwrap().except.unwrap().len(), 5);
        assert_eq!(peers[1].ip_block.clone().unwrap().except.unwrap().len(), 2);
    }

    #[test]
    fn run_policy_is_owned_by_its_job_and_keeps_ports() {
        let p = run_policy(
            "assignment-validate-ll-7",
            "uid-1",
            &[
                ("10.20.0.0/16".into(), vec![5432]),
                ("1.2.3.4/32".into(), vec![]),
            ],
        );
        assert_eq!(
            p.metadata.name.as_deref(),
            Some("assignment-validate-ll-7-net")
        );
        assert_eq!(p.metadata.owner_references.unwrap()[0].uid, "uid-1");
        let egress = p.spec.unwrap().egress.unwrap();
        assert_eq!(
            egress[0].ports.clone().unwrap()[0].port,
            Some(IntOrString::Int(5432))
        );
        assert!(egress[1].ports.is_none(), "all ports");
    }
}
