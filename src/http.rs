use crate::executor;

use core::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{cmp, debug_assert, io};

use futures::Stream;
use hyper::body::HttpBody;

pub use hyper::body::Bytes;

/// An HTTP request type that wraps hyper's Request with our custom Body type.
///
/// This type alias combines hyper's Request with our custom [`Body`] implementation,
/// providing a convenient way to handle HTTP requests in the azrath framework.
pub type Request = hyper::Request<Body>;

/// An HTTP response.
///
/// You can create a response with the [`new`](hyper::Response::new) method:
///
/// ```
/// # use azrath::{Response, Body};
/// let response = Response::new(Body::new("Hello world!"));
/// ```
///
/// Or with a [`ResponseBuilder`]:
///
/// ```
/// # use azrath::{ResponseBuilder, Body};
/// let response = ResponseBuilder::new()
///     .status(404)
///     .header("X-Custom-Foo", "Bar")
///     .body(Body::new("Page not found."))
///     .unwrap();
/// ```
///
/// See [`http::Response`](hyper::Response) and [`Body`] for details.
pub type Response = hyper::Response<Body>;

/// A builder for constructing HTTP responses with a fluent API.
///
/// This type alias provides access to hyper's ResponseBuilder, allowing for
/// easy construction of responses with headers, status codes, and body content.
///
/// # Examples
///
/// ```rust
/// use azrath::{ResponseBuilder, Body};
///
/// let response = ResponseBuilder::new()
///     .status(404)
///     .header("X-Custom-Foo", "Bar")
///     .body(Body::new("Page not found."))
///     .unwrap();
/// ```
pub type ResponseBuilder = hyper::http::response::Builder;

/// A streaming HTTP body that can be used for both requests and responses.
///
/// The `Body` type provides a wrapper around hyper's body implementation with
/// additional functionality for streaming data chunks and conversion to/from
/// various types.
///
/// # Features
/// - Streaming interface via Iterator implementation
/// - Conversion from strings and byte arrays
/// - Reader interface via [`BodyReader`]
/// - Integration with async streams
///
/// # Examples
///
/// Creating a body from a string:
/// ```rust
/// # use azrath::Body;
/// let body = Body::new("Hello world!");
/// ```
///
/// Reading a request body:
/// ```rust
/// # use azrath::{Request, Response, Body};
/// fn handle(mut req: Request) -> Response {
///     for chunk in req.body_mut() {
///         if let Ok(chunk) = chunk {
///             println!("Received chunk: {:?}", chunk);
///         }
///     }
///     Response::new(Body::empty())
/// }
/// ```
pub struct Body(pub(crate) hyper::Body);

impl Body {
    /// Create a body from a string or bytes.
    ///
    /// ```rust
    /// # use azrath::Body;
    /// let string = Body::new("Hello world!");
    /// let bytes = Body::new(vec![0, 1, 0, 1, 0]);
    /// ```
    pub fn new(data: impl Into<Bytes>) -> Body {
        Body(hyper::Body::from(data.into()))
    }

    /// Create an empty body.
    pub fn empty() -> Body {
        Body(hyper::Body::empty())
    }

    /// Create a body from an implementor of [`io::Read`].
    ///
    /// ```rust
    /// use azrath::{Request, Response, ResponseBuilder, Body};
    /// use std::fs::File;
    ///
    /// fn handle(_request: Request) -> Response {
    ///     let file = File::open("index.html").unwrap();
    ///
    ///     ResponseBuilder::new()
    ///         .header("Content-Type", "text/html")
    ///         .body(Body::wrap_reader(file))
    ///         .unwrap()
    /// }
    /// ```
    pub fn wrap_reader<R>(reader: R) -> Body
    where
        R: io::Read + Send + 'static,
    {
        Body(hyper::Body::wrap_stream(ReaderStream::new(reader)))
    }

    /// Creates a [`BodyReader`] that implements [`std::io::Read`].
    ///
    /// This method allows you to read the body's contents using a standard
    /// synchronous read interface, which is useful when working with APIs
    /// that expect [`std::io::Read`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use azrath::Body;
    /// # use std::io::Read;
    /// let mut body = Body::new("Hello world!");
    /// let mut reader = body.reader();
    /// let mut buffer = String::new();
    /// reader.read_to_string(&mut buffer).unwrap();
    /// ```
    pub fn reader(&mut self) -> BodyReader<'_> {
        BodyReader {
            body: self,
            prev_bytes: Bytes::new(),
        }
    }
}

impl<T> From<T> for Body
where
    Bytes: From<T>,
{
    fn from(data: T) -> Body {
        Body::new(data)
    }
}

impl Iterator for Body {
    type Item = io::Result<Bytes>;

    fn next(&mut self) -> Option<Self::Item> {
        match executor::Parker::new().block_on(self.0.data()) {
            Ok(maybe_result) => maybe_result.map(|res| {
                res.map_err(|err| io::Error::new(io::ErrorKind::Other, err))
            }),
            Err(e) => {
                log::error!("Error polling body data: {}", e);
                None
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        Stream::size_hint(&self.0)
    }
}

/// A synchronous reader interface for [`Body`].
///
/// This type implements [`std::io::Read`] for [`Body`], allowing the body's
/// contents to be read synchronously. It maintains an internal buffer of
/// previously read bytes to ensure efficient reading of chunks.
///
/// # Examples
///
/// ```rust
/// # use azrath::Body;
/// # use std::io::Read;
/// let mut body = Body::new("Hello world!");
/// let mut reader = body.reader();
/// let mut buffer = vec![0; 1024];
/// let bytes_read = reader.read(&mut buffer).unwrap();
/// ```
pub struct BodyReader<'b> {
    body: &'b mut Body,
    prev_bytes: Bytes,
}

impl<'b> std::io::Read for BodyReader<'b> {
    fn read(&mut self, mut buf: &mut [u8]) -> io::Result<usize> {
        let mut written = 0;
        loop {
            if buf.is_empty() {
                return Ok(written);
            }

            if !self.prev_bytes.is_empty() {
                let chunk_size = cmp::min(buf.len(), self.prev_bytes.len());
                let prev_bytes_start = self.prev_bytes.split_to(chunk_size);
                buf[..chunk_size].copy_from_slice(&prev_bytes_start[..]);
                buf = &mut buf[chunk_size..];
                written += chunk_size;
                continue;
            }

            if written != 0 {
                return Ok(written);
            }

            debug_assert!(self.prev_bytes.is_empty());
            debug_assert!(written == 0);

            self.prev_bytes = if let Some(next) = self.body.next() {
                next?
            } else {
                return Ok(written);
            }
        }
    }
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Default for Body {
    fn default() -> Self {
        Self::empty()
    }
}

impl HttpBody for Body {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_data(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Data, Self::Error>>> {
        Pin::new(&mut self.0).poll_data(cx)
    }

    fn poll_trailers(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<hyper::HeaderMap>, Self::Error>> {
        Pin::new(&mut self.0).poll_trailers(cx)
    }
}

/// A stream adapter that converts a [`std::io::Read`] into a [`Stream`].
///
/// This type is used internally by [`Body::wrap_reader`] to convert synchronous
/// readers into an async stream that can be used with hyper's body implementation.
///
/// The stream reads data in chunks of [`CAP`] bytes and yields them as
/// [`Bytes`] instances.
struct ReaderStream<R> {
    reader: Option<R>,
    buf: Vec<u8>,
}

/// Default capacity for reader buffer chunks.
const CAP: usize = 4096;

impl<R> ReaderStream<R> {
    fn new(reader: R) -> Self {
        Self {
            reader: Some(reader),
            buf: vec![0; CAP],
        }
    }
}

impl<R> Unpin for ReaderStream<R> {}

impl<R> Stream for ReaderStream<R>
where
    R: io::Read,
{
    type Item = io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let ReaderStream { reader, buf } = &mut *self;

        let reader = match reader {
            Some(reader) => reader,
            None => return Poll::Ready(None),
        };

        if buf.capacity() == 0 {
            buf.extend_from_slice(&[0; CAP]);
        }

        match reader.read(buf) {
            Err(err) => {
                self.reader.take();
                Poll::Ready(Some(Err(err)))
            }
            Ok(0) => {
                self.reader.take();
                Poll::Ready(None)
            }
            Ok(n) => {
                let remaining = buf.split_off(n);
                let chunk = std::mem::replace(buf, remaining);
                Poll::Ready(Some(Ok(Bytes::from(chunk))))
            }
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    /// Tests the creation of a `Body` from a string and verifies its content.
    #[test]
    fn test_body_creation() {
        let body = Body::new("Hello World!");
        let content: Vec<_> = body.into_iter()
            .map(|chunk| chunk.unwrap())
            .collect();
        assert_eq!(&content[0][..], b"Hello World!");
    }

    /// Tests the creation of an empty `Body` and ensures it has no content.
    #[test]
    fn test_empty_body() {
        let body = Body::empty();
        assert!(body.into_iter().next().is_none());
    }

    /// Tests the `ResponseBuilder` by creating a response and verifying its status and headers.
    #[test]
    fn test_response_builder() {
        let response = hyper::Response::builder()
            .status(200)
            .header("Content-Type", "text/plain")
            .body(Body::new("OK"))
            .unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers().get("Content-Type").unwrap(),
            "text/plain"
        );
    }
}
