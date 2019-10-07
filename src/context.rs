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
use crate::addr::send::AddrSender;
use std::process::Output;

pub type ActorItem<A> = LocalBoxFuture<'static, Option<BoxAction<A>>>;


pub struct Context<A: Actor> {
    sender: AddrSender<A>,
    mailbox: Receiver<BoxAction<A>>,
    new_items: Vec<ActorItem<A>>,

    items: FuturesUnordered<ActorItem<A>>,

    waker: Option<Waker>,
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


impl<A: Actor> Future for Context<A> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        unsafe {
            'main: loop {
                let mut modified = false;
                let mut this = Pin::get_unchecked_mut(self.as_mut());
                this.waker = Some(cx.waker().clone());


                for item in this.new_items.drain(..) {
                    this.items.push(item);
                }
                'mb: loop {
                    match Pin::new_unchecked(&mut this.mailbox).poll_next(cx) {
                        Poll::Ready(Some(next)) => {
                            modified = true;
                            next.open(&mut this);
                        }
                        Poll::Ready(None) => {
                            panic!("Malbox closed")
                        }
                        Poll::Pending => {
                            break 'mb;
                        }
                    }
                }
                'it: loop {
                    match Pin::new_unchecked(&mut this.items).poll_next(cx) {
                        Poll::Ready(Some(next)) => {
                            if let Some(envelope) = next {
                                println!("Opening item");
                                modified = true;
                                envelope.open(this);
                            }
                        }
                        Poll::Ready(None) => {
                            break 'it;
                        }
                        Poll::Pending => {
                            break 'it;
                        }
                    }
                }

                if !modified {
                    break 'main;
                }
            }
            return Poll::Pending;
        }
    }
}

impl<A: Actor> Context<A>
{
    pub fn sender(&self) -> AddrSender<A> {
        self.sender.clone()
    }

    pub fn create<F, Fut>(create: F) -> Addr<A>
        where F: FnOnce(Addr<A>) -> Fut + Send + 'static,
              Fut: Future<Output=A> + 'static

    {
        let (tx, rx) = crate::addr::send::channel();

        let sender = tx.clone();


        let addr = Addr { sender: tx };
        let addr2 = addr.clone();
        rt::spawn(async move {
            let mut items = FuturesUnordered::new();
            items.push(pending().boxed_local());

            let actor = create(addr2).await;
            let mut context = Context {
                new_items: vec![],
                actor,
                sender,
                mailbox: rx,
                items,
                waker: None,
                running: true,
            };
            context.await;
        }.boxed_local());

        return addr;
    }

    pub fn stop(&mut self) {
        self.running = false;
    }

    pub fn add_item(&mut self, f: ActorItem<A>) {
        self.new_items.push(f);
        if let Some(ref waker) = &self.waker {
            waker.wake_by_ref();
        }
    }

    pub fn spawn<F, Fut>(self: &mut ContextRef<A>, f: F)
        where F: FnOnce(ContextRef<A>) -> Fut + 'static,
              Fut: Future<Output=()> + 'static {
        let this = self.make_clone();

        self.add_item(async move {
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

