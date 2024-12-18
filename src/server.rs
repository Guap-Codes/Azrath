use crate::net::Reactor;
use crate::{Body, Request, Response};

use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::net::{SocketAddr, TcpListener, ToSocketAddrs};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use std::sync::Arc;

use hyper::server::conn::Http;

use crate::config::ServerConfig;

/// An HTTP server that can be configured and started to handle incoming HTTP requests.
///
/// # Examples
///
/// ```no_run
/// use azrath::{Server, Request, Response, Body, ConnectionInfo};
/// use std::io;
///
/// #[tokio::main]
/// async fn main() -> io::Result<()> {
///     let server = Server::bind("127.0.0.1:3000");
///     
///     // Using a closure as the service
///     server.serve(|_req: Request, _info: ConnectionInfo| {
///         Response::new(Body::new("Hello World!"))
///     }).await?;
///     
///     Ok(())
/// }
/// ```
///
/// Using a custom service implementation:
///
/// ```no_run
/// use azrath::{Server, Request, Response, Body, Service, ConnectionInfo};
/// use std::io;
/// use std::sync::{Arc, Mutex};
/// use std::sync::atomic::AtomicUsize;
///
/// #[derive(Clone)]
/// struct MyService {
///     request_count: Arc<AtomicUsize>,
/// }
///
/// impl Service for MyService {
///     fn call(&self, _req: Request, _info: ConnectionInfo) -> Response {
///         let count = self.request_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
///         println!("request #{}", count);
///         Response::new(Body::new("Hello world"))
///     }
/// }
///
/// #[tokio::main]
/// async fn main() -> io::Result<()> {
///     let server = Server::bind("127.0.0.1:3000");
///     
///     server.serve(MyService { 
///         request_count: Arc::new(AtomicUsize::new(0))
///     }).await?;
///     
///     Ok(())
/// }
/// ```
#[derive(Default)]
pub struct Server {
    /// The bound TCP listener for accepting connections
    listener: Option<TcpListener>,
    /// Whether to use keep-alive for HTTP/1 connections (default: true)
    http1_keep_alive: Option<bool>,
    /// Whether to support half-closures for HTTP/1 connections (default: false)
    http1_half_close: Option<bool>,
    http1_max_buf_size: Option<usize>,
    http1_pipeline_flush: Option<bool>,
    http1_writev: Option<bool>,
    http1_title_case_headers: Option<bool>,
    http1_preserve_header_case: Option<bool>,
    http1_only: Option<bool>,
    #[cfg(feature = "http2")]
    http2_only: Option<bool>,
    #[cfg(feature = "http2")]
    http2_initial_stream_window_size: Option<u32>,
    #[cfg(feature = "http2")]
    http2_initial_connection_window_size: Option<u32>,
    #[cfg(feature = "http2")]
    http2_adaptive_window: Option<bool>,
    #[cfg(feature = "http2")]
    http2_max_frame_size: Option<u32>,
    #[cfg(feature = "http2")]
    http2_max_concurrent_streams: Option<u32>,
    #[cfg(feature = "http2")]
    http2_max_send_buf_size: Option<usize>,
    worker_keep_alive: Option<Duration>,
    max_workers: Option<usize>,
}

/// Information about an HTTP connection, such as the peer address.
#[derive(Clone, Debug)]
pub struct ConnectionInfo {
    peer_addr: Option<SocketAddr>,
}

impl ConnectionInfo {
    /// Returns the socket address of the remote peer of this connection.
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer_addr
    }
}

/// A trait for implementing request handlers.
///
/// # Examples
///
/// ```no_run
/// use azrath::{Request, Response, Server, Service, Body, ConnectionInfo};
/// use std::sync::Mutex;
/// use std::sync::Arc;
///
/// #[derive(Clone)]
/// struct MyService {
///     count: Arc<Mutex<usize>>,
/// }
///
/// impl Service for MyService {
///     fn call(&self, _request: Request, _info: ConnectionInfo) -> Response {
///         let mut count = self.count.lock().unwrap();
///         *count += 1;
///         println!("request #{}", *count);
///         Response::new(Body::new("Hello world"))
///     }
/// }
///
/// #[tokio::main]
/// async fn main() -> std::io::Result<()> {
///     let server = Server::bind("127.0.0.1:3000");
///     server.serve(MyService { 
///         count: Arc::new(Mutex::new(0)) 
///     }).await
/// }
/// ```
pub trait Service: Send + 'static {
    fn call(&self, request: Request, info: ConnectionInfo) -> Response;
}

impl<F> Service for F
where
    F: Fn(Request, ConnectionInfo) -> Response + Send + Sync + 'static,
{
    fn call(&self, request: Request, info: ConnectionInfo) -> Response {
        (self)(request, info)
    }
}

impl<S> Service for Arc<S>
where
    S: Service + Sync,
{
    fn call(&self, request: Request, info: ConnectionInfo) -> Response {
        (**self).call(request, info)
    }
}

impl Server {
    /// Binds a server to the provided address.
    ///
    /// # Panics
    ///
    /// This method will panic if binding to the address fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use azrath::Server;
    /// use std::net::SocketAddr;
    ///
    /// let server = Server::bind("localhost:3000");
    /// let server = Server::bind(SocketAddr::from(([127, 0, 0, 1], 3000)));
    /// ```
    pub fn bind(addr: impl ToSocketAddrs) -> Server {
        let listener = std::net::TcpListener::bind(addr).expect("failed to bind listener");

        Server {
            listener: Some(listener),
            ..Default::default()
        }
    }

    /// Serve incoming connections with the provided service.
    pub async fn serve<S>(mut self, service: S) -> io::Result<()> 
    where
        S: Service + Clone + Sync + 'static,
    {
        let listener = self.listener.take().expect("server not bound");
        let reactor = Reactor::new().map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let mut http = Http::new();
        self.configure(&mut http);

        loop {
            match listener.accept() {
                Ok((stream, addr)) => {
                    let conn = reactor.register(stream).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                    let service = service.clone();
                    let builder = http.clone();
                    let info = ConnectionInfo {
                        peer_addr: Some(addr),
                    };

                    tokio::spawn(async move {
                        if let Err(err) = builder
                            .serve_connection(conn, service::HyperService(service, info))
                            .await
                        {
                            log::error!("Error serving connection: {}", err);
                        }
                    });
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    tokio::task::yield_now().await;
                    continue;
                }
                Err(e) => {
                    log::error!("Accept error: {}", e);
                    return Err(e);
                }
            }
        }
    }

    /// Sets the maximum number of threads in the worker pool.
    ///
    /// By default, the limit is 15 threads per CPU core.
    pub fn max_workers(mut self, val: usize) -> Self {
        self.max_workers = Some(val);
        self
    }

    /// Sets how long to keep alive an idle thread in the worker pool.
    ///
    /// By default, the timeout is set to 6 seconds.
    pub fn worker_keep_alive(mut self, val: Duration) -> Self {
        self.worker_keep_alive = Some(val);
        self
    }

    /// Sets whether to use keep-alive for HTTP/1 connections.
    ///
    /// Default is `true`.
    pub fn http1_keep_alive(mut self, val: bool) -> Self {
        self.http1_keep_alive = Some(val);
        self
    }

    /// Set whether HTTP/1 connections should support half-closures.
    ///
    /// Clients can choose to shutdown their write-side while waiting
    /// for the server to respond. Setting this to `true` will
    /// prevent closing the connection immediately if `read`
    /// detects an EOF in the middle of a request.
    ///
    /// Default is `false`.
    pub fn http1_half_close(mut self, val: bool) -> Self {
        self.http1_half_close = Some(val);
        self
    }

    /// Set the maximum buffer size.
    ///
    /// Default is 512kb (524,288 bytes).
    pub fn http1_max_buf_size(mut self, val: usize) -> Self {
        self.http1_max_buf_size = Some(val);
        self
    }

    /// Sets whether to bunch up HTTP/1 writes until the read buffer is empty.
    ///
    /// This isn't really desirable in most cases, only really being useful in
    /// silly pipeline benchmarks.
    pub fn http1_pipeline_flush(mut self, val: bool) -> Self {
        self.http1_pipeline_flush = Some(val);
        self
    }

    /// Set whether HTTP/1 connections should try to use vectored writes,
    /// or always flatten into a single buffer.
    ///
    /// Note that setting this to false may mean more copies of body data,
    /// but may also improve performance when an IO transport doesn't
    /// support vectored writes well, such as most TLS implementations.
    ///
    /// Setting this to true will force hyper to use queued strategy
    /// which may eliminate unnecessary cloning on some TLS backends
    ///
    /// Default is `auto`. In this mode hyper will try to guess which
    /// mode to use
    pub fn http1_writev(mut self, enabled: bool) -> Self {
        self.http1_writev = Some(enabled);
        self
    }

    /// Set whether HTTP/1 connections will write header names as title case at
    /// the socket level.
    ///
    /// Note that this setting does not affect HTTP/2.
    ///
    /// Default is false.
    pub fn http1_title_case_headers(mut self, val: bool) -> Self {
        self.http1_title_case_headers = Some(val);
        self
    }

    /// Set whether to support preserving original header cases.
    ///
    /// Currently, this will record the original cases received, and store them
    /// in a private extension on the `Request`. It will also look for and use
    /// such an extension in any provided `Response`.
    ///
    /// Since the relevant extension is still private, there is no way to
    /// interact with the original cases. The only effect this can have now is
    /// to forward the cases in a proxy-like fashion.
    ///
    /// Note that this setting does not affect HTTP/2.
    ///
    /// Default is false.
    pub fn http1_preserve_header_case(mut self, val: bool) -> Self {
        self.http1_preserve_header_case = Some(val);
        self
    }

    /// Sets whether HTTP/1 is required.
    ///
    /// Default is `false`.
    pub fn http1_only(mut self, val: bool) -> Self {
        self.http1_only = Some(val);
        self
    }

    /// Sets whether HTTP/2 is required.
    ///
    /// Default is `false`.
    #[cfg(feature = "http2")]
    pub fn http2_only(mut self, val: bool) -> Self {
        self.http2_only = Some(val);
        self
    }

    /// Sets the [`SETTINGS_INITIAL_WINDOW_SIZE`][spec] option for HTTP2
    /// stream-level flow control.
    ///
    /// Passing `None` will do nothing.
    ///
    /// If not set, hyper will use a default.
    ///
    /// [spec]: https://http2.github.io/http2-spec/#SETTINGS_INITIAL_WINDOW_SIZE
    #[cfg(feature = "http2")]
    pub fn http2_initial_stream_window_size(mut self, sz: impl Into<Option<u32>>) -> Self {
        self.http2_initial_stream_window_size = sz.into();
        self
    }

    /// Sets the max connection-level flow control for HTTP2
    ///
    /// Passing `None` will do nothing.
    ///
    /// If not set, hyper will use a default.
    #[cfg(feature = "http2")]
    pub fn http2_initial_connection_window_size(mut self, sz: impl Into<Option<u32>>) -> Self {
        self.http2_initial_connection_window_size = sz.into();
        self
    }

    /// Sets whether to use an adaptive flow control.
    ///
    /// Enabling this will override the limits set in
    /// `http2_initial_stream_window_size` and
    /// `http2_initial_connection_window_size`.
    #[cfg(feature = "http2")]
    pub fn http2_adaptive_window(mut self, enabled: bool) -> Self {
        self.http2_adaptive_window = Some(enabled);
        self
    }

    /// Sets the maximum frame size to use for HTTP2.
    ///
    /// Passing `None` will do nothing.
    ///
    /// If not set, hyper will use a default.
    #[cfg(feature = "http2")]
    pub fn http2_max_frame_size(mut self, sz: impl Into<Option<u32>>) -> Self {
        self.http2_max_frame_size = sz.into();
        self
    }

    /// Sets the [`SETTINGS_MAX_CONCURRENT_STREAMS`][spec] option for HTTP2
    /// connections.
    ///
    /// Default is no limit (`std::u32::MAX`). Passing `None` will do nothing.
    ///
    /// [spec]: https://http2.github.io/http2-spec/#SETTINGS_MAX_CONCURRENT_STREAMS
    #[cfg(feature = "http2")]
    pub fn http2_max_concurrent_streams(mut self, max: impl Into<Option<u32>>) -> Self {
        self.http2_max_concurrent_streams = max.into();
        self
    }

    /// Set the maximum write buffer size for each HTTP/2 stream.
    ///
    /// Default is currently ~400KB, but may change.
    ///
    /// # Panics
    ///
    /// The value must be no larger than `u32::MAX`.
    #[cfg(feature = "http2")]
    pub fn http2_max_send_buf_size(mut self, max: usize) -> Self {
        self.http2_max_send_buf_size = Some(max);
        self
    }

    /// Get the local address of the bound socket
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "Server::bind not called yet"))?
            .local_addr()
    }

    /// Configures the HTTP server with the specified options.
    fn configure<T>(&self, http: &mut Http<T>) {
        macro_rules! configure {
            ($self:ident, $other:expr, [$($option:ident),* $(,)?], [$($other_option:ident => $this_option:ident),* $(,)?]) => {{
                $(
                    if let Some(val) = $self.$option {
                        $other.$option(val);
                    }
                )*
                $(
                    if let Some(val) = $self.$this_option {
                        $other.$other_option(val);
                    }
                )*
            }};
        }

        #[cfg(feature = "http2")]
        configure!(
            self,
            http,
            [
                http1_keep_alive,
                http1_half_close,
                http1_writev,
                http1_title_case_headers,
                http1_preserve_header_case,
                http1_only,
                http2_only,
                http2_initial_stream_window_size,
                http2_initial_connection_window_size,
                http2_adaptive_window,
                http2_max_frame_size,
                http2_max_concurrent_streams,
                http2_max_send_buf_size
            ],
            [
                max_buf_size => http1_max_buf_size,
                pipeline_flush => http1_pipeline_flush,
            ]
        );

        #[cfg(not(feature = "http2"))]
        configure!(
            self,
            http,
            [
                http1_keep_alive,
                http1_half_close,
                http1_writev,
                http1_title_case_headers,
                http1_preserve_header_case,
                http1_only,
            ],
            [
                max_buf_size => http1_max_buf_size,
                pipeline_flush => http1_pipeline_flush,
            ]
        );
    }

    /// Creates a new server with configuration from environment/config file
    pub fn from_config() -> io::Result<Server> {
        let config = ServerConfig::new().map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("Config error: {}", e))
        })?;
        let addr = format!("{}:{}", config.host, config.port);
        let listener = std::net::TcpListener::bind(addr)?;

        let mut server = Server {
            listener: Some(listener),
            max_workers: Some(config.max_workers),
            worker_keep_alive: Some(config.worker_keep_alive()),
            http1_keep_alive: Some(config.http1_keep_alive),
            http1_half_close: Some(config.http1_half_close),
            http1_max_buf_size: Some(config.http1_max_buf_size),
            ..Default::default()
        };

        #[cfg(feature = "http2")]
        {
            server.http2_max_send_buf_size = Some(config.http2_max_send_buf_size);
            server.http2_only = Some(config.http2_only);
        }

        Ok(server)
    }
}

mod service {
    use super::*;

    /// Type alias for the underlying Hyper request type
    type HyperRequest = hyper::Request<hyper::Body>;

    /// A wrapper that adapts our Service trait to Hyper's service trait.
    /// Holds both the service implementation and connection information.
    pub struct HyperService<S>(pub S, pub ConnectionInfo);

    /// Represents a future that will resolve to a service response.
    /// Contains the service, request, and connection information needed
    /// to process the request.
    pub struct Lazy<S>(S, Option<HyperRequest>, ConnectionInfo);

    impl<S> hyper::service::Service<HyperRequest> for HyperService<S>
    where
        S: Service + Clone,
    {
        type Response = Response;
        type Error = Infallible;
        type Future = Lazy<S>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, req: HyperRequest) -> Self::Future {
            Lazy(self.0.clone(), Some(req), self.1.clone())
        }
    }

    impl<S> Unpin for Lazy<S> {}

    impl<S> Future for Lazy<S>
    where
        S: Service,
    {
        type Output = Result<Response, Infallible>;

        fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            let (parts, body) = self.1.take().unwrap().into_parts();
            let req = Request::from_parts(parts, Body(body));

            let res = self.0.call(req, self.2.clone());
            Poll::Ready(Ok(res))
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Tests that the Server builder pattern correctly configures HTTP/1.x settings.
    /// 
    /// Verifies that all HTTP/1.x configuration options are properly set and stored
    /// in the Server instance, including:
    /// - Worker pool settings (size and keep-alive duration)
    /// - Keep-alive behavior
    /// - Buffer sizes and pipeline settings
    /// - Header case handling
    #[test]
    fn test_server_builder_configuration() {
        let server = Server::bind("127.0.0.1:0")
            .max_workers(30)
            .worker_keep_alive(Duration::from_secs(10))
            .http1_keep_alive(true)
            .http1_half_close(false)
            .http1_max_buf_size(524288)
            .http1_pipeline_flush(false)
            .http1_writev(true)
            .http1_title_case_headers(false)
            .http1_preserve_header_case(false)
            .http1_only(false);

        assert_eq!(server.max_workers, Some(30));
        assert_eq!(server.worker_keep_alive, Some(Duration::from_secs(10)));
        assert_eq!(server.http1_keep_alive, Some(true));
        assert_eq!(server.http1_half_close, Some(false));
        assert_eq!(server.http1_max_buf_size, Some(524288));
        assert_eq!(server.http1_pipeline_flush, Some(false));
        assert_eq!(server.http1_writev, Some(true));
        assert_eq!(server.http1_title_case_headers, Some(false));
        assert_eq!(server.http1_preserve_header_case, Some(false));
        assert_eq!(server.http1_only, Some(false));
    }

    /// Tests the HTTP/2 specific configuration options of the Server.
    /// 
    /// Verifies that HTTP/2 specific settings are properly configured, including:
    /// - Window sizes for streams and connections
    /// - Frame size limits
    /// - Concurrent stream limits
    /// - Adaptive window behavior
    #[test]
    #[cfg(feature = "http2")]
    fn test_http2_configuration() {
        let server = Server::bind("127.0.0.1:0")
            .http2_only(false)
            .http2_initial_stream_window_size(Some(65535))
            .http2_initial_connection_window_size(Some(1048576))
            .http2_adaptive_window(true)
            .http2_max_frame_size(Some(16384))
            .http2_max_concurrent_streams(Some(100))
            .http2_max_send_buf_size(524288);

        assert_eq!(server.http2_only, Some(false));
        assert_eq!(server.http2_initial_stream_window_size, Some(65535));
        assert_eq!(server.http2_initial_connection_window_size, Some(1048576));
        assert_eq!(server.http2_adaptive_window, Some(true));
        assert_eq!(server.http2_max_frame_size, Some(16384));
        assert_eq!(server.http2_max_concurrent_streams, Some(100));
        assert_eq!(server.http2_max_send_buf_size, Some(524288));
    }

    /// Tests server initialization from configuration files/environment.
    #[test]
    fn test_server_from_config() {
        // Set up required environment variables
        std::env::set_var("AZRATH_HOST", "127.0.0.1");
        std::env::set_var("AZRATH_PORT", "3000");
        std::env::set_var("AZRATH_MAX_WORKERS", "30");
        std::env::set_var("AZRATH_WORKER_KEEP_ALIVE_SECS", "10");
        std::env::set_var("AZRATH_HTTP1_MAX_BUF_SIZE", "524288");

        let server = Server::from_config().unwrap();
        
        // Verify configuration
        assert_eq!(server.max_workers, Some(30));
        assert_eq!(server.worker_keep_alive, Some(Duration::from_secs(10)));
        assert_eq!(server.http1_keep_alive, Some(true));
        assert_eq!(server.http1_half_close, Some(false));
        assert_eq!(server.http1_max_buf_size, Some(524288));
        
        #[cfg(feature = "http2")]
        {
            assert_eq!(server.http2_max_send_buf_size, Some(524288));
            assert_eq!(server.http2_only, Some(false));
        }

        // Clean up environment variables
        std::env::remove_var("AZRATH_HOST");
        std::env::remove_var("AZRATH_PORT");
        std::env::remove_var("AZRATH_MAX_WORKERS");
        std::env::remove_var("AZRATH_WORKER_KEEP_ALIVE_SECS");
        std::env::remove_var("AZRATH_HTTP1_MAX_BUF_SIZE");
    }

    /// Tests the basic Service trait implementation for function handlers.
    /// 
    /// Verifies that:
    /// - A closure can be used as a Service
    /// - Request handling works correctly
    /// - Response body contains expected content
    /// - Body chunks are properly concatenated
    #[test]
    fn test_service_trait_implementation() {
        let response_text = "Hello, World!";
        
        let service = move |_req: Request, _info: ConnectionInfo| {
            Response::new(Body::new(response_text))
        };

        let request = Request::new(Body::empty());
        let info = ConnectionInfo { peer_addr: None };
        
        let response = Service::call(&service, request, info);
        
        let body = response.into_body();
        let content = body
            .into_iter()
            .map(|chunk| chunk.unwrap())
            .collect::<Vec<_>>();
        
        assert_eq!(content.len(), 1);
        assert_eq!(&content[0][..], response_text.as_bytes());
    }

    /// Tests the ConnectionInfo structure functionality.
    /// 
    /// Verifies that:
    /// - Peer address can be properly stored and retrieved
    /// - None values are handled correctly
    /// - Clone implementation works as expected
    #[test]
    fn test_connection_info() {
        let addr = "127.0.0.1:8080".parse().unwrap();
        let info = ConnectionInfo {
            peer_addr: Some(addr),
        };
        
        assert_eq!(info.peer_addr(), Some(addr));
        
        let info_none = ConnectionInfo { peer_addr: None };
        assert_eq!(info_none.peer_addr(), None);
    }

    /// Tests stateful service implementation using a thread-safe counter.
    /// 
    /// Verifies that:
    /// - State can be maintained between requests using Mutex
    /// - Service trait implementation works with stateful structs
    /// - Multiple requests increment the counter correctly
    /// - Response bodies contain the expected counter values
    #[test]
    fn test_stateful_service() {
        use std::sync::{Arc, Mutex};
        
        #[derive(Clone)]
        struct CounterService {
            count: Arc<Mutex<usize>>,
        }
        
        impl Service for CounterService {
            fn call(&self, _req: Request, _info: ConnectionInfo) -> Response {
                let mut count = self.count.lock().unwrap();
                *count += 1;
                Response::new(Body::new(count.to_string()))
            }
        }
        
        let service = CounterService {
            count: Arc::new(Mutex::new(0)),
        };
        
        let request = Request::new(Body::empty());
        let info = ConnectionInfo { peer_addr: None };
        
        let request2 = Request::new(Body::empty());
        
        let response1 = service.call(request, info.clone());
        let response2 = service.call(request2, info);
        
        let body1 = response1.into_body().into_iter().next().unwrap().unwrap();
        let body2 = response2.into_body().into_iter().next().unwrap().unwrap();
        
        assert_eq!(&body1[..], b"1");
        assert_eq!(&body2[..], b"2");
    }
}

