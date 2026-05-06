use crate::config::schema::ProviderProfile;
use crate::providers::traits::{Provider, ProviderError};

pub fn create_llamacpp(profile: &ProviderProfile) -> Result<Box<dyn Provider>, ProviderError> {
    let base_url = profile
        .base_url
        .clone()
        .unwrap_or_else(|| "http://localhost:8080".into());

    let mut modified = profile.clone();
    modified.base_url = Some(base_url);

    crate::providers::openai::create(&modified, Some("no-key-needed"))
}

pub fn create_nvidia(
    profile: &ProviderProfile,
    credential: Option<&str>,
) -> Result<Box<dyn Provider>, ProviderError> {
    let base_url = profile
        .base_url
        .clone()
        .unwrap_or_else(|| "https://integrate.api.nvidia.com".into());

    let mut modified = profile.clone();
    modified.base_url = Some(base_url);

    crate::providers::openai::create(&modified, credential)
}
