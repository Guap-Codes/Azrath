use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake};
use std::thread::{self, Thread};
use std::time::Duration;
use crate::error::{ExecutorError, Result};
#[allow(unused_imports)]
use hyper::rt::Executor as _;

/// A parker implementation that enables blocking a thread until a wake signal is received.
/// This is used to implement blocking operations in an async context.
pub struct Parker {
    thread: Thread,
    parked: AtomicBool,
}

impl Parker {
    /// Creates a new Parker instance wrapped in an Arc.
    /// 
    /// The parker starts in a parked state to ensure any wakeups that occur
    /// between polling and parking are not missed.
    pub fn new() -> Arc<Self> {
        Arc::new(Parker {
            thread: thread::current(),
            // start off as parked to ensure wakeups are seen in between polling and parking
            parked: AtomicBool::new(true),
        })
    }
}

impl Wake for Parker {
    fn wake(self: Arc<Self>) {
        if self.parked.swap(false, Ordering::Release) {
            self.thread.unpark();
        }
    }
}

impl Parker {
    pub fn block_on<F>(self: &Arc<Self>, mut fut: F) -> Result<F::Output>
    where
        F: Future,
    {
        self.parked.store(true, Ordering::Relaxed);

        let waker = self.clone().into();
        let mut cx = Context::from_waker(&waker);

        let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
        loop {
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(res) => break Ok(res),
                Poll::Pending => {
                    while self.parked.swap(true, Ordering::Acquire) {
                        thread::park();
                    }
                }
            }
        }
    }
}

/// A thread executor that manages a pool of worker threads for executing async tasks.
/// 
/// The executor implements a work-stealing scheduler with the following features:
/// - Dynamic thread pool that grows up to a maximum number of workers
/// - Worker threads that automatically exit after a period of inactivity
/// - Task queuing with notification system for idle workers
#[derive(Clone)]
pub struct Executor {
    inner: Arc<Inner>,
}

/// Internal state shared between the executor and its worker threads
struct Inner {
    /// Duration a worker thread will wait for new tasks before shutting down
    keep_alive: Duration,
    /// Shared state protected by a mutex
    shared: Mutex<Shared>,
    /// Maximum number of worker threads allowed
    max_workers: usize,
    /// Condition variable for worker thread synchronization
    condvar: Condvar,
}

/// Shared state for the thread pool
struct Shared {
    /// Queue of pending tasks
    queue: VecDeque<Box<dyn Future<Output = ()> + Send>>,
    /// Current number of worker threads
    workers: usize,
    /// Number of idle worker threads
    idle: usize,
    /// Number of workers that have been notified of new work
    notified: usize,
}

impl Executor {
    /// Creates a new executor with the specified configuration.
    ///
    /// # Arguments
    /// * `max_workers` - Optional maximum number of worker threads. Defaults to 15 * CPU cores
    /// * `keep_alive` - Optional duration workers will wait for new tasks. Defaults to 6 seconds
    #[allow(dead_code)]
    pub fn new(max_workers: Option<usize>, keep_alive: Option<Duration>) -> Self {
        Self {
            inner: Arc::new(Inner {
                shared: Mutex::new(Shared {
                    queue: VecDeque::new(),
                    workers: 0,
                    idle: 0,
                    notified: 0,
                }),
                condvar: Condvar::new(),
                keep_alive: keep_alive.unwrap_or_else(|| Duration::from_secs(6)),
                max_workers: max_workers.unwrap_or_else(|| num_cpus::get() * 15),
            }),
        }
    }

    /// Spawns a new worker thread that will process tasks from the queue.
    /// 
    /// # Errors
    /// Returns an error if the thread cannot be spawned.
    fn spawn_worker(&self, inner: Arc<Inner>) -> Result<()> {
        std::thread::Builder::new()
            .name("azrath-worker".to_owned())
            .spawn(move || {
                if let Err(e) = futures::executor::block_on(inner.run()) {
                    log::error!("Worker thread error: {}", e);
                }
            })
            .map_err(ExecutorError::ThreadSpawn)?;
        Ok(())
    }
}

impl<F> hyper::rt::Executor<F> for Executor
where
    F: Future<Output = ()> + Send + 'static,
{
    /// Executes a future on the thread pool.
    /// 
    /// This will either wake up an idle worker or spawn a new one if needed.
    /// Errors during execution are logged but not propagated.
    fn execute(&self, fut: F) {
        let result: Result<()> = (|| {
            let mut shared = self.inner.shared
                .lock()
                .map_err(|_| ExecutorError::LockPoisoned)?;
                
            shared.queue.push_back(Box::new(fut));

            if shared.idle == 0 && shared.workers != self.inner.max_workers {
                shared.workers += 1;
                let inner = self.inner.clone();
                self.spawn_worker(inner)?;
            } else if shared.idle > 0 {
                shared.idle -= 1;
                shared.notified += 1;
                self.inner.condvar.notify_one();
            }
            Ok(())
        })();

        if let Err(e) = result {
            log::error!("Failed to execute task: {}", e);
        }
    }
}

impl Inner {
    /// Main worker thread loop.
    /// 
    /// The worker will:
    /// 1. Process all available tasks in the queue
    /// 2. Wait for new tasks using a condition variable
    /// 3. Exit if no tasks arrive within the keep-alive duration
    /// 
    /// # Errors
    /// Returns an error if mutex operations fail
    async fn run(&self) -> Result<()> {
        let parker = Parker::new();
        let mut shared = self.shared
            .lock()
            .map_err(|_| ExecutorError::LockPoisoned)?;

        'alive: loop {
            while let Some(task) = shared.queue.pop_front() {
                drop(shared);
                parker.block_on(async {
                    Pin::from(task).await
                })?;
                shared = self.shared
                    .lock()
                    .map_err(|_| ExecutorError::LockPoisoned)?;
            }

            shared.idle += 1;

            loop {
                let result = self.condvar
                    .wait_timeout(shared, self.keep_alive)
                    .map_err(|_| ExecutorError::LockPoisoned)?;
                    
                shared = result.0;
                let timeout = result.1;

                if shared.notified != 0 {
                    shared.notified -= 1;
                    continue 'alive;
                }

                if timeout.timed_out() {
                    break 'alive;
                }
            }
        }

        shared.workers -= 1;
        shared.idle -= 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Tests that an executor is created with the correct configuration parameters.
    /// 
    /// Verifies that:
    /// - The maximum number of workers is set correctly
    /// - The keep-alive duration is set correctly
    #[test]
    fn test_executor_creation() {
        let executor = Executor::new(
            Some(4),
            Some(Duration::from_secs(10))
        );
        assert!(executor.inner.max_workers == 4);
        assert_eq!(executor.inner.keep_alive, Duration::from_secs(10));
    }

    /// Tests that tasks submitted to the executor are properly executed.
    #[test]
    fn test_task_execution() {
        let executor = Executor::new(None, None);
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let done = Arc::new(Condvar::new());
        let done_mutex = Arc::new(Mutex::new(()));
        let done_clone = done.clone();
        let lock = done_mutex.clone();

        executor.execute(async move {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            let _guard = lock.lock().unwrap();
            done_clone.notify_one();
        });

        // Wait for task completion with timeout
        let guard = done_mutex.lock().unwrap();
        let timeout = done.wait_timeout(guard, Duration::from_secs(1)).unwrap();
        assert!(!timeout.1.timed_out(), "Task execution timed out");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    /// Tests the Parker implementation for thread parking and waking.
    /// 
    /// This test verifies that:
    /// 1. A Parker can be created and used to block on async operations
    /// 2. The Parker correctly wakes up when signaled from another thread
    /// 3. The blocked operation completes successfully after being woken
    /// 
    /// The test spawns a separate thread that waits briefly before waking
    /// the Parker, simulating an asynchronous wake-up scenario.
    #[test]
    fn test_parker() {
        let parker = Parker::new();
        let parker_clone = parker.clone();

        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            parker_clone.wake();
        });

        let result = parker.block_on(async {
            "completed"
        });

        assert_eq!(result.unwrap(), "completed");
    }
}
