//! Canonical export includes negative outcomes, unknown usage, frozen inputs and
//! independent decisions. Artifact custody is supplied by the application.
use super::experiments::ExperimentService;
use admiralty_core::{
    content_digest,
    trials::{TrialError, TrialId},
};

impl ExperimentService {
    pub fn export_dataset(&self, id: &TrialId) -> Result<(String, Vec<u8>), TrialError> {
        let dataset = self.dataset(id)?;
        let digest = content_digest(dataset).map_err(|e| TrialError::Invalid(e.to_string()))?;
        let bytes = serde_json::to_vec(dataset).map_err(|e| TrialError::Invalid(e.to_string()))?;
        Ok((digest, bytes))
    }
}
