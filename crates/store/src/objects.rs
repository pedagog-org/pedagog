//! S3-compatible object storage (Garage) for assignment archives and run results.
//!
//! Only platform services hold credentials. Build and run pods get short-lived
//! presigned URLs instead, so no S3 secret ever enters a pod running instructor
//! or student code.

use std::time::Duration;

use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path;
use object_store::signer::{Method, Signer};
use object_store::{ObjectStore, ObjectStoreExt, PutPayload, WriteMultipart};

pub use object_store::signer::Url;
pub use object_store::{Error, Result};

pub const S3_ENDPOINT: &str = "PEDAGOG_S3_ENDPOINT";
pub const S3_REGION: &str = "PEDAGOG_S3_REGION";
pub const S3_BUCKET: &str = "PEDAGOG_S3_BUCKET";
pub const S3_ACCESS_KEY_ID: &str = "PEDAGOG_S3_ACCESS_KEY_ID";
pub const S3_SECRET_ACCESS_KEY: &str = "PEDAGOG_S3_SECRET_ACCESS_KEY";

// Garage's default region name.
const DEFAULT_REGION: &str = "garage";

#[derive(Clone)]
pub struct ObjectsConfig {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

impl ObjectsConfig {
    /// Reads the config from `PEDAGOG_S3_*` env vars; the region defaults to Garage's.
    pub fn from_env() -> std::result::Result<Self, String> {
        let var = |name: &str| std::env::var(name).map_err(|_| format!("{name} is not set"));
        Ok(Self {
            endpoint: var(S3_ENDPOINT)?,
            region: std::env::var(S3_REGION).unwrap_or_else(|_| DEFAULT_REGION.to_owned()),
            bucket: var(S3_BUCKET)?,
            access_key_id: var(S3_ACCESS_KEY_ID)?,
            secret_access_key: var(S3_SECRET_ACCESS_KEY)?,
        })
    }
}

// Hand-written so the secret never ends up in logs.
impl std::fmt::Debug for ObjectsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectsConfig")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("access_key_id", &self.access_key_id)
            .finish_non_exhaustive()
    }
}

/// A bucket on the platform's object store.
#[derive(Clone, Debug)]
pub struct Objects {
    store: AmazonS3,
}

impl Objects {
    pub fn connect(cfg: &ObjectsConfig) -> Result<Self> {
        let store = AmazonS3Builder::new()
            .with_endpoint(&cfg.endpoint)
            .with_region(&cfg.region)
            .with_bucket_name(&cfg.bucket)
            .with_access_key_id(&cfg.access_key_id)
            .with_secret_access_key(&cfg.secret_access_key)
            // In-cluster Garage serves plain HTTP on a private network, path-style.
            .with_allow_http(cfg.endpoint.starts_with("http://"))
            .with_virtual_hosted_style_request(false)
            .build()?;
        Ok(Self { store })
    }

    pub async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        self.store
            .put(&Path::from(key), PutPayload::from(bytes))
            .await?;
        Ok(())
    }

    pub async fn get(&self, key: &str) -> Result<Vec<u8>> {
        let result = self.store.get(&Path::from(key)).await?;
        Ok(result.bytes().await?.to_vec())
    }

    /// A URL a pod can GET `key` from without credentials, valid for `ttl`.
    pub async fn presign_get(&self, key: &str, ttl: Duration) -> Result<Url> {
        self.store
            .signed_url(Method::GET, &Path::from(key), ttl)
            .await
    }

    /// Write `key` in parts as they come, so a large file is never held whole.
    pub async fn upload(&self, key: &str) -> Result<Upload> {
        let parts = self.store.put_multipart(&Path::from(key)).await?;
        Ok(Upload(WriteMultipart::new(parts)))
    }

    pub async fn exists(&self, key: &str) -> Result<bool> {
        match self.store.head(&Path::from(key)).await {
            Ok(_) => Ok(true),
            Err(Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The keys directly under `prefix` (one level, as ingredients are kept).
    pub async fn keys_under(&self, prefix: &str) -> Result<Vec<String>> {
        let listed = self
            .store
            .list_with_delimiter(Some(&Path::from(prefix)))
            .await?;
        Ok(listed
            .objects
            .into_iter()
            .map(|o| o.location.to_string())
            .collect())
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        self.store.delete(&Path::from(key)).await
    }

    /// A URL a pod can PUT `key` to without credentials, valid for `ttl`.
    pub async fn presign_put(&self, key: &str, ttl: Duration) -> Result<Url> {
        self.store
            .signed_url(Method::PUT, &Path::from(key), ttl)
            .await
    }
}

/// An object being written in parts ([`Objects::upload`]); nothing is stored
/// unless it's finished.
pub struct Upload(WriteMultipart);

impl Upload {
    /// Add `bytes`, waiting while too many parts are in flight (bounded memory).
    pub async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.0.wait_for_capacity(4).await?;
        self.0.write(bytes);
        Ok(())
    }

    pub async fn finish(self) -> Result<()> {
        self.0.finish().await.map(|_| ())
    }

    pub async fn abort(self) -> Result<()> {
        self.0.abort().await
    }
}

/// Object key for an uploaded assignment archive, by content hash.
pub fn archive_key(course: &str, slug: &str, sha256: &str) -> String {
    format!("assignments/{course}/{slug}/{sha256}.tgz")
}

/// Object key for a submission's normalized tree, by content hash.
pub fn submission_key(course: &str, slug: &str, tree_hash: &str) -> String {
    format!("submissions/{course}/{slug}/{tree_hash}.tgz")
}

/// Where ingredients are kept: once per content, shared by every recipe.
pub const INGREDIENTS_PREFIX: &str = "ingredients/sha256";

/// Object key for an ingredient, by its SHA-256.
pub fn ingredient_key(sha256: &str) -> String {
    format!("{INGREDIENTS_PREFIX}/{sha256}")
}

/// Object key a grading run uploads its `results.yaml` to.
pub fn grading_results_key(run_id: i64) -> String {
    format!("results/grading/{run_id}/results.yaml")
}

/// Object key a validation run (by build id) uploads its `results.yaml` to.
pub fn results_key(run_id: i64) -> String {
    format!("results/{run_id}/results.yaml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_namespaced_and_stable() {
        assert_eq!(
            archive_key("cop3502-f26", "pointers", "ab12"),
            "assignments/cop3502-f26/pointers/ab12.tgz"
        );
        assert_eq!(results_key(42), "results/42/results.yaml");
    }

    #[test]
    fn debug_never_prints_the_secret() {
        let cfg = ObjectsConfig {
            endpoint: "http://garage:3900".into(),
            region: "garage".into(),
            bucket: "pedagog".into(),
            access_key_id: "GKabc".into(),
            secret_access_key: "super-secret".into(),
        };
        assert!(!format!("{cfg:?}").contains("super-secret"));
    }

    /// Round-trips through a real S3 endpoint when one is configured (local
    /// Garage or CI); skipped otherwise.
    #[tokio::test]
    async fn put_get_and_presigned_urls_against_live_s3() {
        let Ok(cfg) = ObjectsConfig::from_env() else {
            eprintln!("skipping: {S3_ENDPOINT} etc. not set");
            return;
        };
        let objects = Objects::connect(&cfg).unwrap();
        let key = format!("test/{}", std::process::id());
        objects.put(&key, b"hello".to_vec()).await.unwrap();
        assert_eq!(objects.get(&key).await.unwrap(), b"hello");

        let put_url = objects
            .presign_put(&format!("{key}-presigned"), Duration::from_secs(60))
            .await
            .unwrap();
        let status = std::process::Command::new("curl")
            .args([
                "-fsS",
                "-X",
                "PUT",
                "--data-binary",
                "via-url",
                put_url.as_str(),
            ])
            .status()
            .unwrap();
        assert!(status.success(), "presigned PUT failed");

        let get_url = objects
            .presign_get(&format!("{key}-presigned"), Duration::from_secs(60))
            .await
            .unwrap();
        let out = std::process::Command::new("curl")
            .args(["-fsS", get_url.as_str()])
            .output()
            .unwrap();
        assert_eq!(out.stdout, b"via-url");
    }
}
