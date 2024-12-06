use std::io;
use thiserror::Error;

/// Represents errors that can occur during task execution in the executor system.
#[derive(Debug, Error)]
pub enum ExecutorError {
    /// Error when a mutex lock becomes poisoned due to a panic in another thread
    #[error("mutex lock poisoned")]
    LockPoisoned,
    
    /// Error when spawning a new thread fails
    #[error("thread spawn failed: {0}")]
    ThreadSpawn(io::Error),
    
    /// Error during task execution with a descriptive message
    #[error("task execution failed: {0}")]
    TaskExecution(String),
    
    /// Wrapper for standard I/O errors
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

/// Represents errors that can occur in the reactor system responsible for I/O events.
#[derive(Debug, Error)]
pub enum ReactorError {
    /// Error during reactor initialization
    #[error("reactor initialization failed: {0}")]
    Init(io::Error),
    
    /// Error when registering resources with the reactor
    #[error("registration failed: {0}")]
    Registration(io::Error),
    
    /// Error during event polling operations
    #[error("polling failed: {0}")]
    Polling(io::Error),
    
    /// Error when a mutex lock becomes poisoned due to a panic in another thread
    #[error("mutex lock poisoned")]
    LockPoisoned,
    
    /// Wrapper for standard I/O errors
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

/// Type alias for Results using ExecutorError as the error type
pub type Result<T> = std::result::Result<T, ExecutorError>;

/// Type alias for Results using ReactorError as the error type
pub type ReactorResult<T> = std::result::Result<T, ReactorError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    /// Tests the conversion and formatting of ExecutorError with IO errors
    #[test]
    fn test_executor_error_conversion() {
        let io_err = io::Error::new(io::ErrorKind::Other, "test error");
        let exec_err = ExecutorError::Io(io_err);
        
        assert!(matches!(exec_err, ExecutorError::Io(_)));
        assert_eq!(exec_err.to_string(), "io error: test error");
    }

    /// Tests the automatic conversion from io::Error to ReactorError
    #[test]
    fn test_reactor_error_conversion() {
        let io_err = io::Error::new(io::ErrorKind::Other, "test error");
        let reactor_err = ReactorError::from(io_err);
        
        assert!(matches!(reactor_err, ReactorError::Io(_)));
    }
}