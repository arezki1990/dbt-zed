use settings::{RegisterSetting, Settings};

/// Resolved settings for the dbt integration.
#[derive(Clone, Debug, RegisterSetting)]
pub struct DbtSettings {
    /// Maximum number of rows fetched by `dbt show`.
    pub show_limit: u64,
    /// The dbt executable to run.
    pub binary: String,
    /// Download the official dbt Fusion CLI automatically when no dbt binary
    /// is configured or on PATH.
    pub auto_install: bool,
    /// Which dbt Fusion build the auto-installer fetches: a channel name
    /// ("latest", "dev", "canary") or an explicit version like
    /// "2.0.0-preview.218".
    pub fusion_version: String,
    /// "fusion" (default) or "core".
    pub distribution: String,
    /// Adapter installed with dbt Core when auto-installing.
    pub core_adapter: String,
    /// The `--target` to pass, if any.
    pub target: Option<String>,
    /// The `--profiles-dir` to pass, if any.
    pub profiles_dir: Option<String>,
    /// Environment variables set for dbt commands.
    pub env: Vec<(String, String)>,
    /// Explicit dbt project directory; None auto-discovers.
    pub project_dir: Option<String>,
    /// Run `dbt parse` automatically when a dbt project is first detected.
    pub parse_on_load: bool,
    /// Additional dotenv file to load for dbt commands.
    pub env_file: Option<String>,
    /// Lineage graph canvas depth per direction.
    pub lineage_depth: u64,
    /// Lineage tree sidebar depth per direction.
    pub lineage_tree_depth: u64,
    /// Node cap per lineage computation.
    pub lineage_max_nodes: u64,
}

impl Settings for DbtSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let content = content.dbt.clone().unwrap_or_default();
        Self {
            show_limit: content.show_limit.unwrap_or(500),
            binary: content
                .binary
                .filter(|binary| !binary.is_empty())
                .unwrap_or_else(|| "dbt".to_owned()),
            auto_install: content.auto_install.unwrap_or(true),
            fusion_version: content
                .fusion_version
                .filter(|version| !version.is_empty())
                .unwrap_or_else(|| "latest".to_owned()),
            distribution: content
                .distribution
                .filter(|distribution| !distribution.is_empty())
                .unwrap_or_else(|| "fusion".to_owned())
                .to_lowercase(),
            core_adapter: content
                .core_adapter
                .filter(|adapter| !adapter.is_empty())
                .unwrap_or_default()
                .to_lowercase(),
            target: content.target.filter(|target| !target.is_empty()),
            profiles_dir: content.profiles_dir.filter(|dir| !dir.is_empty()),
            env: content
                .env
                .map(|env| env.into_iter().collect())
                .unwrap_or_default(),
            project_dir: content.project_dir.filter(|dir| !dir.is_empty()),
            parse_on_load: content.parse_on_load.unwrap_or(true),
            env_file: content.env_file.filter(|file| !file.is_empty()),
            lineage_depth: content.lineage_depth.unwrap_or(4).max(1),
            lineage_tree_depth: content.lineage_tree_depth.unwrap_or(8).max(1),
            lineage_max_nodes: content.lineage_max_nodes.unwrap_or(500).max(10),
        }
    }
}
