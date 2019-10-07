use crate::prelude::*;
use super::{Message, MessageAction};
use futures::sink::SinkExt;
use crate::{Actor, Handler};

use crate::addr::BoxAction;


pub trait TSender<M>: Send
    where
        M::Result: Send,
        M: Message + Send,
{
    fn inner_send(&self, msg: M) -> LocalBoxFuture<'static, M::Result>;

    fn boxed(&self) -> Box<dyn TSender<M>>;
}

pub fn channel<A : Actor>() -> (AddrSender<A>, futures::channel::mpsc::Receiver<BoxAction<A>>) {
    let (tx, rx) = futures::channel::mpsc::channel(2);

    (AddrSender { inner : tx}, rx)
}

pub struct AddrSender<A> {
    inner: futures::channel::mpsc::Sender<super::BoxAction<A>>,
}
impl<A> Clone for AddrSender<A> {
    fn clone(&self) -> Self {
        AddrSender {
            inner : self.inner.clone(),
        }
    }
}

impl<A, M> TSender<M> for AddrSender<A>
    where
        A : Actor + Handler<M>,
        M: Message + Send,
        M::Result: Send
{
    fn inner_send(&self, msg: M) -> LocalBoxFuture<'static, M::Result> {
        let (tx, rx) = futures::channel::oneshot::channel();
        let mut chan = self.inner.clone();
        async move {
            chan.send(Box::new(MessageAction(msg, tx))).await.unwrap();
            rx.await.unwrap()
        }.boxed_local()
    }

    fn boxed(&self) -> Box<dyn TSender<M>> {
        let sender : AddrSender<A> = self.clone();
        Box::new(sender)
    }
}

