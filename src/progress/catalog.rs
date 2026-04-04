#![allow(dead_code)]

use std::collections::HashMap;

use crate::model::Config;

/// Stable plugin display metadata used by progress renderers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DisplayPlugin {
    /// Canonical plugin id.
    pub(crate) id: String,
    /// Human-readable display label.
    pub(crate) label: String,
    /// Stable line slot index used by fixed-row renderers.
    pub(crate) slot: usize,
}

/// Stable ordered plugin catalog used by progress reducers/renderers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DisplayCatalog {
    plugins: Vec<DisplayPlugin>,
    by_id: HashMap<String, usize>,
}

impl DisplayCatalog {
    /// Build a stable ordered display catalog from config and optional target id.
    pub(crate) fn from_config(config: &Config, target_id: Option<&str>) -> Self {
        let mut by_name: HashMap<&str, Vec<&str>> = HashMap::new();
        let remote_plugins: Vec<_> = config
            .plugins
            .iter()
            .filter_map(|plugin| {
                let id = plugin.remote_id()?;
                target_id.is_none_or(|target| target == id).then_some((id, plugin.name.as_str()))
            })
            .collect();

        for (id, name) in &remote_plugins {
            by_name.entry(name).or_default().push(id);
        }

        let mut plugins = Vec::with_capacity(remote_plugins.len());
        let mut by_id = HashMap::with_capacity(remote_plugins.len());

        for (slot, (id, name)) in remote_plugins.into_iter().enumerate() {
            let colliding_ids = &by_name[name];
            let label = if colliding_ids.len() == 1 {
                name.to_string()
            } else {
                let short = short_remote_id(id);
                let short_is_unique =
                    colliding_ids.iter().filter(|other| short_remote_id(other) == short).count()
                        == 1;
                if short_is_unique { short.to_string() } else { id.to_string() }
            };

            let id_owned = id.to_string();
            by_id.insert(id_owned.clone(), slot);
            plugins.push(DisplayPlugin { id: id_owned, label, slot });
        }

        Self { plugins, by_id }
    }

    /// Return plugin metadata by canonical id.
    pub(crate) fn plugin(&self, id: &str) -> Option<&DisplayPlugin> {
        self.by_id.get(id).and_then(|idx| self.plugins.get(*idx))
    }

    /// Resolve the display label for an id, falling back to provided name.
    pub(crate) fn label_for<'a>(&'a self, id: &'a str, fallback_name: &'a str) -> &'a str {
        self.plugin(id).map(|plugin| plugin.label.as_str()).unwrap_or(fallback_name)
    }

    /// Resolve the stable slot index for an id.
    pub(crate) fn slot_for(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).copied()
    }

    /// Return the number of plugins in this display catalog.
    pub(crate) fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Iterate through display plugin metadata in slot order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &DisplayPlugin> {
        self.plugins.iter()
    }
}

fn short_remote_id(id: &str) -> &str {
    id.split_once('/').map(|(_, tail)| tail).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::DisplayCatalog;
    use crate::model::{Config, Options, PluginSource, PluginSpec, Tracking};

    fn remote_plugin(raw: &str, id: &str, name: &str) -> PluginSpec {
        PluginSpec {
            source: PluginSource::Remote {
                raw: raw.to_string(),
                id: id.to_string(),
                clone_url: format!("https://{id}.git"),
            },
            name: name.to_string(),
            opt_prefix: "@plugin".to_string(),
            tracking: Tracking::DefaultBranch,
            build: None,
            opts: Vec::new(),
        }
    }

    #[test]
    fn display_catalog_assigns_stable_slots() {
        let config = Config {
            options: Options::default(),
            plugins: vec![
                remote_plugin(
                    "tmux-plugins/tmux-sensible",
                    "github.com/tmux-plugins/tmux-sensible",
                    "tmux-sensible",
                ),
                remote_plugin(
                    "acme/tmux-sensible",
                    "github.com/acme/tmux-sensible",
                    "tmux-sensible",
                ),
                remote_plugin(
                    "tmux-plugins/tmux-yank",
                    "github.com/tmux-plugins/tmux-yank",
                    "tmux-yank",
                ),
            ],
        };

        let all = DisplayCatalog::from_config(&config, None);
        assert_eq!(all.len(), 3);
        assert_eq!(all.slot_for("github.com/tmux-plugins/tmux-sensible"), Some(0));
        assert_eq!(all.slot_for("github.com/acme/tmux-sensible"), Some(1));
        assert_eq!(all.slot_for("github.com/tmux-plugins/tmux-yank"), Some(2));

        assert_eq!(
            all.label_for("github.com/tmux-plugins/tmux-sensible", "fallback"),
            "tmux-plugins/tmux-sensible"
        );
        assert_eq!(
            all.label_for("github.com/acme/tmux-sensible", "fallback"),
            "acme/tmux-sensible"
        );
        assert_eq!(all.label_for("github.com/tmux-plugins/tmux-yank", "fallback"), "tmux-yank");
        assert_eq!(all.label_for("github.com/unknown/plugin", "fallback"), "fallback");

        let filtered =
            DisplayCatalog::from_config(&config, Some("github.com/tmux-plugins/tmux-yank"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered.slot_for("github.com/tmux-plugins/tmux-yank"), Some(0));
        assert_eq!(filtered.slot_for("github.com/tmux-plugins/tmux-sensible"), None);
        assert_eq!(
            filtered.label_for("github.com/tmux-plugins/tmux-yank", "fallback"),
            "tmux-yank"
        );

        let visible_ids: Vec<_> = all.iter().map(|plugin| plugin.id.as_str()).collect();
        assert_eq!(
            visible_ids,
            vec![
                "github.com/tmux-plugins/tmux-sensible",
                "github.com/acme/tmux-sensible",
                "github.com/tmux-plugins/tmux-yank",
            ]
        );
    }
}
