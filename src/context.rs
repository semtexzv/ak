use crate::prelude::*;
use crate::actor::Actor;

use crate::addr::{Addr, Envelope, Message, BoxEnvelope};
use std::{task, mem};
use futures::{select, Stream, channel::mpsc::{Sender, Receiver}, SinkExt, StreamExt};
use std::sync::Arc;
use std::cell::{RefCell, UnsafeCell};
use futures::stream::FuturesUnordered;
use std::collections::LinkedList;
use std::ptr::NonNull;


pub type ActorItem<A> = LocalBoxFuture<'static, Option<BoxEnvelope<A>>>;


pub struct Context<A: Actor> {
    stack: usize,
    sender: Sender<BoxEnvelope<A>>,
    mailbox: Receiver<BoxEnvelope<A>>,
    items: FuturesUnordered<ActorItem<A>>,
    running: bool,
    actor: A,
}

impl<A: Actor> Deref for Context<A> {
    type Target = A;
    fn deref(&self) -> &A {
        &self.actor
    }
}

impl<A: Actor> DerefMut for Context<A> {
    fn deref_mut(&mut self) -> &mut A {
        &mut self.actor
    }
}


pub async fn dispatch<M: Message, A: Actor + crate::actor::Handler<M>>(ctx: &mut Context<A>, msg: M, mut tx: oneshot::Sender<M::Result>)
{
    use crate::actor::Handler;
    let mut ctx_ref = ContextRef::from_ctx_ref(ctx);
    let fut = ctx_ref.handle(msg);

    ctx.items.push(async {
        tx.send(fut.await);
        None
    }.boxed_local());
}


pub struct ContextRef<A: Actor> {
    data: *mut Context<A>,
}

impl<A: Actor> ContextRef<A> {
    pub(crate) fn from_ctx_ref(ctx: &mut Context<A>) -> Self {
        unsafe {
            ContextRef {
                data: ctx as *mut _
            }
        }
    }
}

impl<A: Actor> Deref for ContextRef<A> {
    type Target = Context<A>;
    fn deref(&self) -> &Context<A> {
        unsafe { self.data.as_ref().unwrap() }
    }
}

impl<A: Actor> DerefMut for ContextRef<A> {
    fn deref_mut(&mut self) -> &mut Context<A> {
        unsafe { self.data.as_mut().unwrap() }
    }
}


impl<A: Actor> Context<A>
{
    async fn into_future(mut self) {
        loop {
            if !self.running {
                return;
            }
            let mut next_msg = (&mut self.mailbox).next();
            pin_utils::pin_mut!(next_msg);

            let mut next_item = (&mut self.items).next();
            pin_utils::pin_mut!(next_item);

            match select(next_msg, next_item).await {
                Either::Left((msg, _)) => {
                    if let Some(msg) = msg {
                        println!("Opening new message");
                        msg.open(&mut self).await;
                    } else {
                        println!("Mailbox has closed");
                        return;
                    }
                }
                Either::Right((item, _)) => {
                    if let Some(env_opt) = item {
                        println!("Internal future completed");
                        if let Some(envelope) = env_opt {
                            envelope.open(&mut self).await;
                        }
                    } else {
                        println!("Internal future none");
                    }
                }
            }
        }
    }


    pub fn sender(&self) -> Sender<BoxEnvelope<A>> {
        self.sender.clone()
    }


    pub fn start<F: FnOnce() -> A + Send + 'static>(create: F) -> Addr<A> {
        let (tx, rx) = futures::channel::mpsc::channel(100);

        let sender = tx.clone();


        std::thread::spawn(move || {
            let mut items = FuturesUnordered::new();
            items.push(pending().boxed_local());

            let mut context = Context {
                stack: 0,
                actor: create(),
                sender,
                mailbox: rx,
                items,
                running: true,
            };

            let mut rt = tokio::runtime::current_thread::Builder::new().build().unwrap();

            rt.spawn(context.into_future());
            rt.run().unwrap();
        });

        return Addr { sender: tx };
    }

    pub fn stop(&mut self) {
        self.running = false;
    }

    pub async fn suspend<F>(mut self: ContextRef<A>, f: F) -> (ContextRef<A>, F::Output)
        where F: Future + 'static,

    {
        (self, f.await)
    }
}

