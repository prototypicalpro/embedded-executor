use core::marker::PhantomData;

use futures::future::FutureObj;

use lock_api::{
    Mutex,
    RawMutex,
};

#[cfg(feature = "alloc")]
use alloc::{
    sync::Arc,
    task::Wake,
};

pub trait TaskReg<'a> {
    type Id: Copy + Send + Sync + 'static;

    fn register(&mut self, future: FutureObj<'a, ()>) -> Self::Id;
    fn get_mut(&mut self, id: Self::Id) -> Option<&mut FutureObj<'a, ()>>;
    fn deregister(&mut self, id: Self::Id);
    fn is_empty(&self) -> bool;
}

pub trait PollQueue: Send + 'static {
    type Id: Copy + Send + Sync + 'static;

    fn enqueue(&mut self, id: Self::Id);
    fn dequeue(&mut self) -> Option<Self::Id>;
}

pub unsafe trait UnsafePollQueue<R>
where
    R: RawMutex,
{
    type Inner: PollQueue + Send;
    fn into_shared(self) -> *const Mutex<R, Self::Inner>;
    unsafe fn clone_raw(ptr: *const Mutex<R, Self::Inner>) -> *const Mutex<R, Self::Inner>;
    unsafe fn drop_raw(ptr: *const Mutex<R, Self::Inner>);
}

unsafe impl<Q, R> UnsafePollQueue<R> for &'static Mutex<R, Q>
where
    Q: PollQueue,
    R: RawMutex,
{
    type Inner = Q;
    fn into_shared(self) -> *const Mutex<R, Self::Inner> {
        self as *const _
    }
    unsafe fn clone_raw(ptr: *const Mutex<R, Self::Inner>) -> *const Mutex<R, Self::Inner> {
        ptr
    }
    unsafe fn drop_raw(_: *const Mutex<R, Self::Inner>) {}
}

pub trait Alarm: Clone + Send + Sync + 'static {
    fn ring(&self);
}

pub trait Sleep {
    type Alarm: Alarm;

    fn make_alarm() -> Self::Alarm;
    fn sleep(handle: &Self::Alarm);
}

pub struct QueueWaker<Q, R, A>(
    *const Mutex<R, <Q as UnsafePollQueue<R>>::Inner>,
    <<Q as UnsafePollQueue<R>>::Inner as PollQueue>::Id,
    A,
    PhantomData<fn(&Q)>,
)
where
    Q: UnsafePollQueue<R> + 'static,
    R: RawMutex + Send + Sync,
    A: Alarm;

impl<Q, R, A> QueueWaker<Q, R, A>
where
    Q: UnsafePollQueue<R> + 'static,
    R: RawMutex + Send + Sync,
    A: Alarm,
{
    pub fn new(
        queue: *const Mutex<R, <Q as UnsafePollQueue<R>>::Inner>,
        id: <<Q as UnsafePollQueue<R>>::Inner as PollQueue>::Id,
        alarm: A,
    ) -> Self {
        QueueWaker(unsafe { Q::clone_raw(queue) }, id, alarm, PhantomData)
    }
}

impl<Q, R, A> Drop for QueueWaker<Q, R, A>
where
    Q: UnsafePollQueue<R>,
    R: RawMutex + Send + Sync,
    A: Alarm,
{
    fn drop(&mut self) {
        unsafe { Q::drop_raw(self.0) }
    }
}

unsafe impl<Q, R, A> Send for QueueWaker<Q, R, A>
where
    Q: UnsafePollQueue<R>,
    R: RawMutex + Send + Sync,
    A: Alarm,
{
}

unsafe impl<Q, R, A> Sync for QueueWaker<Q, R, A>
where
    Q: UnsafePollQueue<R>,
    R: RawMutex + Send + Sync,
    A: Alarm,
{
}

#[cfg(feature = "alloc")]
impl<Q, R, A> Wake for QueueWaker<Q, R, A>
where
    Q: UnsafePollQueue<R>,
    R: RawMutex + Send + Sync + 'static,
    A: Alarm,
{
    fn wake(arc_self: &Arc<Self>) {
        unsafe { &*arc_self.0 }.lock().enqueue(arc_self.1);
        arc_self.2.ring();
    }
}
