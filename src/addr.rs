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


pub type BoxAction<A> = Box<dyn Action<A>>;


pub trait Action<A: Actor>  {
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
        ctx.items.push(async {
            let _ = tx.send(fut.await);
            None
        }.boxed_local());
    }
}


pub struct FnAction<A, F>(pub(crate) F, PhantomData<A>) where
    A: Actor,
    F: FnOnce(ContextRef<A>) -> () + 'static;


impl<A, F, > Action<A> for FnAction<A, F>
    where
        A: Actor,
        F: FnOnce(ContextRef<A>) -> () + 'static
{
    fn open(self: Box<Self>, ctx: &mut Context<A>) {
        let res = self.0(ContextRef::from_ctx_ref(ctx));
    }
}

pub struct Addr<A: Actor> {
    pub(crate) sender: Sender<BoxAction<A>>,
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
    pub fn send<M: Message>(&self, msg: M) -> impl Future<Output=Result<M::Result, Canceled>> + 'static
        where A: Handler<M>, M::Result: Send {
        let sent = self.send_async(msg);
        async { sent.await.await }
    }

    /// Two-phase send. The first future is resolved when a message was sucessfully sent
    /// to the recipient, and the second one is resolved upon receiving the result of this
    /// operation. Can be used to asynchronously sent messages to different actors, and
    /// await for all of them, instead of using [Future::join]
    pub fn send_async<M: Message>(&self, msg: M) -> impl Future<Output=OneReceiver<M::Result>> + 'static
        where A: Handler<M>, M::Result: Send {
        let mut sender = self.sender.clone();
        let (mut tx, rx) = oneshot();
        let action = MessageAction::<M>(msg, tx);

        async move {
            let mut sent = sender.send(Box::new(action));
            sent.await.unwrap();
            rx
        }
    }
    pub fn recipient<M: Message>(&self) -> Recipient<M>
        where A: Handler<M>
    {
        return Recipient {
            sender: unimplemented!()
            // sender: self.sender.clone()
        };
    }
}

pub struct Recipient<M: Message> {
    sender: Sender<Box<MessageAction<M>>>,
}

impl<M: Message> Recipient<M> {
    /// Two-phase send. The first future is resolved when a message was sucessfully sent
   /// to the recipient, and the second one is resolved upon receiving the result of this
   /// operation. Can be used to asynchronously sent messages to different actors, and
   /// await for all of them, instead of using [Future::join]
    pub fn send_async(&self, msg: M) -> impl Future<Output=OneReceiver<M::Result>> + 'static
        where M::Result: Send {
        let mut sender = self.sender.clone();
        let (mut tx, rx) = oneshot();
        let action = MessageAction::<M>(msg, tx);

        async move {
            let mut sent = sender.send(Box::new(action));
            sent.await.unwrap();
            rx
        }
    }


    /// Sends a message and awaits for the result
   /// Note, if the returned future is not polled, the message is dropped
    pub fn send(&self, msg: M) -> impl Future<Output=Result<M::Result, Canceled>> + 'static
        where M::Result: Send {
        let sent = self.send_async(msg);
        async { sent.await.await }
    }
}