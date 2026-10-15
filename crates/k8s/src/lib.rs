//! Kubernetes client wrapper and Kaniko build orchestration.
//!
//! The only crate that touches `kube`/`k8s-openapi`. `build` owns image builds;
//! `validate` runs assignment validation Jobs; `run` (session/submission pods)
//! will land later.

use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{ConfigMap, Pod};
use k8s_openapi::api::networking::v1::NetworkPolicy;
use kube::{Api, Client};

pub mod build;
pub mod validate;

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
