use core::task::Poll;

use pin_utils::pin_mut;

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

// Super simple Wake implementation
// Sticks the Index into the queue and calls Alarm::ring
struct QueueWaker<R: RawMutex + Send + Sync + 'static, A: Alarm>(
    Arc<Mutex<R, VecDeque<Index>>>,
    Index,
    A,
);

impl<R, A> Wake for QueueWaker<R, A>
where
    R: RawMutex + Send + Sync + 'static,
    A: Alarm,
{
    fn wake(arc_self: &Arc<Self>) {
        arc_self.0.lock().push_back(arc_self.1);
        arc_self.2.ring();
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
    R: RawMutex + Send + Sync + 'static,
    S: Sleep,
{
    // Wow, so this is an ugly type. Sorry about that.
    // Anyway, we're storing our Wake-implementing type next to its task so that
    // we can re-use the exact same Arc every time we poll it. That way we're
    // not creating a new allocation on every poll and it gives the Future
    // implementations the ability to take advantage of the `will_wake*`
    // functions.
    registry: Arena<(
        FutureObj<'a, ()>,
        Option<Arc<QueueWaker<R, <S as Sleep>::Alarm>>>,
    )>,
    poll_queue: Arc<Mutex<R, VecDeque<Index>>>,
    spawn_queue: Arc<Mutex<R, VecDeque<FutureObj<'a, ()>>>>,
    alarm: <S as Sleep>::Alarm,
}

/// Spawner for an `AllocExecutor`
///
/// This can be cloned and passed to an async function to allow it to spawn more
/// tasks.
pub struct Spawner<'a, R>(Arc<Mutex<R, VecDeque<FutureObj<'a, ()>>>>)
where
    R: RawMutex + Send + Sync + 'static;

impl<'a, R> Spawner<'a, R>
where
    R: RawMutex + Send + Sync + 'static,
{
    /// Spawn a `FutureObj` into the corresponding `AllocExecutor`
    pub fn spawn_obj(&mut self, future: FutureObj<'a, ()>) {
        self.0.lock().push_back(future);
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
    R: RawMutex + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Spawner(self.0.clone())
    }
}

impl<'a, R> Spawn for Spawner<'a, R>
where
    R: RawMutex + Send + Sync + 'static,
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
        // TODO(Josh) `with_capacity`?
        AllocExecutor {
            registry: Arena::new(),
            poll_queue: Arc::new(Mutex::new(Default::default())),
            spawn_queue: Arc::new(Mutex::new(Default::default())),
            alarm: S::make_alarm(),
        }
    }

    /// Get a handle to a `Spawner` that can be passed to `Future` constructors
    /// to spawn even *more* `Future`s
    pub fn spawner(&self) -> Spawner<'a, R> {
        Spawner(self.spawn_queue.clone())
    }

    /// Spawn a `FutureObj` into the executor.
    ///
    /// Thanks to the `'a` lifetime bound, these don't necessarily have to be
    /// `'static` `Futures`, so long as they outlive the executor.
    pub fn spawn_obj(&mut self, future: FutureObj<'a, ()>) {
        let id = self.registry.insert((future, None));
        self.registry.get_mut(id).unwrap().1 = Some(Arc::new(QueueWaker(
            self.poll_queue.clone(),
            id,
            self.alarm.clone(),
        )));
        self.poll_queue.lock().push_back(id);
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

    /// Run the executor
    ///
    /// Each loop will poll at most one task from the queue and then check for
    /// newly spawned tasks. If there are no new tasks spawned and nothing left
    /// in the queue, the executor will attempt to sleep.
    ///
    /// Once there's nothing to spawn and nothing left in the registry, the
    /// executor will return.
    pub fn run(&mut self) {
        // Cloning these pointers at the start so that we don't anger the borrow
        // checking gods.
        let poll_queue = self.poll_queue.clone();
        let spawn_queue = self.spawn_queue.clone();

        loop {
            // This will be the queue length *after* the front is popped.
            // We're only going to handle one task per loop so that futures that
            // call wake immediately don't starve the spawner. We'll use the
            // remaining queue length to decide whether we need to sleep or not.
            let (queue_len, front) = {
                let mut queue = poll_queue.lock();
                let front = queue.pop_front();
                let queue_len = queue.len();
                (queue_len, front)
            };

            // It's possible that the waker is still hanging out somewhere and
            // getting called even though its task is gone. If so, we can just
            // skip it.
            if let Some((future, waker, id)) =
                front.and_then(|id| self.registry.get_mut(id).map(|(f, w)| (f, w, id)))
            {
                pin_mut!(future);

                let waker = waker.as_ref().expect("waker not set").clone();

                // Our waker doesn't do anything special for wake_local vs wake,
                // so this is safe.
                match future.poll(&unsafe { local_waker(waker) }) {
                    Poll::Ready(_) => {
                        self.registry.remove(id);
                    }
                    Poll::Pending => {}
                }
            }

            let mut spawn_queue = spawn_queue.lock();
            if spawn_queue.is_empty() {
                // if there's nothing to spawn and nothing left in the task
                // registry, there's nothing more to do and we can break.
                // However, if the registry isn't empty, we need to know if there
                // are more things waiting to be polled before deciding to sleep.
                if self.registry.is_empty() {
                    break;
                } else if queue_len == 0 {
                    S::sleep(&self.alarm);
                }
            } else {
                // If there *are* things to spawn, those will go straight into
                // the poll queue, so we don't need to sleep here either.
                while let Some(future) = spawn_queue.pop_front() {
                    self.spawn_obj(future)
                }
            }
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
