use crate::config::ProviderKind;

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: &'static str,
    pub display_name: &'static str,
    pub provider_kind: ProviderKind,
    pub context_window: u32,
    pub supports_tools: bool,
    pub supports_streaming: bool,
    /// Cost per million input tokens in USD (0.0 for free/local models)
    pub input_cost_per_mtok: f64,
    /// Cost per million output tokens in USD (0.0 for free/local models)
    pub output_cost_per_mtok: f64,
}

pub fn known_models() -> Vec<ModelInfo> {
    vec![
        // ---- Anthropic / Claude ----
        ModelInfo {
            id: "claude-opus-4-7",
            display_name: "Claude Opus 4.7",
            provider_kind: ProviderKind::Claude,
            context_window: 200_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 15.0,
            output_cost_per_mtok: 75.0,
        },
        ModelInfo {
            id: "claude-opus-4-6",
            display_name: "Claude Opus 4.6",
            provider_kind: ProviderKind::Claude,
            context_window: 200_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 15.0,
            output_cost_per_mtok: 75.0,
        },
        ModelInfo {
            id: "claude-sonnet-4-6",
            display_name: "Claude Sonnet 4.6",
            provider_kind: ProviderKind::Claude,
            context_window: 200_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 3.0,
            output_cost_per_mtok: 15.0,
        },
        ModelInfo {
            id: "claude-sonnet-4-5-20241022",
            display_name: "Claude Sonnet 4.5",
            provider_kind: ProviderKind::Claude,
            context_window: 200_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 3.0,
            output_cost_per_mtok: 15.0,
        },
        ModelInfo {
            id: "claude-haiku-4-5-20251001",
            display_name: "Claude Haiku 4.5",
            provider_kind: ProviderKind::Claude,
            context_window: 200_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.80,
            output_cost_per_mtok: 4.0,
        },
        ModelInfo {
            id: "claude-3-5-sonnet-20241022",
            display_name: "Claude 3.5 Sonnet",
            provider_kind: ProviderKind::Claude,
            context_window: 200_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 3.0,
            output_cost_per_mtok: 15.0,
        },
        ModelInfo {
            id: "claude-3-5-haiku-20241022",
            display_name: "Claude 3.5 Haiku",
            provider_kind: ProviderKind::Claude,
            context_window: 200_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.80,
            output_cost_per_mtok: 4.0,
        },
        // ---- OpenAI ----
        ModelInfo {
            id: "gpt-4.1",
            display_name: "GPT-4.1",
            provider_kind: ProviderKind::OpenAI,
            context_window: 1_000_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 2.0,
            output_cost_per_mtok: 8.0,
        },
        ModelInfo {
            id: "gpt-4.1-mini",
            display_name: "GPT-4.1 Mini",
            provider_kind: ProviderKind::OpenAI,
            context_window: 1_000_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.40,
            output_cost_per_mtok: 1.60,
        },
        ModelInfo {
            id: "gpt-4.1-nano",
            display_name: "GPT-4.1 Nano",
            provider_kind: ProviderKind::OpenAI,
            context_window: 1_000_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.10,
            output_cost_per_mtok: 0.40,
        },
        ModelInfo {
            id: "gpt-4o",
            display_name: "GPT-4o",
            provider_kind: ProviderKind::OpenAI,
            context_window: 128_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 2.50,
            output_cost_per_mtok: 10.0,
        },
        ModelInfo {
            id: "gpt-4o-mini",
            display_name: "GPT-4o Mini",
            provider_kind: ProviderKind::OpenAI,
            context_window: 128_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.15,
            output_cost_per_mtok: 0.60,
        },
        ModelInfo {
            id: "o3",
            display_name: "o3",
            provider_kind: ProviderKind::OpenAI,
            context_window: 200_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 2.0,
            output_cost_per_mtok: 8.0,
        },
        ModelInfo {
            id: "o4-mini",
            display_name: "o4-mini",
            provider_kind: ProviderKind::OpenAI,
            context_window: 200_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 1.10,
            output_cost_per_mtok: 4.40,
        },
        ModelInfo {
            id: "o3-pro",
            display_name: "o3-pro",
            provider_kind: ProviderKind::OpenAI,
            context_window: 200_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 20.0,
            output_cost_per_mtok: 80.0,
        },
        // ---- Google / Gemini ----
        ModelInfo {
            id: "gemini-2.5-pro",
            display_name: "Gemini 2.5 Pro",
            provider_kind: ProviderKind::Gemini,
            context_window: 1_000_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 1.25,
            output_cost_per_mtok: 10.0,
        },
        ModelInfo {
            id: "gemini-2.5-flash",
            display_name: "Gemini 2.5 Flash",
            provider_kind: ProviderKind::Gemini,
            context_window: 1_000_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.15,
            output_cost_per_mtok: 0.60,
        },
        ModelInfo {
            id: "gemini-2.0-flash",
            display_name: "Gemini 2.0 Flash",
            provider_kind: ProviderKind::Gemini,
            context_window: 1_000_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.10,
            output_cost_per_mtok: 0.40,
        },
        // ---- Nvidia NIM ----
        ModelInfo {
            id: "meta/llama-3.3-70b-instruct",
            display_name: "Llama 3.3 70B",
            provider_kind: ProviderKind::Nvidia,
            context_window: 128_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.0,
            output_cost_per_mtok: 0.0,
        },
        ModelInfo {
            id: "meta/llama-3.1-405b-instruct",
            display_name: "Llama 3.1 405B",
            provider_kind: ProviderKind::Nvidia,
            context_window: 128_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.0,
            output_cost_per_mtok: 0.0,
        },
        ModelInfo {
            id: "nvidia/llama-3.1-nemotron-70b-instruct",
            display_name: "Nemotron 70B",
            provider_kind: ProviderKind::Nvidia,
            context_window: 128_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.0,
            output_cost_per_mtok: 0.0,
        },
        ModelInfo {
            id: "deepseek-ai/deepseek-r1",
            display_name: "DeepSeek R1 (NIM)",
            provider_kind: ProviderKind::Nvidia,
            context_window: 128_000,
            supports_tools: false,
            supports_streaming: true,
            input_cost_per_mtok: 0.0,
            output_cost_per_mtok: 0.0,
        },
        ModelInfo {
            id: "qwen/qwen2.5-72b-instruct",
            display_name: "Qwen 2.5 72B (NIM)",
            provider_kind: ProviderKind::Nvidia,
            context_window: 128_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.0,
            output_cost_per_mtok: 0.0,
        },
        // ---- Ollama (local) ----
        ModelInfo {
            id: "llama3.1",
            display_name: "Llama 3.1",
            provider_kind: ProviderKind::Ollama,
            context_window: 128_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.0,
            output_cost_per_mtok: 0.0,
        },
        ModelInfo {
            id: "llama3",
            display_name: "Llama 3",
            provider_kind: ProviderKind::Ollama,
            context_window: 8_192,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.0,
            output_cost_per_mtok: 0.0,
        },
        ModelInfo {
            id: "qwen3",
            display_name: "Qwen 3",
            provider_kind: ProviderKind::Ollama,
            context_window: 128_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.0,
            output_cost_per_mtok: 0.0,
        },
        ModelInfo {
            id: "deepseek-r1",
            display_name: "DeepSeek R1",
            provider_kind: ProviderKind::Ollama,
            context_window: 128_000,
            supports_tools: false,
            supports_streaming: true,
            input_cost_per_mtok: 0.0,
            output_cost_per_mtok: 0.0,
        },
        ModelInfo {
            id: "mistral",
            display_name: "Mistral 7B",
            provider_kind: ProviderKind::Ollama,
            context_window: 32_768,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.0,
            output_cost_per_mtok: 0.0,
        },
        ModelInfo {
            id: "codellama",
            display_name: "Code Llama",
            provider_kind: ProviderKind::Ollama,
            context_window: 16_384,
            supports_tools: false,
            supports_streaming: true,
            input_cost_per_mtok: 0.0,
            output_cost_per_mtok: 0.0,
        },
        ModelInfo {
            id: "phi4",
            display_name: "Phi-4",
            provider_kind: ProviderKind::Ollama,
            context_window: 16_384,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.0,
            output_cost_per_mtok: 0.0,
        },
        ModelInfo {
            id: "gemma3",
            display_name: "Gemma 3",
            provider_kind: ProviderKind::Ollama,
            context_window: 128_000,
            supports_tools: true,
            supports_streaming: true,
            input_cost_per_mtok: 0.0,
            output_cost_per_mtok: 0.0,
        },
    ]
}

pub fn context_window_for_model(model_id: &str) -> Option<u32> {
    let models = known_models();
    models
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| m.context_window)
}

pub fn cost_for_model(
    model_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
) -> Option<f64> {
    let models = known_models();
    let info = models.iter().find(|m| m.id == model_id)?;
    if info.input_cost_per_mtok == 0.0 && info.output_cost_per_mtok == 0.0 {
        return None;
    }
    let input_rate = info.input_cost_per_mtok;
    let (cache_read_rate, cache_write_rate) = match info.provider_kind {
        ProviderKind::Claude => (input_rate * 0.1, input_rate * 1.25),
        ProviderKind::OpenAI => (input_rate * 0.5, input_rate),
        _ => (input_rate, input_rate),
    };
    let cost = (input_tokens as f64 * input_rate
        + output_tokens as f64 * info.output_cost_per_mtok
        + cache_read_tokens as f64 * cache_read_rate
        + cache_creation_tokens as f64 * cache_write_rate)
        / 1_000_000.0;
    Some(cost)
}

pub fn provider_kind_for_model(model_id: &str) -> Option<ProviderKind> {
    known_models()
        .into_iter()
        .find(|m| m.id == model_id)
        .map(|m| m.provider_kind)
}

pub fn models_for_provider(kind: ProviderKind) -> Vec<&'static ModelInfo> {
    // Leak the vec so we can return static references — called rarely.
    let models: &'static Vec<ModelInfo> = Box::leak(Box::new(known_models()));
    models.iter().filter(|m| m.provider_kind == kind).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_models_not_empty() {
        assert!(!known_models().is_empty());
    }

    #[test]
    fn all_models_have_nonzero_context() {
        for m in known_models() {
            assert!(m.context_window > 0, "model {} has zero context", m.id);
        }
    }

    #[test]
    fn nvidia_models_present() {
        let models = known_models();
        assert!(
            models
                .iter()
                .any(|m| m.provider_kind == ProviderKind::Nvidia)
        );
    }

    #[test]
    fn each_provider_has_models() {
        let models = known_models();
        for kind in [
            ProviderKind::Claude,
            ProviderKind::OpenAI,
            ProviderKind::Gemini,
            ProviderKind::Nvidia,
            ProviderKind::Ollama,
        ] {
            assert!(
                models.iter().any(|m| m.provider_kind == kind),
                "no models for {:?}",
                kind
            );
        }
    }

    #[test]
    fn provider_kind_for_known_model() {
        assert_eq!(
            provider_kind_for_model("claude-opus-4-7"),
            Some(ProviderKind::Claude)
        );
        assert_eq!(
            provider_kind_for_model("gpt-4.1"),
            Some(ProviderKind::OpenAI)
        );
        assert_eq!(
            provider_kind_for_model("gemini-2.5-pro"),
            Some(ProviderKind::Gemini)
        );
    }

    #[test]
    fn provider_kind_for_unknown_model() {
        assert_eq!(provider_kind_for_model("nonexistent-model-xyz"), None);
    }
}
