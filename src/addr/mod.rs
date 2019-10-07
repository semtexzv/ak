use crate::prelude::*;
use crate::actor::*;

use crate::context::{Context, ContextRef};
use futures::{
    SinkExt,
    channel::mpsc::Sender,
    channel::oneshot::{Sender as OneSender, Receiver as OneReceiver, channel as oneshot, Canceled},
    executor::LocalSpawner,
};
use async_trait::async_trait;
use crate::addr::send::TSender;

pub mod send;

pub type BoxAction<A> = Box<dyn Action<A>>;


pub trait Action<A: Actor>: Send {
    fn open(self: Box<Self>, ctx: &mut Context<A>);
    fn boxed(self) -> BoxAction<A>
        where Self: Sized + 'static {
        Box::new(self)
    }
}

pub struct MessageAction<M: Message>(M, oneshot::Sender<M::Result>);

unsafe impl<M: Message> Send for MessageAction<M> {}

impl<A, M> Action<A> for MessageAction<M>
    where A: Actor + Handler<M>,
          M: Message + Send + 'static,
          M::Result: Send + 'static
{
    fn open(self: Box<Self>, ctx: &mut Context<A>) {
        use crate::actor::Handler;
        let mut ctx_ref = ContextRef::from_ctx_ref(ctx);
        let MessageAction(msg, tx) = *self;
        let fut = ctx_ref.handle(msg);

        ctx.add_item(async {
            let _ = tx.send(fut.await);
            None
        }.boxed_local());
    }
}

use std::sync::Mutex;
use futures::task::LocalSpawnExt;

pub struct FnAction<A, F>(pub(crate) F, PhantomData<fn() -> A>) where
    A: Actor,
    F: FnOnce(ContextRef<A>) -> () + 'static;


impl<A, F, > Action<A> for FnAction<A, F>
    where
        A: Actor,
        F: FnOnce(ContextRef<A>) -> () + Send + 'static
{
    fn open(self: Box<Self>, ctx: &mut Context<A>) {
        let res = self.0(ContextRef::from_ctx_ref(ctx));
    }
}

pub struct Addr<A: Actor> {
    pub(crate) sender: send::AddrSender<A>,
}

unsafe impl<A: Actor> Send for Addr<A> {}

pub trait Message: Send + 'static {
    type Result;
}

impl<A: Actor> Clone for Addr<A> {
    fn clone(&self) -> Self {
        Self { sender: self.sender.clone() }
    }
}

impl<A: Actor> Addr<A> {
    /// Sends a message and awaits for the result
   /// Note, if the returned future is not polled, the message is dropped
    pub fn send<M>(&self, msg: M) -> impl Future<Output=Result<M::Result, Canceled>> + 'static
        where
            A: Handler<M>,
            M: Message + Send,
            M::Result: Send {
        let mut sender = self.sender.clone();


        async move {
            Ok(sender.inner_send(msg).await)
        }
    }

    pub fn recipient<M: Message>(&self) -> Recipient<M>
        where A: Handler<M>,
              M: Send,
              M::Result: Send,
    {
        return Recipient {
            sender: self.sender.boxed(),
        };
    }
}

pub struct Recipient<M: Message> {
    sender: Box<dyn send::TSender<M>>,
}

impl<M: Message> Recipient<M> {
    /// Sends a message and awaits for the result
   /// Note, if the returned future is not polled, the message is dropped
    pub fn send(&self, msg: M) -> impl Future<Output=Result<M::Result, Canceled>> + 'static
        where M::Result: Send {
        let mut sender = self.sender.boxed();


        async move {
            Ok(sender.inner_send(msg).await)
        }
    }
}