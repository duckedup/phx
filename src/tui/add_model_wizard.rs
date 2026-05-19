use crate::config::ProviderKind;

pub struct AddModelWizard {
    pub step: WizardStep,
    pub kind: Option<ProviderKind>,
    pub base_url: String,
    pub api_key: String,
    pub model_name: String,
    pub provider_name: String,
}

pub enum WizardStep {
    SelectKind,
    EnterBaseUrl,
    EnterApiKey,
    EnterModelName,
    EnterProviderName,
    Done,
}

impl AddModelWizard {
    pub fn new() -> Self {
        Self {
            step: WizardStep::SelectKind,
            kind: None,
            base_url: String::new(),
            api_key: String::new(),
            model_name: String::new(),
            provider_name: String::new(),
        }
    }
}

impl Default for AddModelWizard {
    fn default() -> Self {
        Self::new()
    }
}
