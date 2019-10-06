use crate::prelude::*;
use crate::actor::*;

use futures::channel::mpsc::Sender;
use crate::{Context, dispatch, ContextRef};
use futures::{
    SinkExt,
    channel::oneshot::{Sender as OneSender, Receiver as OneReceiver, channel as oneshot, Canceled},
    executor::LocalSpawner,
};
use async_trait::async_trait;
use std::sync::Mutex;


pub type BoxEnvelope<A> = Box<dyn Envelope<A>>;

#[async_trait(?Send)]
pub trait Envelope<A: Actor>: Send {
    async fn open(self: Box<Self>, ctx: &mut Context<A>);
    fn boxed(self) -> BoxEnvelope<A>
        where Self: Sized + 'static {
        Box::new(self)
    }
}

pub struct MessageEnvelope<A, M: Message>(M, oneshot::Sender<M::Result>, PhantomData<A>);

unsafe impl<A, M: Message> Send for MessageEnvelope<A, M> {}


#[async_trait(? Send)]
impl<A, M> Envelope<A> for MessageEnvelope<A, M>
    where A: Actor + Handler<M>,
          M: Message + Send + 'static,
          M::Result: Send + 'static
{
    async fn open(self: Box<Self>, ctx: &mut Context<A>) {
        let res = dispatch(ctx, self.0, self.1).await;
    }
}


pub struct Addr<A: Actor> {
    pub(crate) sender: Sender<BoxEnvelope<A>>,
}

unsafe impl<A: Actor> Send for Addr<A> {}

pub trait Message: Send + 'static {
    type Result;
}

impl<A: Actor> Addr<A> {
    pub fn send<M: Message>(&self, msg: M) -> impl Future<Output=Result<M::Result, Canceled>> + 'static
        where A: Handler<M>, M::Result: Send {

        let mut sender = self.sender.clone();

        async move {
            let (mut tx, rx) = oneshot();
            let envelope = MessageEnvelope::<A, M>(msg, tx, PhantomData);
            let mut sent = sender.send(Box::new(envelope));
            sent.await.unwrap();
            rx.await
        }
    }
}
