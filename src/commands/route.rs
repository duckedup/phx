use crate::config::schema::{Config, ToolRoute};

#[derive(Debug)]
pub struct RouteEntry {
    pub tool: String,
    pub provider: String,
    pub model: Option<String>,
}

#[derive(Debug)]
pub enum RouteResult {
    Table(Vec<RouteEntry>),
    Set { tool: String, provider: String },
    Cleared(String),
    AllCleared,
    Error(String),
}

pub fn handle(args: &str, config: &Config) -> RouteResult {
    let args = args.trim();

    if args.is_empty() {
        let entries: Vec<RouteEntry> = config
            .tool_routing
            .iter()
            .map(|(tool, route)| RouteEntry {
                tool: tool.clone(),
                provider: route.provider.clone(),
                model: route.model.clone(),
            })
            .collect();
        return RouteResult::Table(entries);
    }

    if args == "clear" {
        return RouteResult::AllCleared;
    }

    let parts: Vec<&str> = args.split_whitespace().collect();
    match parts.as_slice() {
        [tool, "clear"] => RouteResult::Cleared(tool.to_string()),
        [tool, provider] => {
            if !config.providers.contains_key(*provider) {
                return RouteResult::Error(format!("Unknown provider: {provider}"));
            }
            RouteResult::Set {
                tool: tool.to_string(),
                provider: provider.to_string(),
            }
        }
        _ => RouteResult::Error("Usage: /route [tool provider | tool clear | clear]".into()),
    }
}

pub fn apply(result: &RouteResult, config: &mut Config) -> Option<String> {
    match result {
        RouteResult::Set { tool, provider } => {
            config.tool_routing.insert(
                tool.clone(),
                ToolRoute {
                    provider: provider.clone(),
                    model: None,
                },
            );
            Some(format!("{tool} → {provider}"))
        }
        RouteResult::Cleared(tool) => {
            config.tool_routing.remove(tool);
            Some(format!("Cleared route for {tool}"))
        }
        RouteResult::AllCleared => {
            config.tool_routing.clear();
            Some("All routes cleared".into())
        }
        _ => None,
    }
}
