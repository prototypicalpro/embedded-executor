use core::{
    marker::PhantomData,
    pin::Pin,
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
    future_box::*,
    prelude::*,
};

impl<I> PollQueue for VecDeque<I>
where
    I: Copy + Send + Sync + 'static,
{
    type Id = I;

    fn init() -> Self {
        VecDeque::new()
    }

    fn enqueue(&mut self, id: I) {
        self.push_back(id);
    }

    fn dequeue(&mut self) -> Option<I> {
        self.pop_front()
    }
}

struct QueueWaker<R: RawMutex + Send + Sync + 'static, Q: PollQueue, A: Alarm>(
    Arc<Mutex<R, Q>>,
    <Q as PollQueue>::Id,
    A,
);

impl<R, Q, A> Wake for QueueWaker<R, Q, A>
where
    R: RawMutex + Send + Sync + 'static,
    Q: PollQueue,
    A: Alarm,
{
    fn wake(arc_self: &Arc<Self>) {
        arc_self.0.lock().enqueue(arc_self.1);
        arc_self.2.ring();
    }
}

impl<'a> TaskReg<'a> for Arena<FutureObj<'a, ()>> {
    type Id = Index;

    fn init() -> Self {
        Arena::new()
    }

    fn register(&mut self, future: FutureObj<'a, ()>) -> Index {
        self.insert(future)
    }

    fn get_mut(&mut self, id: Index) -> Option<&mut FutureObj<'a, ()>> {
        self.get_mut(id)
    }

    fn deregister(&mut self, id: Index) {
        self.remove(id);
    }

    fn is_empty(&self) -> bool {
        self.is_empty()
    }
}

pub struct AllocExecutor<'a, T, Q, R, S>
where
    T: TaskReg<'a>,
    Q: PollQueue<Id = <T as TaskReg<'a>>::Id>,
    R: RawMutex + Send + Sync,
    S: Sleep,
{
    registry: T,
    poll_queue: Arc<Mutex<R, Q>>,
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

impl<'a, T, Q, R, S> AllocExecutor<'a, T, Q, R, S>
where
    T: TaskReg<'a>,
    Q: PollQueue<Id = <T as TaskReg<'a>>::Id>,
    R: RawMutex + Send + Sync + 'static,
    S: Sleep,
{
    pub fn new() -> Self {
        AllocExecutor {
            registry: T::init(),
            poll_queue: Arc::new(Mutex::new(Q::init())),
            spawn_queue: Arc::new(Mutex::new(Default::default())),
            _ph: PhantomData,
        }
    }

    pub fn spawner(&self) -> Spawner<R> {
        Spawner(self.spawn_queue.clone())
    }

    fn spawn_obj(&mut self, future: FutureObj<'static, ()>) {
        let id = self.registry.register(future);
        self.poll_queue.lock().enqueue(id);
    }

    pub fn spawn<F>(&mut self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_obj(make_obj(future));
    }

    pub fn run(&mut self) {
        let sleep_handle = S::make_alarm();
        let queue = self.poll_queue.clone();
        loop {
            while let Some((future, id)) = queue
                .lock()
                .dequeue()
                .and_then(|id| self.registry.get_mut(id).map(|f| (f, id)))
            {
                let future = Pin::new(future);
                let waker = Arc::new(QueueWaker(queue.clone(), id, sleep_handle.clone()));
                match future.poll(&local_waker_from_nonlocal(waker)) {
                    Poll::Ready(_) => self.registry.deregister(id),
                    Poll::Pending => {}
                }
            }
            let spawn_queue = self.spawn_queue.clone();
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

pub type Executor<'a, R, S> = AllocExecutor<'a, Arena<FutureObj<'a, ()>>, VecDeque<Index>, R, S>;

#[cfg(test)]
mod test {
    use super::*;
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

    #[test]
    fn executor() {
        let mut executor = Executor::<RawMutex, NopSleep>::new();
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
