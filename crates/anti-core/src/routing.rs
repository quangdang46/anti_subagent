//! Model routing — config-based, not hard-coded.
//!
//! Disposition × Complexity → CapabilityTier → model (from provider config).
//! Model names come from provider config, NOT from anti-core.

use crate::model::Disposition;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Capability tier — determines what level of model is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityTier {
    /// Quick lookups, search, narrow checks
    Lightweight,
    /// Standard implementation, debugging, reviews
    Standard,
    /// Architecture, deep analysis, complex refactors
    Heavyweight,
}

/// Task complexity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Complexity {
    Low,
    Medium,
    High,
}

/// Model route — resolved from disposition × complexity × config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoute {
    pub disposition: Disposition,
    pub complexity: Complexity,
    pub capability: CapabilityTier,
    pub provider: String,
    pub model: String,
}

/// Provider configuration — loaded from .anti_subagent/providers.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub providers: HashMap<String, ProviderTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderTier {
    pub lightweight: String,
    pub standard: String,
    pub heavyweight: String,
}

impl ProviderConfig {
    /// Load from TOML config string.
    pub fn from_toml(toml_str: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let config: ProviderConfig = toml::from_str(toml_str)?;
        Ok(config)
    }

    /// Resolve capability tier to model name for a provider.
    pub fn resolve(&self, provider: &str, capability: &CapabilityTier) -> String {
        self.providers
            .get(provider)
            .map(|tier| match capability {
                CapabilityTier::Lightweight => tier.lightweight.clone(),
                CapabilityTier::Standard => tier.standard.clone(),
                CapabilityTier::Heavyweight => tier.heavyweight.clone(),
            })
            .unwrap_or_else(|| format!("unknown-provider-{:?}", capability))
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        let mut providers = HashMap::new();
        providers.insert(
            "claude".to_string(),
            ProviderTier {
                lightweight: "haiku".to_string(),
                standard: "sonnet".to_string(),
                heavyweight: "opus".to_string(),
            },
        );
        providers.insert(
            "codex".to_string(),
            ProviderTier {
                lightweight: "gpt-4o-mini".to_string(),
                standard: "gpt-4o".to_string(),
                heavyweight: "o3".to_string(),
            },
        );
        Self { providers }
    }
}

/// Resolve model route from disposition and complexity.
pub fn resolve_route(
    disposition: Disposition,
    complexity: Complexity,
    config: &ProviderConfig,
    default_provider: &str,
) -> ModelRoute {
    let capability = match (&disposition, &complexity) {
        (Disposition::Scout, _) | (Disposition::Shadow, _) => CapabilityTier::Lightweight,
        (Disposition::Engineer, Complexity::Low) => CapabilityTier::Standard,
        (Disposition::Engineer, Complexity::High) => CapabilityTier::Heavyweight,
        (Disposition::Engineer, Complexity::Medium) => CapabilityTier::Standard,
        (Disposition::ProofAuditor, _) => CapabilityTier::Heavyweight,
        (Disposition::Reviewer, _) => CapabilityTier::Standard,
        (Disposition::Architect, _) => CapabilityTier::Standard,
    };

    let model = config.resolve(default_provider, &capability);

    ModelRoute {
        disposition,
        complexity,
        capability,
        provider: default_provider.to_string(),
        model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scout_always_lightweight() {
        let config = ProviderConfig::default();
        let route = resolve_route(Disposition::Scout, Complexity::High, &config, "claude");
        assert_eq!(route.capability, CapabilityTier::Lightweight);
        assert_eq!(route.model, "haiku");
    }

    #[test]
    fn engineer_high_complexity_heavyweight() {
        let config = ProviderConfig::default();
        let route = resolve_route(Disposition::Engineer, Complexity::High, &config, "claude");
        assert_eq!(route.capability, CapabilityTier::Heavyweight);
        assert_eq!(route.model, "opus");
    }

    #[test]
    fn engineer_low_complexity_standard() {
        let config = ProviderConfig::default();
        let route = resolve_route(Disposition::Engineer, Complexity::Low, &config, "claude");
        assert_eq!(route.capability, CapabilityTier::Standard);
        assert_eq!(route.model, "sonnet");
    }

    #[test]
    fn proof_auditor_always_heavyweight() {
        let config = ProviderConfig::default();
        let route = resolve_route(Disposition::ProofAuditor, Complexity::Low, &config, "claude");
        assert_eq!(route.capability, CapabilityTier::Heavyweight);
        assert_eq!(route.model, "opus");
    }

    #[test]
    fn codex_provider() {
        let config = ProviderConfig::default();
        let route = resolve_route(Disposition::Engineer, Complexity::Medium, &config, "codex");
        assert_eq!(route.provider, "codex");
        assert_eq!(route.model, "gpt-4o");
    }
}
