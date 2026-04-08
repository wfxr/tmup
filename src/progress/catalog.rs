use std::collections::HashMap;

use crate::model::Config;

/// Plugin display metadata used only to seed reducer snapshot state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DisplayPlugin {
    /// Canonical plugin id.
    pub(crate) id: String,
    /// Human-readable display label.
    pub(crate) label: String,
}

/// Initialization-only helper for building the first snapshot plugin order/labels.
///
/// This catalog is not consulted for runtime lookup after reporter construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DisplayCatalog {
    plugins: Vec<DisplayPlugin>,
}

impl DisplayCatalog {
    /// Build ordered display metadata from config and optional target id.
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

        for (id, name) in remote_plugins {
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

            plugins.push(DisplayPlugin { id: id.to_string(), label });
        }

        Self { plugins }
    }

    /// Return the number of plugins in this display catalog.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Iterate through initial plugin metadata in stable declaration order.
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
    use crate::model::{Config, Options};
    use crate::progress::test_support::remote_plugin;

    #[test]
    fn display_catalog_assigns_stable_order_and_labels() {
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
        let all_visible: Vec<_> =
            all.iter().map(|plugin| (plugin.id.as_str(), plugin.label.as_str())).collect();
        assert_eq!(
            all_visible,
            vec![
                ("github.com/tmux-plugins/tmux-sensible", "tmux-plugins/tmux-sensible"),
                ("github.com/acme/tmux-sensible", "acme/tmux-sensible"),
                ("github.com/tmux-plugins/tmux-yank", "tmux-yank"),
            ]
        );

        let filtered =
            DisplayCatalog::from_config(&config, Some("github.com/tmux-plugins/tmux-yank"));
        assert_eq!(filtered.len(), 1);
        let filtered_visible: Vec<_> =
            filtered.iter().map(|plugin| (plugin.id.as_str(), plugin.label.as_str())).collect();
        assert_eq!(filtered_visible, vec![("github.com/tmux-plugins/tmux-yank", "tmux-yank")]);

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
