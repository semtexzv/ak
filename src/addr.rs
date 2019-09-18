use crate::prelude::*;
use crate::actor::*;

use futures::channel::mpsc::Sender;
use crate::Context;
use futures::SinkExt;
use std::sync::{Arc, Mutex};
use std::cell::RefCell;

pub trait EnvelopeProxy {
    type Actor: Actor;
    fn handle(&mut self, act: &mut Context<Self::Actor>);
}

pub struct Envelope<A: Actor> {
    _p: PhantomData<A>,
    proxy: Box<dyn EnvelopeProxy<Actor=A> + Send + 'static>,
}

unsafe impl<A: Actor> Send for Envelope<A> {}

pub struct Addr<A: Actor> {
    pub(crate) sender: Arc<RefCell<Sender<Envelope<A>>>>,
}

unsafe impl<A: Actor> Send for Addr<A> {}

pub trait Message: Send + 'static {
    type Result;
}

struct SimpleProxy<A, M> {
    _a: PhantomData<A>,
    msg: Option<M>,
}

unsafe impl<A: Actor, M: Message> Send for SimpleProxy<A, M> {}

impl<A: Actor + Handler<M>, M: Message> EnvelopeProxy for SimpleProxy<A, M> {
    type Actor = A;

    fn handle(&mut self, mut act: &mut Context<Self::Actor>) {
        if let Some(msg) = self.msg.take() {
            let res = act.deref_mut().handle(msg);
        }
    }
}

impl<A: Actor> Addr<A> {
    pub async fn send<M: Message>(&self, msg: M)
        where A: Handler<M> {
        self.sender.deref().borrow_mut().send(Envelope {
            proxy: box SimpleProxy::<A, M> { _a: PhantomData, msg: Some(msg) },
            _p: PhantomData,
        }).await.unwrap();
    }
    pub fn do_send<M: Message>(&self, msg: M) where A: Handler<M> {
        unimplemented!()
    }
}