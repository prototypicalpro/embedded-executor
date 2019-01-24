use core::{
    mem,
    pin::Pin,
    task::Poll,
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
    future::FutureObj,
    task::{
        LocalWaker,
        Spawn,
        SpawnError,
    },
};

use generational_arena::{
    Arena,
    Index,
};

use lock_api::{
    Mutex,
    RawMutex,
};

use crate::{
    future_box::*,
    prelude::*,
    sleep::*,
};

// default registry capacity
const REG_CAP: usize = 16;

// default queue capacity
const QUEUE_CAP: usize = 16;

enum QueueItem<'a> {
    Poll(Index),
    Spawn(FutureObj<'a, ()>),
}

type Queue<'a> = VecDeque<QueueItem<'a>>;

type QueueHandle<'a, R> = Arc<Mutex<R, Queue<'a>>>;

fn new_queue<'a, R>(capacity: usize) -> QueueHandle<'a, R>
where
    R: RawMutex + Send + Sync,
{
    Arc::new(Mutex::new(Queue::with_capacity(capacity)))
}

// Super simple Wake implementation
// Sticks the Index into the queue and calls Alarm::ring
struct QueueWaker<R, A>
where
    R: RawMutex + Send + Sync,
{
    queue: QueueHandle<'static, R>,
    id: Index,
    alarm: A,
}

impl<R, A> QueueWaker<R, A>
where
    R: RawMutex + Send + Sync,
{
    fn new(queue: QueueHandle<'static, R>, id: Index, alarm: A) -> Self {
        QueueWaker { queue, id, alarm }
    }
}

impl<R, A> Wake for QueueWaker<R, A>
where
    R: RawMutex + Send + Sync,
    A: Alarm,
{
    fn wake(arc_self: &Arc<Self>) {
        arc_self
            .queue
            .lock()
            .push_back(QueueItem::Poll(arc_self.id));
        arc_self.alarm.ring();
    }
}

struct Task<'a> {
    future: FutureObj<'a, ()>,
    waker: Option<LocalWaker>,
}

impl<'a> Task<'a> {
    fn new(future: FutureObj<'a, ()>) -> Task<'a> {
        Task {
            future,
            waker: None,
        }
    }
    fn set_waker(&mut self, waker: LocalWaker) {
        self.waker = Some(waker);
    }
}

/// Alloc-only `Future` executor
///
/// Assuming the `RawMutex` implementation provided is sound, this *should* be
/// safe to use in both embedded and non-embedded scenarios. On embedded devices,
/// it will probably be a type that disables/re-enables interrupts. On real OS's,
/// it can be an actual mutex implementation.
///
/// The `Sleep` implementation can be used to put the event loop into a low-power
/// state using something like `cortex_m::wfi/e`.
// TODO(Josh) Investigate lock-free queues rather than using Mutexes. Might not
// really get us much for embedded devices where disabling interrupts is just an
// instruction away, but could be beneficial if threads get involved.
pub struct AllocExecutor<'a, R, S>
where
    R: RawMutex + Send + Sync,
    S: Sleep,
{
    registry: Arena<Task<'a>>,
    queue: QueueHandle<'a, R>,
    alarm: <S as Sleep>::Alarm,
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
    /// Spawn a `FutureObj` into the corresponding `AllocExecutor`
    pub fn spawn_obj(&mut self, future: FutureObj<'a, ()>) {
        self.0.lock().push_back(QueueItem::Spawn(future));
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
        self.spawn_obj(make_obj(future));
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
            alarm: S::make_alarm(),
        }
    }

    /// Get a handle to a `Spawner` that can be passed to `Future` constructors
    /// to spawn even *more* `Future`s
    pub fn spawner(&self) -> Spawner<'a, R> {
        Spawner(self.queue.clone())
    }

    /// Spawn a `FutureObj` into the executor.
    ///
    /// Thanks to the `'a` lifetime bound, these don't necessarily have to be
    /// `'static` `Futures`, so long as they outlive the executor.
    pub fn spawn_obj(&mut self, future: FutureObj<'a, ()>) {
        let id = self.registry.insert(Task::new(future));

        // Safety: The QueueWaker only deals in 'static lifetimed things, i.e.
        // task `Index`es, only writes to the queue, and will never give anyone
        // else this transmuted version.
        let static_queue: QueueHandle<'static, R> = unsafe { mem::transmute(self.queue.clone()) };

        let queue_waker = Arc::new(QueueWaker::new(static_queue, id, self.alarm.clone()));

        // Safety: Our QueueWaker does the exact same thing for local vs
        // non-local wake.
        let local_waker = unsafe { local_waker(queue_waker) };
        self.registry.get_mut(id).unwrap().set_waker(local_waker);

        // Insert the newly spawned task into the queue to be polled
        self.queue.lock().push_back(QueueItem::Poll(id));
    }

    /// Spawn a `Future` into the executor.
    ///
    /// Thanks to the `'a` lifetime bound, these don't necessarily have to be
    /// `'static` `Futures`, so long as they outlive the executor.
    pub fn spawn<F>(&mut self, future: F)
    where
        F: Future<Output = ()> + Send + 'a,
    {
        self.spawn_obj(make_obj(future));
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

            // Safety: Our waker doesn't do anything special for wake_local vs
            // wake, so this is safe.
            match future.poll(waker) {
                Poll::Ready(_) => {
                    self.registry.remove(id);
                }
                Poll::Pending => {}
            }
        }
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
        // Cloning this pointer at the start so that we don't anger the borrow
        // checking gods.
        let queue = self.queue.clone();

        loop {
            while let Some(item) = queue.lock().pop_front() {
                match item {
                    QueueItem::Poll(id) => {
                        self.poll_task(id);
                    }
                    QueueItem::Spawn(task) => {
                        self.spawn_obj(task);
                    }
                }
            }
            if self.registry.is_empty() {
                break;
            }
            S::sleep(&self.alarm);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use core::sync::atomic::{
        AtomicBool,
        Ordering,
        ATOMIC_BOOL_INIT,
    };
    use embrio_async::{
        async_block,
        r#await,
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

    struct NopSleep;
    #[derive(Copy, Clone)]
    struct NopAlarm;

    impl Sleep for NopSleep {
        type Alarm = NopAlarm;

        fn make_alarm() -> NopAlarm {
            NopAlarm
        }
        fn sleep(_: &NopAlarm) {}
    }

    impl Alarm for NopAlarm {
        fn ring(&self) {}
    }

    fn foo() -> impl Future<Output = i32> {
        async_block!({ 5 })
    }

    fn bar() -> impl Future<Output = i32> {
        async_block!({
            let a = r#await!(foo());
            println!("{}", a);
            let b = a + 1;
            b
        })
    }

    fn baz<S: Spawn>(mut spawner: S) -> impl Future<Output = ()> {
        async_block!({
            let c = r#await!(bar());
            for i in c..25 {
                let spam = async_block!({ println!("{}", i) });
                spawner.spawn_obj(make_obj(spam)).unwrap();
            }
        })
    }

    #[test]
    fn executor() {
        let mut executor = AllocExecutor::<RawSpinlock, NopSleep>::new();
        let mut spawner = executor.spawner();
        let entry = async_block!({
            for i in 0..10 {
                spawner.spawn_obj(make_obj(async_block! {
                    println!("{}", i);
                }));
            }
        });
        executor.spawn(entry);
        executor.spawn(baz(executor.spawner()));
        executor.run();
    }
}
