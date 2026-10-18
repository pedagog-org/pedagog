//! What a build's test runs need (platform, isolation, egress, submission
//! rules), saved on the build at dispatch so admission and intake read one row
//! instead of re-deriving it from the stored archive. Builds from before it was
//! saved get it derived once and backfilled.

use pedagog_core::recipe::assignment::{AssignmentYaml, Isolation};
use pedagog_core::recipe::network::{EgressTarget, NetworkSpec};
use pedagog_core::recipe::platform::PlatformKind;
use pedagog_core::recipe::submission::SubmissionRules;
use pedagog_k8s::validate::Egress;
use pedagog_store::db::assignments as db;
use pedagog_store::{AssignmentBuild, BuildSettings};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct RunSettings {
    pub platform: PlatformKind,
    pub isolation: Isolation,
    pub egress: Egress,
    pub submission: SubmissionRules,
}

/// `Egress` as stored.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
enum StoredEgress {
    Deny,
    Internet,
    Cidrs { cidrs: Vec<(String, Vec<u16>)> },
}

impl RunSettings {
    pub fn of(assignment: &AssignmentYaml) -> Self {
        Self {
            platform: assignment.environment.platform.kind.clone(),
            isolation: assignment
                .testing
                .as_ref()
                .map(|t| t.isolation)
                .unwrap_or_default(),
            egress: egress(&assignment.environment.network),
            submission: assignment.submission.clone(),
        }
    }

    pub fn to_stored(&self) -> anyhow::Result<BuildSettings> {
        let egress = match &self.egress {
            Egress::Deny => StoredEgress::Deny,
            Egress::Internet => StoredEgress::Internet,
            Egress::Cidrs(cidrs) => StoredEgress::Cidrs {
                cidrs: cidrs.clone(),
            },
        };
        Ok(BuildSettings {
            platform: self.platform.clone(),
            isolation: self.isolation,
            egress: serde_json::to_string(&egress)?,
            submission: serde_json::to_string(&self.submission)?,
        })
    }

    pub fn from_stored(stored: &BuildSettings) -> anyhow::Result<Self> {
        let egress = match serde_json::from_str(&stored.egress)? {
            StoredEgress::Deny => Egress::Deny,
            StoredEgress::Internet => Egress::Internet,
            StoredEgress::Cidrs { cidrs } => Egress::Cidrs(cidrs),
        };
        Ok(Self {
            platform: stored.platform.clone(),
            isolation: stored.isolation,
            egress,
            submission: serde_json::from_str(&stored.submission)?,
        })
    }
}

/// A build's settings: from its row, or (for a build from before they were
/// saved) derived from its stored archive and backfilled.
pub async fn settings_of(state: &AppState, build: &AssignmentBuild) -> anyhow::Result<RunSettings> {
    if let Some(stored) = &build.settings {
        return RunSettings::from_stored(stored);
    }
    let upload = db::get_upload(&state.pool, build.upload_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("build {} has no upload {}", build.id, build.upload_id))?;
    let settings = crate::assignments::plan_for(state, &upload, false)
        .await?
        .settings;
    db::set_build_settings(&state.pool, build.id, &settings.to_stored()?).await?;
    tracing::info!(build = build.id, "backfilled run settings");
    Ok(settings)
}

/// `environment.network` as test-run egress. Host rules need the egress proxy
/// and aren't enforced yet, so they grant nothing (fail closed).
pub fn egress(network: &NetworkSpec) -> Egress {
    match network {
        NetworkSpec::Allow => Egress::Internet,
        NetworkSpec::Deny { allow } => {
            let cidrs: Vec<(String, Vec<u16>)> = allow
                .iter()
                .filter_map(|rule| match &rule.target {
                    EgressTarget::Cidr(net) => Some((
                        net.to_string(),
                        rule.ports.iter().map(|p| p.get()).collect(),
                    )),
                    EgressTarget::Host(_) => None,
                })
                .collect();
            if cidrs.is_empty() {
                Egress::Deny
            } else {
                Egress::Cidrs(cidrs)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assignment(extra: &str) -> AssignmentYaml {
        serde_yaml::from_str(&format!(
            "id: x\nname: X\nenvironment:\n  os: ubuntu-22\n  platform:\n    kind: upload\n{extra}"
        ))
        .unwrap()
    }

    #[test]
    fn network_maps_to_test_run_egress() {
        let spec = |yaml: &str| serde_yaml::from_str::<NetworkSpec>(yaml).unwrap();
        assert_eq!(egress(&NetworkSpec::default()), Egress::Deny);
        assert_eq!(egress(&spec("mode: allow\n")), Egress::Internet);
        assert_eq!(
            egress(&spec("mode: deny\nallow:\n  - host: pypi.org\n")),
            Egress::Deny,
            "host rules grant nothing yet"
        );
        assert_eq!(
            egress(&spec(
                "mode: deny\nallow:\n  - cidr: 10.20.0.0/16\n    ports: [5432]\n  - host: a.com\n"
            )),
            Egress::Cidrs(vec![("10.20.0.0/16".into(), vec![5432])])
        );
    }

    #[test]
    fn settings_round_trip_through_storage() {
        let a = assignment(
            "  network:\n    mode: deny\n    allow:\n      - cidr: 10.20.0.0/16\n        ports: [5432]\ntesting:\n  framework: pytest:1\n  isolation: in-process\nsubmission:\n  include: ['*.py']\n  required: [main.py]\n",
        );
        let settings = RunSettings::of(&a);
        let back = RunSettings::from_stored(&settings.to_stored().unwrap()).unwrap();
        assert_eq!(back.platform, PlatformKind::Upload);
        assert_eq!(back.isolation, Isolation::InProcess);
        assert_eq!(
            back.egress,
            Egress::Cidrs(vec![("10.20.0.0/16".into(), vec![5432])])
        );
        assert!(back.submission.accepts("main.py") && !back.submission.accepts("notes.txt"));
    }

    #[test]
    fn defaults_are_process_isolation_and_no_egress() {
        let settings = RunSettings::of(&assignment(""));
        assert_eq!(settings.isolation, Isolation::Process);
        assert_eq!(settings.egress, Egress::Deny);
        let stored = settings.to_stored().unwrap();
        assert_eq!(stored.egress, r#"{"mode":"deny"}"#);
    }
}
