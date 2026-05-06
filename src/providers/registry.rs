use crate::config::schema::{ProviderKind, ProviderProfile};
use crate::providers::traits::{Provider, ProviderError};

pub fn create_provider(
    name: &str,
    profile: &ProviderProfile,
) -> Result<Box<dyn Provider>, ProviderError> {
    let credential = profile.resolve_credential();

    match profile.kind {
        ProviderKind::Claude => crate::providers::anthropic::create(profile, credential.as_deref()),
        ProviderKind::OpenAI => crate::providers::openai::create(profile, credential.as_deref()),
        ProviderKind::Gemini | ProviderKind::Vertex => {
            crate::providers::google::create(name, profile, credential.as_deref())
        }
        ProviderKind::Ollama => crate::providers::ollama::create(profile),
        ProviderKind::LlamaCpp => crate::providers::openai_compat::create_llamacpp(profile),
        ProviderKind::Nvidia => {
            crate::providers::openai_compat::create_nvidia(profile, credential.as_deref())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_provider_missing_cred_for_claude() {
        let profile = ProviderProfile {
            kind: ProviderKind::Claude,
            model: "claude-opus-4-7".into(),
            ..Default::default()
        };
        let result = create_provider("claude", &profile);
        assert!(result.is_err());
    }
}
