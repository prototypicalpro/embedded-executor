pub trait Alarm: Clone + Send + Sync + 'static {
    fn ring(&self);
}

pub trait Sleep {
    type Alarm: Alarm;

    fn make_alarm() -> Self::Alarm;
    fn sleep(handle: &Self::Alarm);
}
