use futures::future::FutureObj;

pub trait TaskReg<'a> {
    type Id: Copy + Send + Sync + 'static;

    fn register(&mut self, future: FutureObj<'a, ()>) -> Self::Id;
    fn get_mut(&mut self, id: Self::Id) -> Option<&mut FutureObj<'a, ()>>;
    fn deregister(&mut self, id: Self::Id);
    fn is_empty(&self) -> bool;
}

pub trait PollQueue: Send + Sync + 'static {
    type Id: Copy + Send + Sync + 'static;

    fn enqueue(&self, id: Self::Id);
    fn dequeue(&self) -> Option<Self::Id>;
}

pub trait Alarm: Clone + Send + Sync + 'static {
    fn ring(&self);
}

pub trait Sleep {
    type Alarm: Alarm;

    fn make_alarm() -> Self::Alarm;
    fn sleep(handle: &Self::Alarm);
}
