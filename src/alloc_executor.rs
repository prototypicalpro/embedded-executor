use core::{
    future::Future,
    marker::PhantomData,
    mem,
    pin::Pin,
    task::{
        LocalWaker,
        Poll,
    },
};

use alloc::{
    collections::VecDeque,
    sync::Arc,
    task::{
        local_waker,
        Wake,
    },
};

use futures::{
    future::{
        FutureObj,
        LocalFutureObj,
        UnsafeFutureObj,
    },
    task::{
        Spawn,
        SpawnError,
    },
};

use lock_api::{
    Mutex,
    RawMutex,
};

use generational_arena::{
    Arena,
    Index,
};

use crate::{
    future_box,
    sleep::*,
};

// default initial registry capacity
const REG_CAP: usize = 16;

// default initial queue capacity
const QUEUE_CAP: usize = REG_CAP / 2;

// TODO: Investigate lock-free queues rather than using Mutexes. Might not
// really get us much for embedded devices where disabling interrupts is just an
// instruction away, but could be beneficial if threads get involved.

/// Alloc-only `Future` executor
///
/// Assuming the `RawMutex` implementation provided is sound, this *should* be
/// safe to use in both embedded and non-embedded scenarios. On embedded devices,
/// it will probably be a type that disables/re-enables interrupts. On real OS's,
/// it can be an actual mutex implementation.
///
/// The `Sleep` implementation can be used to put the event loop into a low-power
/// state using something like `cortex_m::wfi/e`.
pub struct AllocExecutor<'a, R, S>
where
    R: RawMutex + Send + Sync,
    S: Sleep,
{
    registry: Arena<Task<'a>>,
    queue: QueueHandle<'a, R>,
    sleeper: S,
}

/// See [`AllocExecutor::spawn_local`]
enum SpawnLoc {
    Front,
    Back,
}

impl<'a, R, S> AllocExecutor<'a, R, S>
where
    R: RawMutex + Send + Sync + 'static,
    S: Sleep,
{
    /// Initialize a new `AllocExecutor`
    ///
    /// Does nothing unless it's `run()`
    pub fn new() -> Self {
        Self::with_capacity(REG_CAP, QUEUE_CAP)
    }

    /// Initialize a new `AllocExecutor` with the given capacities.
    ///
    /// Does nothing unless it's `run()`
    pub fn with_capacity(registry: usize, queue: usize) -> Self {
        AllocExecutor {
            registry: Arena::with_capacity(registry),
            queue: new_queue(queue),
            sleeper: S::default(),
        }
    }

    /// Get a handle to a `Spawner` that can be passed to `Future` constructors
    /// to spawn even *more* `Future`s
    pub fn spawner(&self) -> Spawner<'a, R> {
        Spawner::new(self.queue.clone())
    }

    /// Get a handle to a `LocalSpawner` that can be passed to local `Future` constructors
    /// to spawn even *more* local `Future`s
    pub fn local_spawner(&self) -> LocalSpawner<'a, R> {
        LocalSpawner::new(Spawner::new(self.queue.clone()))
    }

    /// "Real" spawn method
    ///
    /// Differentiates between spawning at the back of the queue and spawning at
    /// the front of the queue. When `spawn` is called directly on the executor,
    /// one would expect the futures to be polled in the order they were spawned,
    /// so they should go to the back of the queue. When tasks are spawned via
    /// the spawn/poll queue, they've already waited in line and get an express
    /// ticket to the front.
    fn spawn_local(&mut self, future: LocalFutureObj<'a, ()>, loc: SpawnLoc) {
        let id = self.registry.insert(Task::new(future));

        let queue_waker = Arc::new(QueueWaker::new(self.queue.clone(), id, self.sleeper.clone()));

        let local_waker = queue_waker.into_local_waker();
        self.registry.get_mut(id).unwrap().set_waker(local_waker);

        let item = QueueItem::Poll(id);
        let mut lock = self.queue.lock();

        match loc {
            SpawnLoc::Front => lock.push_front(item),
            SpawnLoc::Back => lock.push_back(item),
        }
    }

    /// Spawn a local `UnsafeFutureObj` into the executor.
    pub fn spawn_raw<F>(&mut self, future: F)
    where
        F: UnsafeFutureObj<'a, ()>,
    {
        self.spawn_local(LocalFutureObj::new(future), SpawnLoc::Back)
    }

    /// Spawn a `Future` into the executor.
    ///
    /// This will implicitly box the future in order to objectify it.
    pub fn spawn<F>(&mut self, future: F)
    where
        F: Future<Output = ()> + 'a,
    {
        self.spawn_raw(future_box::make_local(future));
    }

    /// Polls a task with the given id
    ///
    /// If no such task exists, it's a no-op.
    /// If the task returns `Poll::Ready`, it will be removed from the registry.
    fn poll_task(&mut self, id: Index) {
        // It's possible that the waker is still hanging out somewhere and
        // getting called even though its task is gone. If so, we can just
        // skip it.
        if let Some(Task { future, waker }) = self.registry.get_mut(id) {
            let future = Pin::new(future);

            let waker = waker
                .as_ref()
                .expect("waker not set, task spawned incorrectly");

            match future.poll(waker) {
                Poll::Ready(_) => {
                    self.registry.remove(id);
                }
                Poll::Pending => {}
            }
        }
    }

    /// Get one task id or new future to be spawned from the queue.
    fn dequeue(&self) -> Option<QueueItem<'a>> {
        self.queue.lock().pop_front()
    }

    /// Run the executor
    ///
    /// Each loop will poll at most one task from the queue and then check for
    /// newly spawned tasks. If there are no new tasks spawned and nothing left
    /// in the queue, the executor will attempt to sleep.
    ///
    /// Once there's nothing to spawn and nothing left in the registry, the
    /// executor will return.
    pub fn run(&mut self) {
        loop {
            while let Some(item) = self.dequeue() {
                match item {
                    QueueItem::Poll(id) => {
                        self.poll_task(id);
                    }
                    QueueItem::Spawn(task) => {
                        self.spawn_local(task.into(), SpawnLoc::Front);
                    }
                }
            }
            if self.registry.is_empty() {
                break;
            }
            self.sleeper.sleep();
        }
    }
}

struct Task<'a> {
    future: LocalFutureObj<'a, ()>,
    // Invariant: waker should always be Some after the task has been spawned.
    waker: Option<LocalWaker>,
}

impl<'a> Task<'a> {
    fn new(future: LocalFutureObj<'a, ()>) -> Task<'a> {
        Task {
            future,
            waker: None,
        }
    }
    fn set_waker(&mut self, waker: LocalWaker) {
        self.waker = Some(waker);
    }
}

type Queue<'a> = VecDeque<QueueItem<'a>>;

type QueueHandle<'a, R> = Arc<Mutex<R, Queue<'a>>>;

fn new_queue<'a, R>(capacity: usize) -> QueueHandle<'a, R>
where
    R: RawMutex + Send + Sync,
{
    Arc::new(Mutex::new(Queue::with_capacity(capacity)))
}

enum QueueItem<'a> {
    Poll(Index),
    Spawn(FutureObj<'a, ()>),
}

// Super simple Wake implementation
// Sticks the Index into the queue and calls Alarm::ring
struct QueueWaker<R, S>
where
    R: RawMutex + Send + Sync,
{
    queue: QueueHandle<'static, R>,
    id: Index,
    sleeper: S,
}

impl<R, S> QueueWaker<R, S>
where
    R: RawMutex + Send + Sync + 'static,
    S: Sleep,
{
    fn new<'a>(queue: QueueHandle<'a, R>, id: Index, sleeper: S) -> Self {
        QueueWaker {
            // Safety: The QueueWaker only deals in 'static lifetimed things, i.e.
            // task `Index`es, only writes to the queue, and will never give anyone
            // else this transmuted version.
            queue: unsafe { mem::transmute(queue) },
            id,
            sleeper,
        }
    }

    fn into_local_waker(self: Arc<Self>) -> LocalWaker {
        // Safety: Our QueueWaker does the exact same thing for local vs
        // non-local wake, so this is fine.
        unsafe { local_waker(self) }
    }
}

impl<R, S> Wake for QueueWaker<R, S>
where
    R: RawMutex + Send + Sync,
    S: Sleep,
{
    fn wake(arc_self: &Arc<Self>) {
        arc_self
            .queue
            .lock()
            .push_back(QueueItem::Poll(arc_self.id));
        arc_self.sleeper.wake();
    }
}

/// Local spawner for an `AllocExecutor`
///
/// This can be used to spawn futures from the same thread as the executor.
///
/// Use a `Spawner` to spawn futures from *other* threads.
#[derive(Clone)]
pub struct LocalSpawner<'a, R>(Spawner<'a, R>, PhantomData<LocalFutureObj<'a, ()>>)
where
    R: RawMutex + Send + Sync;

impl<'a, R> LocalSpawner<'a, R>
where
    R: RawMutex + Send + Sync,
{
    fn new(spawner: Spawner<'a, R>) -> Self {
        LocalSpawner(spawner, PhantomData)
    }
}

impl<'a, R> LocalSpawner<'a, R>
where
    R: RawMutex + Send + Sync,
{
    fn spawn_local(&mut self, future: LocalFutureObj<'a, ()>) {
        // Safety: LocalSpawner is !Send and !Sync, so the future spawned will
        // always remain local to the executor.
        self.0.spawn_obj(unsafe { future.into_future_obj() })
    }

    /// Spawn a `FutureObj` into the corresponding `AllocExecutor`
    pub fn spawn_raw<F>(&mut self, future: F)
    where
        F: UnsafeFutureObj<'a, ()>,
    {
        self.spawn_local(LocalFutureObj::new(future));
    }

    /// Spawn a `Future` into the corresponding `AllocExecutor`
    ///
    /// While the lifetime on the Future is `'a`, unless you're calling this on a
    /// non-`'static` `Future` before the executor has started, you're most
    /// likely going to be stuck with the `Spawn` trait's `'static` bound.
    pub fn spawn<F>(&mut self, future: F)
    where
        F: Future<Output = ()> + 'a,
    {
        self.spawn_raw(future_box::make_local(future));
    }
}

/// Spawner for an `AllocExecutor`
///
/// This can be cloned and passed to an async function to allow it to spawn more
/// tasks.
pub struct Spawner<'a, R>(QueueHandle<'a, R>)
where
    R: RawMutex + Send + Sync;

impl<'a, R> Spawner<'a, R>
where
    R: RawMutex + Send + Sync,
{
    fn new(handle: QueueHandle<'a, R>) -> Self {
        Spawner(handle)
    }

    fn spawn_obj(&mut self, future: FutureObj<'a, ()>) {
        self.0.lock().push_back(QueueItem::Spawn(future));
    }

    /// Spawn a `FutureObj` into the corresponding `AllocExecutor`
    pub fn spawn_raw<F>(&mut self, future: F)
    where
        F: UnsafeFutureObj<'a, ()> + Send,
    {
        self.spawn_obj(FutureObj::new(future));
    }

    /// Spawn a `Future` into the corresponding `AllocExecutor`
    ///
    /// While the lifetime on the Future is `'a`, unless you're calling this on a
    /// non-`'static` `Future` before the executor has started, you're most
    /// likely going to be stuck with the `Spawn` trait's `'static` bound.
    pub fn spawn<F>(&mut self, future: F)
    where
        F: Future<Output = ()> + Send + 'a,
    {
        self.spawn_raw(future_box::make_obj(future));
    }
}

impl<'a, R> Clone for Spawner<'a, R>
where
    R: RawMutex + Send + Sync,
{
    fn clone(&self) -> Self {
        Spawner(self.0.clone())
    }
}

impl<'a, R> Spawn for Spawner<'a, R>
where
    R: RawMutex + Send + Sync,
{
    fn spawn_obj(&mut self, future: FutureObj<'static, ()>) -> Result<(), SpawnError> {
        self.spawn_obj(future);
        Ok(())
    }
}

impl<'a, R> From<LocalSpawner<'a, R>> for Spawner<'a, R>
where
    R: RawMutex + Send + Sync,
{
    fn from(other: LocalSpawner<'a, R>) -> Self {
        other.0
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::sleep::Sleep;
    use core::sync::atomic::{
        AtomicBool,
        Ordering,
        ATOMIC_BOOL_INIT,
    };
    use futures::{
        future::{
            self,
            FutureExt,
            FutureObj,
        },
        task::Spawn,
    };
    use lock_api::GuardSend;

    // shamelessly borrowed from the lock_api docs
    // 1. Define our raw lock type
    pub struct RawSpinlock(AtomicBool);

    // 2. Implement RawMutex for this type
    unsafe impl RawMutex for RawSpinlock {
        const INIT: RawSpinlock = RawSpinlock(ATOMIC_BOOL_INIT);

        // A spinlock guard can be sent to another thread and unlocked there
        type GuardMarker = GuardSend;

        fn lock(&self) {
            // Note: This isn't the best way of implementing a spinlock, but it
            // suffices for the sake of this example.
            while !self.try_lock() {}
        }

        fn try_lock(&self) -> bool {
            self.0.swap(true, Ordering::Acquire)
        }

        fn unlock(&self) {
            self.0.store(false, Ordering::Release);
        }
    }

    #[derive(Copy, Clone, Default)]
    struct NopSleep;

    impl Sleep for NopSleep {
        fn sleep(&self) {}
        fn wake(&self) {}
    }

    fn foo() -> impl Future<Output = i32> {
        future::ready(5)
    }

    fn bar() -> impl Future<Output = i32> {
        foo().then(|a| {
            println!("{}", a);
            let b = a + 1;
            future::ready(b)
        })
    }

    fn baz<S: Spawn>(mut spawner: S) -> impl Future<Output = ()> {
        bar().then(move |c| {
            for i in c..25 {
                let spam = future::lazy(move |_| println!("{}", i));
                spawner
                    .spawn_obj(FutureObj::new(future_box::make_obj(spam)))
                    .unwrap();
            }
            future::ready(())
        })
    }

    #[test]
    fn executor() {
        let mut executor = AllocExecutor::<RawSpinlock, NopSleep>::new();
        let mut spawner = executor.spawner();
        let entry = future::lazy(move |_| {
            for i in 0..10 {
                spawner.spawn_raw(future_box::make_obj(future::lazy(move |_| {
                    println!("{}", i);
                })));
            }
        });
        executor.spawn(entry);
        executor.spawn(baz(executor.spawner()));
        executor.run();
    }
}
