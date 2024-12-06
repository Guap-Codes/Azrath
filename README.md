# Azrath

A hybrid blocking/async HTTP server built on hyper with custom reactor-based I/O and comprehensive configuration options.

[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

## Features

- HTTP/1.x and HTTP/2 support (via feature flag)
- Custom reactor-based async I/O
- Configurable thread pool
- Connection keep-alive
- Request routing
- Shared state management
- Flexible configuration via environment variables and TOML files
- Non-blocking I/O operations with mio-based event loop
- Comprehensive error handling with custom error types
- Header case preservation options
- Configurable buffer sizes and pipeline settings
- Adaptive window sizing for HTTP/2
- Built-in support for half-close connections
- Graceful connection cleanup and resource management
- Tokio compatibility for async operations

## Quick Start

Add Azrath to your `Cargo.toml`:

```toml
[dependencies]
azrath = { git = "https://github.com/guap-codes/azrath" }
```

Create a basic server:

```rust,no_run
use azrath::{Body, Request, Response, Server};
#[tokio::main]
async fn main() {
Server::bind("127.0.0.1:3000")
.serve(|req: Request, _info| {
Response::new(Body::new("Hello World!"))
})
.await.expect("failed to start server");
}
```

## Router Example

Here's a more complex example using the router:

```rust,no_run
use azrath::{Body, Request, Response, Server, Service};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct RouterService {
    counter: Arc<Mutex<i32>>,
}

impl Service for RouterService {
    fn call(&self, request: Request, _info: azrath::ConnectionInfo) -> Response {
        match request.uri().path() {
            "/" => Response::new(Body::new("Welcome!")),
            "/counter" => {
                let mut count = self.counter.lock().unwrap();
                *count += 1;
                Response::new(Body::new(format!("Count: {}", count)))
            }
            _ => Response::new(Body::new("Not Found"))
        }
    }
}

#[tokio::main]
async fn main() {
    let service = RouterService {
        counter: Arc::new(Mutex::new(0)),
    };

    Server::bind("127.0.0.1:3000")
        .serve(service)
        .await
        .expect("server failed to start");
}
```

## Architecture

Azrath is built on several key components:

- `Server`: The main entry point that handles configuration and startup
- `Reactor`: Custom event loop for async I/O operations using mio
- `Executor`: Thread pool for handling requests with configurable worker management
- `Service`: Trait for implementing request handlers


## Configuration

Azrath can be configured through environment variables or a config file:

```toml
# config.toml
max_workers = 30
worker_keep_alive_secs = 10
host = "0.0.0.0"
port = 8080
http1_keep_alive = true
http1_max_buf_size = 524288  # 512kb
```

Or using environment variables:

```bash
AZRATH_MAX_WORKERS=30
AZRATH_PORT=8080
```

## Performance

- Custom reactor implementation for efficient I/O handling
- Configurable thread pool with smart worker management
- Keep-alive connection support
- Optimized buffer sizes for both HTTP/1.x and HTTP/2

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
