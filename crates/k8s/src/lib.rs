//! Kubernetes client wrapper and Kaniko build orchestration.
//!
//! The only crate that touches `kube`/`k8s-openapi`. `build` owns image builds;
//! `validate` runs assignment validation Jobs (and the pod shape every test
//! run shares); `grade` runs grading Jobs and reports test-run capacity.

use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{ConfigMap, Pod};
use k8s_openapi::api::networking::v1::NetworkPolicy;
use kube::{Api, Client};

pub use kube::Error as KubeError;

pub mod build;
pub mod grade;
pub mod runs;
pub mod validate;

/// Whether the API refused a request outright (a 4xx other than a conflict or
/// rate limit): retrying it won't help.
pub fn is_refusal(e: &kube::Error) -> bool {
    matches!(e, kube::Error::Api(status) if (400..500).contains(&status.code) && status.code != 409 && status.code != 429)
}

/// Thin wrapper over [`kube::Client`] pinned to a namespace, shared by build/run.
#[derive(Clone)]
pub struct KubeClient {
    client: Client,
    namespace: String,
}

impl KubeClient {
    /// Connect using the ambient config (in-cluster service account, or kubeconfig in dev).
    pub async fn connect(namespace: impl Into<String>) -> kube::Result<Self> {
        Ok(Self {
            client: Client::try_default().await?,
            namespace: namespace.into(),
        })
    }

    /// The same connection, pinned to another namespace.
    pub fn in_namespace(&self, namespace: impl Into<String>) -> Self {
        Self {
            client: self.client.clone(),
            namespace: namespace.into(),
        }
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub(crate) fn jobs(&self) -> Api<Job> {
        Api::namespaced(self.client.clone(), &self.namespace)
    }

    pub(crate) fn pods(&self) -> Api<Pod> {
        Api::namespaced(self.client.clone(), &self.namespace)
    }

    pub(crate) fn configmaps(&self) -> Api<ConfigMap> {
        Api::namespaced(self.client.clone(), &self.namespace)
    }

    pub(crate) fn network_policies(&self) -> Api<NetworkPolicy> {
        Api::namespaced(self.client.clone(), &self.namespace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(code: u16) -> kube::Error {
        kube::Error::Api(
            kube::core::Status {
                code,
                ..Default::default()
            }
            .boxed(),
        )
    }

    #[test]
    fn only_outright_refusals_are_permanent() {
        for code in [400, 403, 422] {
            assert!(is_refusal(&api(code)), "{code}");
        }
        for code in [409, 429, 500, 503] {
            assert!(!is_refusal(&api(code)), "{code}");
        }
    }
}
