use core::{
    marker::PhantomData,
    pin::Pin,
    task::Poll,
};

use alloc::{
    collections::VecDeque,
    sync::Arc,
    task::{
        local_waker_from_nonlocal,
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

pub struct AllocExecutor<'a, R, S>
where
    R: RawMutex + Send + Sync,
    S: Sleep,
{
    registry: Arena<FutureObj<'a, ()>>,
    poll_queue: Arc<Mutex<R, VecDeque<Index>>>,
    spawn_queue: Arc<Mutex<R, VecDeque<FutureObj<'static, ()>>>>,
    _ph: PhantomData<(&'a (), fn(S))>,
}

pub struct Spawner<R>(Arc<Mutex<R, VecDeque<FutureObj<'static, ()>>>>)
where
    R: RawMutex + Send + Sync + 'static;

impl<R> Clone for Spawner<R>
where
    R: RawMutex + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Spawner(self.0.clone())
    }
}

impl<R> Spawn for Spawner<R>
where
    R: RawMutex + Send + Sync + 'static,
{
    fn spawn_obj(&mut self, future: FutureObj<'static, ()>) -> Result<(), SpawnError> {
        self.0.lock().push_back(future);
        Ok(())
    }
}

impl<'a, R, S> AllocExecutor<'a, R, S>
where
    R: RawMutex + Send + Sync + 'static,
    S: Sleep,
{
    pub fn new() -> Self {
        AllocExecutor {
            registry: Arena::new(),
            poll_queue: Arc::new(Mutex::new(Default::default())),
            spawn_queue: Arc::new(Mutex::new(Default::default())),
            _ph: PhantomData,
        }
    }

    pub fn spawner(&self) -> Spawner<R> {
        Spawner(self.spawn_queue.clone())
    }

    fn spawn_obj(&mut self, future: FutureObj<'static, ()>) {
        let id = self.registry.insert(future);
        self.poll_queue.lock().push_back(id);
    }

    pub fn spawn<F>(&mut self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_obj(make_obj(future));
    }

    pub fn run(&mut self) {
        let sleep_handle = S::make_alarm();
        let poll_queue = self.poll_queue.clone();
        let spawn_queue = self.spawn_queue.clone();
        loop {
            while let Some((future, id)) = poll_queue
                .lock()
                .pop_front()
                .and_then(|id| self.registry.get_mut(id).map(|f| (f, id)))
            {
                let future = Pin::new(future);
                let waker = Arc::new(QueueWaker(poll_queue.clone(), id, sleep_handle.clone()));
                match future.poll(&local_waker_from_nonlocal(waker)) {
                    Poll::Ready(_) => {
                        self.registry.remove(id);
                    }
                    Poll::Pending => {}
                }
            }
            let mut spawn_queue = spawn_queue.lock();
            if spawn_queue.is_empty() {
                if self.registry.is_empty() {
                    break;
                } else {
                    S::sleep(&sleep_handle);
                }
            } else {
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
                spawner
                    .spawn_obj(make_obj(async_block! {
                        println!("{}", i);
                    }))
                    .unwrap();
            }
        });
        executor.spawn(entry);
        executor.spawn(baz(executor.spawner()));
        executor.run();
    }
}
