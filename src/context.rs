use crate::prelude::*;
use crate::actor::Actor;
use crate::addr::{Addr, Action, Message, BoxAction, FnAction};

use std::{
    collections::LinkedList,
    cell::{RefCell, UnsafeCell},
    sync::Arc,
    task,
    mem,
    ptr::NonNull,
};
use futures::{
    select,
    Stream,
    channel::mpsc::{Sender, Receiver},
    SinkExt,
    StreamExt,
    stream::FuturesUnordered,
};
use futures::task::SpawnExt;

pub type ActorItem<A> = LocalBoxFuture<'static, Option<BoxAction<A>>>;


pub struct Context<A: Actor> {
    stack: usize,
    sender: Sender<BoxAction<A>>,
    mailbox: Receiver<BoxAction<A>>,
    pub(crate) items: FuturesUnordered<ActorItem<A>>,
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


pub struct ContextRef<A: Actor> {
    data: *mut Context<A>,
}

impl<A: Actor> ContextRef<A> {
    pub(crate) fn from_ctx_ref(ctx: &mut Context<A>) -> Self {
        ContextRef {
            data: ctx as *mut _
        }
    }

    pub(crate) fn make_clone(&self) -> Self {
        Self {
            data: self.data
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
        let mut ctx_ref = ContextRef::from_ctx_ref(&mut self);
        Actor::started(&mut ctx_ref);

        'main: loop {
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
                        msg.open(&mut self);
                    } else {
                        println!("Mailbox has closed");
                        break 'main;
                    }
                }
                Either::Right((item, _)) => {
                    if let Some(env_opt) = item {
                        if let Some(envelope) = env_opt {
                            envelope.open(&mut self);
                        }
                    } else {
                        println!("Internal future none");
                    }
                }
            }
        }
        Actor::stopping(&mut ctx_ref);
    }


    pub fn addr(&self) -> Sender<BoxAction<A>> {
        self.sender.clone()
    }


    pub fn create<F: FnOnce(Addr<A>) -> A + Send + 'static>(create: F) -> Addr<A>
    {
        let (tx, rx) = futures::channel::mpsc::channel(10);

        let sender = tx.clone();


        let addr = Addr { sender: tx };
        let addr2 = addr.clone();
        //std::thread::spawn(|| {
        rt::spawn(|| async move {
            let mut items = FuturesUnordered::new();
            items.push(pending().boxed_local());

            let mut context = Context {
                stack: 0,
                actor: create(addr2),
                sender,
                mailbox: rx,
                items,
                running: true,
            };
            context.into_future().await;
        });
        rt::run(async {});
        // });

        return addr;
    }

    pub fn stop(&mut self) {
        self.running = false;
    }

    pub fn spawn<F, Fut>(self: &mut ContextRef<A>, f: F)
        where F: FnOnce(ContextRef<A>) -> Fut + 'static,
              Fut: Future<Output=()> + 'static {
        let this = self.make_clone();
        // TODO: Check whether this is safe
        self.items.push(async move {
            let fut = f(this);
            fut.await;
            None
        }.boxed_local());
    }

    pub async fn suspend<F>(mut self: ContextRef<A>, f: F) -> (ContextRef<A>, F::Output)
        where F: Future + 'static,

    {
        (self, f.await)
    }
}

