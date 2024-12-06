use serde::Deserialize;
use std::time::Duration;
use std::convert::TryFrom;

/// Configuration for the HTTP server.
/// 
/// This struct contains all the configuration options for both HTTP/1.x and HTTP/2
/// (when the "http2" feature is enabled). It can be initialized from environment
/// variables with the "AZRATH_" prefix or from a config file.
#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    /// Maximum number of worker threads. Defaults to CPU count * 15.
    #[serde(default = "default_max_workers")]
    pub max_workers: usize,
    /// Keep-alive duration in seconds for worker threads. Defaults to 6 seconds.
    #[serde(default = "default_keep_alive_secs")]
    pub worker_keep_alive_secs: u64,
    /// Server host address. Defaults to "127.0.0.1".
    #[serde(default = "default_host")]
    pub host: String,
    /// Server port number. Defaults to 3000.
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub http1_keep_alive: bool,
    #[serde(default)]
    pub http1_half_close: bool,
    /// Maximum buffer size for HTTP/1.x requests. Defaults to 512kb (524,288 bytes).
    #[serde(default = "default_max_buf_size")]
    pub http1_max_buf_size: usize,
    #[serde(default)]
    pub http1_pipeline_flush: bool,
    #[serde(default)]
    pub http1_writev: bool,
    #[serde(default)]
    pub http1_title_case_headers: bool,
    #[serde(default)]
    pub http1_preserve_header_case: bool,
    #[serde(default)]
    pub http1_only: bool,
    #[cfg(feature = "http2")]
    #[serde(default)]
    pub http2_only: bool,
    #[cfg(feature = "http2")]
    pub http2_initial_stream_window_size: Option<u32>,
    #[cfg(feature = "http2")]
    pub http2_initial_connection_window_size: Option<u32>,
    #[cfg(feature = "http2")]
    pub http2_adaptive_window: bool,
    #[cfg(feature = "http2")]
    pub http2_max_frame_size: Option<u32>,
    #[cfg(feature = "http2")]
    pub http2_max_concurrent_streams: Option<u32>,
    #[cfg(feature = "http2")]
    pub http2_max_send_buf_size: usize,
}

/// Implements conversion from the config crate's Config type to ServerConfig.
impl TryFrom<config::Config> for ServerConfig {
    type Error = config::ConfigError;

    fn try_from(config: config::Config) -> Result<Self, Self::Error> {
        config.try_deserialize()
    }
}

impl ServerConfig {
    /// Creates a new ServerConfig instance from environment variables and config file.
    /// 
    /// This method will:
    /// 1. Load environment variables from a .env file if present
    /// 2. Load configuration from a "config" file (if it exists)
    /// 3. Override with environment variables prefixed with "AZRATH_"
    ///
    /// # Errors
    /// Returns a ConfigError if configuration loading or parsing fails.
    pub fn new() -> Result<Self, config::ConfigError> {
        // Load .env file if it exists
        dotenv::dotenv().ok();

        let builder = config::Config::builder()
            // Add config file if it exists
            .add_source(config::File::with_name("config").required(false))
            // Change APP_ prefix to AZRATH_ to match the test
            .add_source(config::Environment::with_prefix("AZRATH"));

        // Build and convert into our ServerConfig type
        builder.build()?.try_into()
    }

    /// Converts the worker_keep_alive_secs value into a Duration.
    pub fn worker_keep_alive(&self) -> Duration {
        Duration::from_secs(self.worker_keep_alive_secs)
    }
}

/// Default value for max_workers. Returns CPU count * 15.
fn default_max_workers() -> usize {
    num_cpus::get() * 15
}

/// Default worker keep-alive duration in seconds.
fn default_keep_alive_secs() -> u64 {
    6
}

/// Default host address.
fn default_host() -> String {
    "127.0.0.1".to_string()
}

/// Default port number.
fn default_port() -> u16 {
    3000
}

/// Default maximum buffer size (512kb).
fn default_max_buf_size() -> usize {
    524_288  // Align with config.toml's 512kb
}

/// Provides default values for all configuration options.
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_workers: num_cpus::get() * 15,  // Default to CPU count * 15
            worker_keep_alive_secs: 10,
            host: String::from("0.0.0.0"),
            port: 8080,
            http1_keep_alive: true,
            http1_half_close: false,
            http1_max_buf_size: 524_288,  // 512kb
            http1_pipeline_flush: false,
            http1_writev: true,
            http1_title_case_headers: false,
            http1_preserve_header_case: false,
            http1_only: false,
            #[cfg(feature = "http2")]
            http2_only: false,
            #[cfg(feature = "http2")]
            http2_initial_stream_window_size: None,
            #[cfg(feature = "http2")]
            http2_initial_connection_window_size: None,
            #[cfg(feature = "http2")]
            http2_adaptive_window: true,
            #[cfg(feature = "http2")]
            http2_max_frame_size: None,
            #[cfg(feature = "http2")]
            http2_max_concurrent_streams: None,
            #[cfg(feature = "http2")]
            http2_max_send_buf_size: 524_288,  // 512kb
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    /// Tests that the default configuration values are set correctly.
    /// Verifies:
    /// - max_workers is CPU count * 15
    /// - port is 8080
    /// - host is "0.0.0.0"
    #[test]
    fn test_default_config() {
        let config = ServerConfig::default();
        assert_eq!(config.max_workers, num_cpus::get() * 15);
        assert_eq!(config.port, 8080);
        assert_eq!(config.host, "0.0.0.0");
    }

    /// Tests configuration loading from environment variables.
    /// Verifies that the ServerConfig correctly reads and applies
    /// values from environment variables with the "AZRATH_" prefix.
    #[test]
    fn test_config_from_env() {
        env::set_var("AZRATH_PORT", "9000");
        env::set_var("AZRATH_HOST", "127.0.0.1");
        env::set_var("AZRATH_MAX_WORKERS", "4");

        let config = ServerConfig::new().unwrap();
        assert_eq!(config.port, 9000);
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.max_workers, 4);

        // Cleanup
        env::remove_var("AZRATH_PORT");
        env::remove_var("AZRATH_HOST");
        env::remove_var("AZRATH_MAX_WORKERS");
    }

    /// Tests HTTP/2 specific default configuration values.
    /// Verifies:
    /// - max_send_buf_size is 524288 (512kb)
    /// - http2_only is false by default
    #[cfg(feature = "http2")]
    #[test]
    fn test_http2_config() {
        let config = ServerConfig::default();
        assert_eq!(config.http2_max_send_buf_size, 524288);
        assert!(!config.http2_only);
    }
}