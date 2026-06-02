use serde::Deserialize;
use std::error::Error;
use std::path::PathBuf;

static APP_NAME: &str = "tray-host";

#[derive(Deserialize, Debug)]
pub struct Config {
    /// Sort tray items alphabetically by title.
    #[serde(default = "sorting_default")]
    pub sorting: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sorting: sorting_default(),
        }
    }
}

impl Config {
    /// Load configuration from a TOML file, or return defaults.
    ///
    /// If `path` is `Some`, loads from that file.
    /// If `path` is `None`, tries `$XDG_CONFIG_HOME/tray-host/config.toml`,
    /// falling back to defaults if the file doesn't exist.
    ///
    /// # Errors
    /// Returns an error if the config file exists but cannot be parsed.
    pub fn new(path: &Option<PathBuf>) -> Result<Self, Box<dyn Error>> {
        let builder = config::Config::builder();
        let builder = match path {
            Some(path) => builder
                .add_source(config::File::from(path.clone()).format(config::FileFormat::Toml)),
            None => {
                let path = Self::get_default_config_path()?;
                if !path.exists() {
                    log::info!("Config file not found. Using default configuration.");
                    return Ok(Self::default());
                }
                builder.add_source(config::File::from(path).format(config::FileFormat::Toml))
            }
        };

        let config = builder.build()?.try_deserialize::<Config>()?;
        Ok(config)
    }

    fn get_default_config_path() -> Result<PathBuf, Box<dyn Error>> {
        match dirs::config_dir() {
            Some(conf_dir) => Ok(conf_dir.join(format!("{APP_NAME}/config.toml"))),
            None => Err(Box::<dyn Error>::from(
                "Couldn't determine default config directory.",
            )),
        }
    }
}

const fn sorting_default() -> bool {
    false
}
