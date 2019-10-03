use crate::prelude::*;
use crate::actor::*;

use futures::channel::mpsc::Sender;
use crate::{AsyncRef, Context};
use futures::SinkExt;
use futures::channel::oneshot::{Sender as OneSender, Receiver as OneReceiver, channel as oneshot};
use futures::executor::LocalSpawner;


pub enum Envelope<'a, A: Actor> {
    Noop,
    Func(Box<dyn FnOnce(&mut Context<A>) + Send + 'a>),
}

impl<'a, A: Actor> Envelope<'a, A> {
    pub(crate) fn noop() -> Envelope<'static, A> { Envelope::Noop }
    pub(crate) fn func<F>(f: F) -> Self
        where for<'r, 's> F: std::ops::FnOnce(&'r mut Context<'s, A>,) + Send + 'a
    {
        Envelope::Func(Box::new(f))
    }
    pub(crate) fn boxed(f: Box<dyn FnOnce(&mut Context<A>) + Send + 'static>) -> Self {
        Envelope::Func(f)
    }
    pub(crate) fn apply(self, ctx: &mut Context<A>) {
        match self {
            Envelope::Noop => {}
            Envelope::Func(fun) => {
                fun(ctx)
            }
        }
    }
}

pub trait Response<A: Actor, M: Message> {
    fn respond(self, ctx: &mut Context<A>, tx: OneSender<M::Result>);
}

impl<A: Actor, M, F> Response<A, M> for F
    where F: Future<Output=M::Result> + 'static,
          M: Message
{
    fn respond(self, ctx: &mut Context<A>, mut tx: OneSender<M::Result>) {
        ctx.spawn(async {
            tx.send(self.await);
        });
    }
}

pub struct Addr<A: Actor> {
    pub(crate) sender: Sender<Envelope<'static, A>>,
}

unsafe impl<A: Actor> Send for Addr<A> {}

pub trait Message: Send + 'static {
    type Result;
}

impl<A: Actor> Addr<A> {
    pub async fn send<M: Message>(&self, msg: M) -> Result<M::Result, futures::channel::oneshot::Canceled>
        where A: Handler<M>, M::Result: Send

    {
        let mut sender = self.sender.clone();
        let (mut tx, rx) = oneshot();
        let envelope = Envelope::func(move |this: &mut Context<A>| {
            println!("Before handle tx");
            let tx = tx;
            let responder = this.handle(msg);
            responder.respond(this, tx);
            println!("Dropping tx");
        });

        sender.send(envelope).await;
        rx.await
    }
}
