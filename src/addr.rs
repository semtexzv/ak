use crate::prelude::*;
use crate::actor::*;
use std::sync::mpsc::Sender;

pub struct Envelope<A: Actor> {
    _p: PhantomData<A>,
}

pub struct Addr<A: Actor> {
    sender: Sender<Envelope<A>>,
}

pub trait Message {
    type Result;
}

impl<A: Actor> Addr<A> {
    fn send<M: Message>(&self, msg : M) where A: Handler<M> {
        unimplemented!()
    }
    fn do_send<M: Message>(&self, msg : M) where A : Handler<M> {
        unimplemented!()
    }
}