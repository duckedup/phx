use std::collections::HashMap;
use std::sync::Arc;

use crate::config::schema::Config;
use crate::providers::registry::create_provider;
use crate::providers::traits::Provider;

pub struct ToolRouter {
    routes: HashMap<String, String>,
    providers: HashMap<String, Arc<dyn Provider>>,
}

impl ToolRouter {
    pub fn from_config(config: &Config) -> Option<Self> {
        if config.tool_routing.is_empty() {
            return None;
        }

        let mut routes = HashMap::new();
        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();

        for (tool, route) in &config.tool_routing {
            let provider_name = &route.provider;
            routes.insert(tool.clone(), provider_name.clone());

            if providers.contains_key(provider_name) {
                continue;
            }

            if let Some(mut profile) = config.providers.get(provider_name).cloned() {
                if let Some(ref model) = route.model {
                    profile.model = model.clone();
                }
                match create_provider(provider_name, &profile) {
                    Ok(p) => {
                        providers.insert(provider_name.clone(), Arc::from(p));
                    }
                    Err(e) => {
                        tracing::warn!(
                            "tool routing: failed to create provider '{}': {}",
                            provider_name,
                            e
                        );
                    }
                }
            } else {
                tracing::warn!(
                    "tool routing: provider '{}' not found in config",
                    provider_name
                );
            }
        }

        Some(Self { routes, providers })
    }

    pub fn provider_for_tool(&self, tool_name: &str) -> Option<Arc<dyn Provider>> {
        let provider_name = self.routes.get(tool_name)?;
        self.providers.get(provider_name).cloned()
    }

    pub fn is_routed(&self, tool_name: &str) -> bool {
        self.routes.contains_key(tool_name)
    }

    pub fn routed_tool_names(&self, provider_name: &str) -> Vec<String> {
        self.routes
            .iter()
            .filter(|(_, pn)| *pn == provider_name)
            .map(|(tool, _)| tool.clone())
            .collect()
    }
}
