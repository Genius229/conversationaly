//! Persisted onboarding state, independent of native model/network adapters.
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OnboardingStatus {
    pub version: String,
    pub completed: bool,
    pub current_step: u8,
    pub model_status: ModelStatus,
    pub last_updated: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ModelStatus {
    pub parakeet: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_summary_model: Option<String>,
}

impl Default for OnboardingStatus {
    fn default() -> Self {
        Self {
            version: "1.0".to_string(),
            completed: false,
            current_step: 1,
            model_status: ModelStatus {
                parakeet: "not_downloaded".to_string(),
                summary: "not_downloaded".to_string(),
                selected_summary_model: None,
            },
            last_updated: chrono::Utc::now().to_rfc3339(),
        }
    }
}

impl OnboardingStatus {
    /// Completing the wizard records observed readiness, never a promised download.
    pub fn complete_with_models(&mut self, model: String, live_ready: bool, summary_ready: bool) {
        self.completed = true;
        self.current_step = 4;
        self.model_status.parakeet = readiness_label(live_ready).into();
        self.model_status.summary = readiness_label(summary_ready).into();
        self.model_status.selected_summary_model = Some(model);
    }
}

fn readiness_label(ready: bool) -> &'static str {
    if ready {
        "downloaded"
    } else {
        "not_downloaded"
    }
}

#[cfg(test)]
mod tests {
    use super::OnboardingStatus;

    #[test]
    fn setup_can_complete_with_no_downloaded_models() {
        let mut status = OnboardingStatus::default();
        status.complete_with_models("gemma4:e2b".into(), false, false);
        assert!(status.completed);
        assert_eq!(status.current_step, 4);
        assert_eq!(status.model_status.parakeet, "not_downloaded");
        assert_eq!(status.model_status.summary, "not_downloaded");
        assert_eq!(
            status.model_status.selected_summary_model.as_deref(),
            Some("gemma4:e2b")
        );
        let restored: OnboardingStatus =
            serde_json::from_str(&serde_json::to_string(&status).unwrap()).unwrap();
        assert!(restored.completed);
        assert_eq!(restored.model_status.summary, "not_downloaded");
    }

    #[test]
    fn completion_records_only_models_verified_ready() {
        let mut status = OnboardingStatus::default();
        status.complete_with_models("gemma4:e4b".into(), true, false);
        assert_eq!(status.model_status.parakeet, "downloaded");
        assert_eq!(status.model_status.summary, "not_downloaded");
        status.complete_with_models("gemma4:e4b".into(), false, true);
        assert_eq!(status.model_status.parakeet, "not_downloaded");
        assert_eq!(status.model_status.summary, "downloaded");
    }
}
