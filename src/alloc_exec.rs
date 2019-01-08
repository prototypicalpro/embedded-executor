use core::{
    marker::PhantomData,
    mem,
    pin::Pin,
};

use alloc::{
    collections::VecDeque,
    sync::Arc,
    task::local_waker_from_nonlocal,
};

use futures::{
    future::FutureObj,
    task::{
        Poll,
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
    core_traits::*,
    prelude::*,
};

impl<I> PollQueue for VecDeque<I>
where
    I: Copy + Send + Sync + 'static,
{
    type Id = I;

    fn enqueue(&mut self, id: I) {
        self.push_back(id);
    }

    fn dequeue(&mut self) -> Option<I> {
        self.pop_front()
    }
}

unsafe impl<R, I> UnsafePollQueue<R> for Arc<Mutex<R, VecDeque<I>>>
where
    R: RawMutex + Send + Sync,
    I: Copy + Send + Sync + 'static,
{
    type Inner = VecDeque<I>;
    fn into_shared(self) -> *const Mutex<R, Self::Inner> {
        Arc::into_raw(self)
    }
    unsafe fn clone_raw(ptr: *const Mutex<R, Self::Inner>) -> *const Mutex<R, Self::Inner> {
        let orig = Arc::from_raw(ptr);
        let new = Arc::into_raw(orig.clone());
        mem::forget(orig);
        new
    }
    unsafe fn drop_raw(ptr: *const Mutex<R, Self::Inner>) {
        Arc::from_raw(ptr);
    }
}

impl TaskReg<'static> for Arena<FutureObj<'static, ()>> {
    type Id = Index;

    fn register(&mut self, future: FutureObj<'static, ()>) -> Index {
        self.insert(future)
    }

    fn get_mut(&mut self, id: Index) -> Option<&mut FutureObj<'static, ()>> {
        self.get_mut(id)
    }

    fn deregister(&mut self, id: Index) {
        self.remove(id);
    }

    fn is_empty(&self) -> bool {
        self.is_empty()
    }
}

pub struct Executor<'t, T, Q, R, S>
where
    T: TaskReg<'t>,
    Q: UnsafePollQueue<R>,
    R: RawMutex + Send + Sync,
    S: Sleep,
{
    registry: T,
    poll_queue: *const Mutex<R, <Q as UnsafePollQueue<R>>::Inner>,
    spawn_queue: Arc<Mutex<R, VecDeque<FutureObj<'static, ()>>>>,
    _sleep: PhantomData<fn(S, &'t ())>,
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

impl<'t, T, Q, R, S> Executor<'t, T, Q, R, S>
where
    Q: UnsafePollQueue<R> + 'static,
    T: TaskReg<'t, Id = <<Q as UnsafePollQueue<R>>::Inner as PollQueue>::Id>,
    R: RawMutex + Send + Sync + 'static,
    S: Sleep,
{
    pub fn new(registry: T, queue: Q) -> Self {
        Executor {
            registry,
            poll_queue: queue.into_shared(),
            spawn_queue: Arc::new(Mutex::new(Default::default())),
            _sleep: PhantomData,
        }
    }

    pub fn spawner(&self) -> Spawner<R> {
        Spawner(self.spawn_queue.clone())
    }

    fn spawn_obj(&mut self, future: FutureObj<'static, ()>) {
        let id = self.registry.register(future);
        unsafe { &*self.poll_queue }.lock().enqueue(id);
    }

    pub fn spawn<F>(&mut self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_obj(make_obj(future));
    }

    pub fn run(&mut self) {
        let sleep_handle = S::make_alarm();
        loop {
            let registry = &mut self.registry;
            let mut poll_queue = unsafe { &*self.poll_queue }.lock();
            while let Some((future, id)) = poll_queue
                .dequeue()
                .and_then(|id| registry.get_mut(id).map(|f| (f, id)))
            {
                let future = Pin::new(future);
                let waker: Arc<QueueWaker<Q, R, _>> =
                    Arc::new(QueueWaker::new(self.poll_queue, id, sleep_handle.clone()));
                match future.poll(&local_waker_from_nonlocal(waker)) {
                    Poll::Ready(_) => registry.deregister(id),
                    Poll::Pending => {}
                }
            }
            let mut spawn_queue = self.spawn_queue.lock();
            if spawn_queue.is_empty() {
                if self.registry.is_empty() {
                    break;
                } else {
                    S::sleep(&sleep_handle);
                }
            } else {
                while let Some(future) = spawn_queue.pop_front() {
                    let id = registry.register(future);
                    poll_queue.enqueue(id);
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::{
        Executor as CoreExecutor,
        *,
    };
    use embrio_async::{
        async_block,
        r#await,
    };
    use parking_lot::RawMutex;

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

    type Executor = CoreExecutor<
        'static,
        Arena<FutureObj<'static, ()>>,
        Arc<Mutex<RawMutex, VecDeque<Index>>>,
        RawMutex,
        NopSleep,
    >;

    #[test]
    fn executor() {
        let mut executor = Executor::new(Arena::new(), Arc::new(Mutex::new(VecDeque::new())));
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
