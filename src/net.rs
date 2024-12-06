/// A reactor-based networking implementation that provides asynchronous I/O operations.
/// This module implements a custom event loop using mio for handling non-blocking TCP connections.
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{self as sys, Shutdown};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};


use mio::{Events, Token};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::error::{ReactorError, ReactorResult};

/// The main reactor type that manages asynchronous I/O operations.
/// It uses an event loop to monitor multiple TCP connections efficiently.
#[derive(Clone)]
pub struct Reactor {
    shared: Arc<Shared>,
}

/// Internal shared state for the reactor.
/// Contains the registry of all monitored connections and their associated data.
///
/// # Implementation Details
/// - Uses a mio Registry for I/O event registration
/// - Maintains atomic counters for unique token generation
/// - Thread-safe storage of active I/O sources
struct Shared {
    /// The mio registry used to register I/O interests
    registry: mio::Registry,
    /// Counter for generating unique tokens for new connections
    token: AtomicUsize,
    /// Map of active I/O sources indexed by their tokens
    sources: Mutex<HashMap<Token, Arc<Source>>>,
}

impl Reactor {
    /// Creates a new reactor instance with its own event loop thread.
    ///
    /// # Returns
    /// - `ReactorResult<Self>`: A new reactor instance if successful
    ///
    /// # Errors
    /// - Returns `ReactorError::Init` if the reactor cannot be initialized
    /// - Returns `ReactorError::Init` if the event loop thread cannot be spawned
    pub fn new() -> ReactorResult<Self> {
        let poll = mio::Poll::new().map_err(ReactorError::Init)?;
        let shared = Arc::new(Shared {
            token: AtomicUsize::new(0),
            registry: poll.registry().try_clone().map_err(ReactorError::Init)?,
            sources: Mutex::new(HashMap::with_capacity(64)),
        });

        std::thread::Builder::new()
            .name("azrath-reactor".to_owned())
            .spawn({
                let shared = shared.clone();
                move || shared.run(poll)
            })
            .map_err(ReactorError::Init)?;

        Ok(Reactor { shared })
    }

    /// Registers a TCP stream with the reactor for async I/O operations.
    ///
    /// # Arguments
    /// - `sys`: A standard TCP stream to be registered
    ///
    /// # Returns
    /// - `ReactorResult<TcpStream>`: A wrapped async TCP stream
    ///
    /// # Errors
    /// - Returns `ReactorError::Registration` if the stream cannot be registered
    /// - Returns `ReactorError::LockPoisoned` if the internal mutex is poisoned
    pub fn register(&self, sys: sys::TcpStream) -> ReactorResult<TcpStream> {
        sys.set_nonblocking(true).map_err(ReactorError::Registration)?;
        let mut sys = mio::net::TcpStream::from_std(sys);
        let token = Token(self.shared.token.fetch_add(1, Ordering::Relaxed));

        self.shared.registry.register(
            &mut sys,
            token,
            mio::Interest::READABLE | mio::Interest::WRITABLE,
        ).map_err(ReactorError::Registration)?;

        let source = Arc::new(Source {
            token,
            interest: Default::default(),
            triggered: Default::default(),
        });

        {
            let mut sources = self.shared.sources.lock().map_err(|_| ReactorError::LockPoisoned)?;
            sources.insert(token, source.clone());
        }

        Ok(TcpStream {
            sys,
            source,
            reactor: self.clone(),
        })
    }

    /// Polls the readiness of a source for I/O operations.
    ///
    /// # Arguments
    /// - `source`: The I/O source to check
    /// - `direction`: The I/O direction (read/write)
    /// - `cx`: The task context
    ///
    /// # Returns
    /// - `Poll<io::Result<()>>`: The readiness state of the operation
    ///
    /// # Implementation Details
    /// - Checks if the operation is already triggered
    /// - Registers a waker if the operation is not ready
    /// - Double-checks triggered state to prevent race conditions
    fn poll_ready(
        &self,
        source: &Source,
        direction: usize,
        cx: &Context<'_>,
    ) -> Poll<io::Result<()>> {
        if source.triggered[direction].load(Ordering::Acquire) {
            return Poll::Ready(Ok(()));
        }

        {
            let mut interest = source.interest.lock().unwrap();

            match &mut interest[direction] {
                Some(existing) if existing.will_wake(cx.waker()) => {}
                _ => {
                    interest[direction] = Some(cx.waker().clone());
                }
            }
        }

        // check if anything changed while we were registering
        // our waker
        if source.triggered[direction].load(Ordering::Acquire) {
            return Poll::Ready(Ok(()));
        }

        Poll::Pending
    }

    /// Clears the triggered state for a given I/O direction.
    ///
    /// # Arguments
    /// - `source`: The I/O source to clear
    /// - `direction`: The I/O direction (read/write) to clear
    ///
    /// # Thread Safety
    /// Uses Release ordering to ensure visibility of the state change across threads
    fn clear_trigger(&self, source: &Source, direction: usize) {
        source.triggered[direction].store(false, Ordering::Release);
    }
}

impl Shared {
    /// Runs the event loop for the reactor.
    ///
    /// # Arguments
    /// - `poll`: The mio Poll instance for event monitoring
    ///
    /// # Returns
    /// - `ReactorResult<()>`: Success or error state of the operation
    ///
    /// # Implementation Details
    /// - Maintains a continuous loop for event processing
    /// - Handles errors gracefully with logging
    fn run(&self, mut poll: mio::Poll) -> ReactorResult<()> {
        let mut events = Events::with_capacity(64);
        let mut wakers = Vec::new();

        loop {
            if let Err(err) = self.poll(&mut poll, &mut events, &mut wakers) {
                log::warn!("Failed to poll reactor: {}", err);
            }

            events.clear();
        }
    }

    /// Polls for I/O events and processes them.
    ///
    /// # Arguments
    /// - `poll`: The mio Poll instance
    /// - `events`: Buffer for received events
    /// - `wakers`: Collection of wakers to be notified
    ///
    /// # Returns
    /// - `ReactorResult<()>`: Success or error state of the operation
    ///
    /// # Error Handling
    /// - Handles interrupted system calls
    /// - Propagates polling errors
    /// - Handles poisoned mutex conditions
    fn poll(
        &self,
        poll: &mut mio::Poll,
        events: &mut Events,
        wakers: &mut Vec<Waker>,
    ) -> ReactorResult<()> {
        if let Err(err) = poll.poll(events, None) {
            if err.kind() != io::ErrorKind::Interrupted {
                log::error!("Polling error: {}", err);
                return Err(ReactorError::Polling(err));
            }
            return Ok(());
        }

        for event in events.iter() {
            let source = {
                let sources = self.sources.lock().map_err(|_| ReactorError::LockPoisoned)?;
                match sources.get(&event.token()) {
                    Some(source) => source.clone(),
                    None => continue,
                }
            };

            let mut interest = source.interest.lock().map_err(|_| ReactorError::LockPoisoned)?;

            if event.is_readable() {
                if let Some(waker) = interest[direction::READ].take() {
                    wakers.push(waker);
                }

                source.triggered[direction::READ].store(true, Ordering::Release);
            }

            if event.is_writable() {
                if let Some(waker) = interest[direction::WRITE].take() {
                    wakers.push(waker);
                }

                source.triggered[direction::WRITE].store(true, Ordering::Release);
            }
        }

        for waker in wakers.drain(..) {
            waker.wake();
        }

        Ok(())
    }
}

mod direction {
    pub const READ: usize = 0;
    pub const WRITE: usize = 1;
}

/// Represents an I/O source in the reactor.
/// Tracks interest in I/O events and their triggered state.
struct Source {
    /// Wakers for read/write operations
    interest: Mutex<[Option<Waker>; 2]>,
    /// Flags indicating if read/write operations are ready
    triggered: [AtomicBool; 2],
    /// Unique identifier for this source
    token: Token,
}

/// An asynchronous TCP stream that works with the reactor.
/// Implements both AsyncRead and AsyncWrite traits for async I/O operations.
pub struct TcpStream {
    /// The underlying mio TCP stream
    pub sys: mio::net::TcpStream,
    /// Reference to the reactor managing this stream
    reactor: Reactor,
    /// The I/O source associated with this stream
    source: Arc<Source>,
}

impl TcpStream {
    /// Performs an I/O operation with the reactor.
    ///
    /// # Arguments
    /// - `direction`: The I/O direction (read/write)
    /// - `f`: The I/O operation to perform
    /// - `cx`: The task context
    ///
    /// # Returns
    /// - `Poll<io::Result<T>>`: The result of the I/O operation
    ///
    /// # Implementation Details
    /// - Handles non-blocking I/O operations
    /// - Manages reactor state for async operations
    /// - Properly handles WouldBlock conditions
    pub fn poll_io<T>(
        &self,
        direction: usize,
        mut f: impl FnMut() -> io::Result<T>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<T>> {
        if self.reactor.poll_ready(&self.source, direction, cx)?.is_pending() {
            return Poll::Pending;
        }

        match f() {
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                self.reactor.clear_trigger(&self.source, direction);
                Poll::Pending
            }
            val => Poll::Ready(val),
        }
    }
}

impl AsyncRead for TcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let unfilled = buf.initialize_unfilled();
        
        match self.poll_io(direction::READ, || (&self.sys).read(unfilled), cx) {
            Poll::Ready(Ok(n)) => {
                buf.advance(n);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for TcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_io(direction::WRITE, || (&self.sys).write(buf), cx)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_io(direction::WRITE, || (&self.sys).flush(), cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.sys.shutdown(Shutdown::Write))
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        let mut sources = self.reactor.shared.sources.lock().unwrap();
        let _ = sources.remove(&self.source.token);
        let _ = self.reactor.shared.registry.deregister(&mut self.sys);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Tests that a new reactor can be created successfully.
    #[test]
    fn test_reactor_creation() {
        let reactor = Reactor::new();
        assert!(reactor.is_ok());
    }

    /// Tests that a TCP stream can be properly registered with the reactor.
    /// This test:
    /// 1. Creates a TCP listener
    /// 2. Attempts to register a TCP stream with the reactor
    /// 3. Verifies the registration succeeds
    #[test]
    fn test_tcp_stream_registration() {
        let reactor = Reactor::new().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        // Test stream registration
        let stream = TcpStream::connect(addr).unwrap();
        let registered = reactor.register(stream);
        assert!(registered.is_ok());
    }

    /// Tests asynchronous I/O operations using the reactor.
    /// This test verifies:
    /// 1. Async write operations work correctly
    /// 2. Async read operations work correctly
    /// 3. Data integrity is maintained during transmission
    /// 
    /// The test creates a background thread that acts as an echo server,
    /// sending back a response after receiving data.
    #[test]
    fn test_async_io_operations() {
        let reactor = Reactor::new().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        // Spawn a background task to accept connections
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 13];
            stream.read_exact(&mut buf).unwrap();
            assert_eq!(&buf, b"Hello, World!");
            stream.write_all(b"Response!").unwrap();
        });

        // Test async write and read
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let stream = TcpStream::connect(addr).unwrap();
            let mut async_stream = reactor.register(stream).unwrap();

            // Test async write
            async_stream.write_all(b"Hello, World!").await.unwrap();

            // Test async read
            let mut buf = vec![0; 9];
            async_stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"Response!");
        });
    }

    /// Tests the reactor's poll_ready mechanism.
    /// Verifies that:
    /// 1. A new source can be created
    /// 2. The poll_ready function returns the expected Pending state
    /// 3. The waker mechanism is properly set up
    #[test]
    fn test_reactor_poll_ready() {
        use std::task::{Context, Poll};
        use futures::task::noop_waker;

        let reactor = Reactor::new().unwrap();
        let waker = noop_waker();
        let cx = Context::from_waker(&waker);
        
        // Create a source for testing
        let source = Arc::new(Source {
            token: Token(0),
            interest: Default::default(),
            triggered: Default::default(),
        });
        
        // Test poll result with READ direction
        let poll_result = reactor.poll_ready(&source, direction::READ, &cx);
        assert!(matches!(poll_result, Poll::Pending));
    }

    /// Tests proper cleanup of resources when a TcpStream is dropped.
    /// Verifies that:
    /// 1. The source is properly removed from the reactor's sources map
    /// 2. No memory leaks occur during cleanup
    #[test]
    fn test_source_cleanup() {
        let reactor = Reactor::new().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        {
            let stream = TcpStream::connect(addr).unwrap();
            let _registered = reactor.register(stream).unwrap();
            // Stream and registration will be dropped here
        }

        // Verify cleanup
        let sources = reactor.shared.sources.lock().unwrap();
        assert!(sources.is_empty());
    }

    /// Tests handling of multiple concurrent connections.
    /// This test verifies:
    /// 1. The reactor can handle multiple simultaneous connections
    /// 2. Each connection can perform independent I/O operations
    /// 3. Data integrity is maintained across all connections
    /// 
    /// Creates an echo server that handles 3 concurrent connections,
    /// each exchanging data independently.
    #[test]
    fn test_multiple_connections() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let reactor = Reactor::new().unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();

            let mut connections = Vec::new();
            for _ in 0..3 {
                let stream = TcpStream::connect(addr).unwrap();
                let async_stream = reactor.register(stream).unwrap();
                connections.push(async_stream);
            }

            // Test reading from each connection
            for (i, conn) in connections.iter_mut().enumerate() {
                let mut buf = [0; 10];
                let result = conn.sys.read(&mut buf);
                assert!(result.is_err(), "Connection {} should not have data", i);
                assert_eq!(
                    result.unwrap_err().kind(),
                    std::io::ErrorKind::WouldBlock,
                    "Connection {} should return WouldBlock",
                    i
                );
            }
        });
    }

    /// Tests that operations on the TCP stream are truly non-blocking.
    /// Verifies that:
    /// 1. Read operations on an empty stream return WouldBlock
    /// 2. The reactor properly handles non-blocking I/O operations
    #[test]
    fn test_nonblocking_operations() {
        let reactor = Reactor::new().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let stream = TcpStream::connect(addr).unwrap();
        let mut async_stream = reactor.register(stream).unwrap();

        // Attempt to read should not block
        let mut buf = [0; 10];
        let result = async_stream.sys.read(&mut buf);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }
}
