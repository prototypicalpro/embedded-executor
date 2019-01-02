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
    future::{
        FutureObj,
        UnsafeFutureObj,
    },
    task::{
        LocalWaker,
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

impl<R, I> PollQueue for Mutex<R, VecDeque<I>>
where
    R: RawMutex + Send + Sync + 'static,
    I: Copy + Send + Sync + 'static,
{
    type Id = I;

    fn enqueue(&self, id: I) {
        self.lock().push_back(id);
    }

    fn dequeue(&self) -> Option<I> {
        self.lock().pop_front()
    }
}

struct QueueWaker<Q: PollQueue, A: Alarm>(Arc<Q>, <Q as PollQueue>::Id, A);

impl<Q, A> Wake for QueueWaker<Q, A>
where
    Q: PollQueue + Send + Sync,
    <Q as PollQueue>::Id: Copy + Send + Sync,
    A: Alarm + Send + Sync,
{
    fn wake(arc_self: &Arc<Self>) {
        arc_self.0.enqueue(arc_self.1);
        arc_self.2.ring();
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

pub struct Executor<R, S>
where
    R: RawMutex + Send + Sync,
    S: Sleep,
{
    registry: Arena<FutureObj<'static, ()>>,
    poll_queue: Arc<Mutex<R, VecDeque<Index>>>,
    spawn_queue: Arc<Mutex<R, VecDeque<FutureObj<'static, ()>>>>,
    _sleep: PhantomData<fn(S)>,
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

struct FutureBox<F>(Box<F>);

unsafe impl<'a, T, F> UnsafeFutureObj<'a, T> for FutureBox<F>
where
    F: Future<Output = T> + 'a,
{
    fn into_raw(self) -> *mut () {
        Box::into_raw(self.0) as *mut ()
    }

    unsafe fn poll(ptr: *mut (), lw: &LocalWaker) -> Poll<T> {
        let ptr = ptr as *mut F;
        let pin: Pin<&mut F> = Pin::new_unchecked(&mut *ptr);
        F::poll(pin, lw)
    }

    unsafe fn drop(ptr: *mut ()) {
        drop(Box::from_raw(ptr as *mut F))
    }
}

fn make_obj<F>(future: F) -> FutureObj<'static, <F as Future>::Output>
where
    F: Future + Send + 'static,
{
    FutureObj::new(FutureBox(Box::new(future)))
}

impl<R, S> Executor<R, S>
where
    R: RawMutex + Send + Sync + 'static,
    S: Sleep,
{
    pub fn new() -> Self {
        Executor {
            registry: Arena::new(),
            poll_queue: Arc::new(Mutex::new(Default::default())),
            spawn_queue: Arc::new(Mutex::new(Default::default())),
            _sleep: PhantomData,
        }
    }

    pub fn spawner(&self) -> Spawner<R> {
        Spawner(self.spawn_queue.clone())
    }

    fn spawn_obj(&mut self, future: FutureObj<'static, ()>) {
        let id = self.registry.register(future);
        self.poll_queue.enqueue(id);
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
            while let Some((future, id)) = self
                .poll_queue
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

#[cfg(test)]
mod test {
    use super::*;
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

    async fn foo() -> i32 {
        5
    }

    async fn bar() -> i32 {
        let a = await!(foo());
        println!("{}", a);
        let b = a + 1;
        b
    }

    async fn baz<S: Spawn>(mut spawner: S) {
        let c = await!(bar());
        for i in c..25 {
            let spam = async move { println!("{}", i) };
            spawner.spawn_obj(make_obj(spam)).unwrap();
        }
    }

    #[test]
    fn executor() {
        let mut executor = Executor::<RawMutex, NopSleep>::new();
        let mut spawner = executor.spawner();
        let entry = async move {
            for i in 0..10 {
                spawner
                    .spawn_obj(make_obj(
                        async move {
                            println!("{}", i);
                        },
                    ))
                    .unwrap();
            }
        };
        executor.spawn(entry);
        executor.spawn(baz(executor.spawner()));
        executor.run();
    }
}
