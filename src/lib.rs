#![allow(clippy::return_self_not_must_use)]
#![doc = include_str!("../README.md")]

//! # Azrath
//! 
//! A high-performance HTTP server built on hyper with support for both HTTP/1.x and HTTP/2.
//! 
//! ## Features
//! - HTTP/1.x and HTTP/2 support (via feature flag)
//! - Custom reactor-based async I/O
//! - Configurable thread pool
//! - Connection keep-alive
//! - Request routing
//! - Shared state management
//! 
//! ## Example
//! ```no_run
//! use azrath::{Body, Request, Response, Server};
//! 
//! #[tokio::main]
//! async fn main() -> std::io::Result<()> {
//!     Server::bind("127.0.0.1:3000")
//!         .serve(|_req: Request, _info| {
//!             Response::new(Body::new("Hello World!"))
//!         })
//!         .await?;
//!     
//!     Ok(())
//! }
//! ```
//! 
//! ## Architecture
//! The server is built on several key components:
//! 
//! - `Server`: The main entry point that handles configuration and startup
//! - `Reactor`: Custom event loop for async I/O operations
//! - `Executor`: Thread pool for handling requests
//! - `Service`: Trait for implementing request handlers
//! 
//! ## Configuration
//! Server settings can be configured via:
//! - Environment variables (prefixed with `AZRATH_`)
//! - Configuration file (`config.toml`)
//! - Builder pattern API
//! 
//! ## Features
//! - `http2`: Enables HTTP/2 support (enabled by default)

pub use crate::http::{Body, Request, Response, ResponseBuilder};
pub use crate::server::{ConnectionInfo, Server, Service};
pub use crate::error::{ExecutorError, ReactorError};
pub use crate::config::ServerConfig;

mod config;
mod error;
mod executor;
mod http;
mod net;
mod server;

// Re-export common types
pub use hyper;
