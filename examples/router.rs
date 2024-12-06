//! A basic HTTP router implementation using the Azrath web framework.
//! This module demonstrates routing, parameter handling, and shared state management.

use azrath::{Body, Request, Response, Server, Service};
use hyper::http;
use matchit::Router;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Represents a route handler function that takes a request and returns a response.
/// The handler must be both `Send` and `Sync` to be safely shared across threads.
type Handler = Box<dyn Fn(&Request) -> Response + Send + Sync>;

/// RouterService implements a basic HTTP router with support for path parameters
/// and shared state across requests.
///
/// The router uses the `matchit` crate for route matching and supports:
/// - Static routes
/// - Path parameters
/// - Shared state management using thread-safe containers
#[derive(Clone)]
struct RouterService {
    router: Arc<Router<Handler>>,
}

impl RouterService {
    /// Creates a new RouterService with predefined routes.
    ///
    /// # Returns
    /// A new instance of RouterService with the following routes configured:
    /// - GET "/" - Returns a welcome message
    /// - GET "/hello/:name" - Returns a personalized greeting
    /// - GET "/counter" - Returns and increments a shared counter
    ///
    /// # Example
    /// ```
    /// let router = RouterService::new();
    /// ```
    fn new() -> Self {
        let mut router = Router::new();
        let state = Arc::new(Mutex::new(HashMap::new()));
        let state_clone = state.clone();

        // Register routes
        router.insert("/", Box::new(|_req: &'_ Request| {
            Response::new(Body::new("Welcome to Azrath!"))
        }) as Handler).unwrap();

        router.insert("/hello/:name", Box::new(move |req: &Request| {
            let params = req.uri().path().split('/').collect::<Vec<_>>();
            let name = params.get(2).unwrap_or(&"world");
            Response::new(Body::new(format!("Hello, {}!", name)))
        }) as Handler).unwrap();

        // Example of a route that modifies shared state
        router.insert("/counter", Box::new(move |_req: &'_ Request| {
            let mut state = state_clone.lock().unwrap();
            let count = state.entry("counter".to_string())
                .and_modify(|c: &mut String| *c = (c.parse::<i32>().unwrap() + 1).to_string())
                .or_insert("1".to_string())
                .clone();
            Response::new(Body::new(format!("Counter: {}", count)))
        }) as Handler).unwrap();

        Self { 
            router: Arc::new(router)
        }
    }
}

impl Service for RouterService {
    /// Handles incoming HTTP requests by matching routes and executing
    /// the corresponding handler.
    ///
    /// # Arguments
    /// * `request` - The incoming HTTP request
    /// * `_info` - Connection information (currently unused)
    ///
    /// # Returns
    /// Returns a Response containing either:
    /// - The handler's response for matched routes
    /// - A 404 Not Found response for unmatched routes
    fn call(&self, request: Request, _info: azrath::ConnectionInfo) -> Response {
        // Match the route
        match self.router.at(request.uri().path()) {
            Ok(matched) => {
                // Call the handler function
                (matched.value)(&request)
            }
            Err(_) => {
                // Return 404 for unmatched routes
                http::Response::builder()
                    .status(404)
                    .body(Body::new("Not Found"))
                    .unwrap()
            }
        }
    }
}

/// Entry point for the HTTP server application.
///
/// Starts a server on localhost:3000 and configures it with the RouterService.
/// Prints available routes to stdout when the server starts.
#[tokio::main]
async fn main() {
    // Create and configure the server
    let server = Server::bind("127.0.0.1:3000");
    
    println!("Server running at http://127.0.0.1:3000");
    println!("Try these routes:");
    println!("  - /");
    println!("  - /hello/your-name");
    println!("  - /counter (increments on each visit)");

    // Start the server with our router service
    server.serve(RouterService::new())
        .await.expect("Server failed to start");
}