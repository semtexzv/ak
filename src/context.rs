use crate::prelude::*;
use crate::actor::Actor;

use crate::addr::{Addr, Envelope};
use std::{task, mem};
use futures::{select, Stream, channel::mpsc::{Sender, Receiver}, SinkExt, StreamExt};
use std::sync::Arc;
use std::cell::{RefCell, UnsafeCell};
use futures::stream::FuturesUnordered;
use std::collections::LinkedList;


// Heap-stored information about an actor.
// Should contain it's mailbox
pub struct Context<'a, A: Actor> {
    _a: PhantomData<A>,
    sender: Sender<Envelope<'static, A>>,
    mailbox: Receiver<Envelope<'static, A>>,
    items: FuturesUnordered<LocalBoxFuture<'a, Envelope<'a, A>>>,
    running: bool,
    actor: A,
}

impl<'a, A: Actor> Deref for Context<'a, A> {
    type Target = A;
    fn deref(&self) -> &A {
        &self.actor
    }
}

impl<'a, A: Actor> DerefMut for Context<'a, A> {
    fn deref_mut(&mut self) -> &mut A {
        &mut self.actor
    }
}

impl<'a, A: Actor> Context<'a, A>
{
    fn future(mut self) -> impl Future<Output=()> + 'a {
        async move {
            while self.running {
                let joined = select((&mut self.mailbox).next(), (&mut self.items).next());

                match joined.await {
                    // We have received a message
                    Either::Left((mb, _)) => {
                        if let Some(mut mb) = mb {
                            mb.apply(&mut self)
                        } else {}
                    }
                    // Internal future has resolved
                    Either::Right((item, _)) => {
                        if let Some(mut item) = item {
                            item.apply(&mut self)
                        }
                        // We did not get any internal futures resolved
                    }
                }
            }

            println!("Finished")
        }
    }

    pub fn sender(&self) -> Sender<Envelope<'static, A>> {
        self.sender.clone()
    }
    pub fn local_sender(&self) -> Sender<Envelope<A>> {
        unsafe { mem::transmute(self.sender.clone()) }
    }
    pub fn get_ref<'b>(&'b self) -> AsyncRef<A> {
        unsafe { AsyncRef(std::mem::transmute(self.sender.clone())) }
    }

    pub fn start<F: FnOnce() -> A + Send + 'static>(create: F) -> Addr<A> {
        let (tx, rx) = futures::channel::mpsc::channel(1);

        let sender = tx.clone();
        std::thread::spawn(move || {
            let context = Context {
                _a: PhantomData,
                actor: create(),
                sender,
                mailbox: rx,
                items: FuturesUnordered::new(),
                running: true,
            };

            let mut rt = tokio::runtime::current_thread::Builder::new().build().unwrap();
            rt.spawn(context.future());
            rt.run().unwrap();
        });

        return Addr { sender: tx };
    }

    pub fn stop(&mut self) {
        self.running = false;
    }

    pub fn spawn<F: Future<Output=()> + 'a>(&mut self, f: F) {
        self.items.push(async {
            f.await;
            Envelope::Noop
        }.boxed_local());
    }
}


// this struct contains pinned box to raw context values
pub struct AsyncRef<A: Actor>(Sender<Envelope<'static, A>>);

impl<A: Actor> AsyncRef<A> {
    pub async fn sync<F, O>(mut self, f: F) -> O
        where F: FnOnce(&mut Context<A>) -> O + Send + 'static,
              O: Send + 'static
    {
        let (mut tx, rx) = oneshot::channel();
        self.0.send(Envelope::func(move |this: &mut Context<A>| {
            let res = f(this);
            tx.send(res);
        })).await;
        rx.await.unwrap()
    }

    pub fn within<Fn, FF, O>(mut self, f: Fn) -> impl Future<Output=(AsyncRef<A>, O)>
        where Fn: FnOnce(&mut Context<A>) -> FF + Send + 'static,
              FF: Future<Output=O> + 'static,
              O: Send + 'static,
    {
        //let (mut tx, rx) = oneshot::channel();
        async {
            /*self.0.send(Envelope::func(move |this: &mut Context<A>| {
                let res = f(this);
                /*this.spawn(async {
                    let res = res.await;
                    tx.send(res);
                });*/
            })).await;
            (self, rx.await.unwrap())*/
            unimplemented!()
        }
    }
}

