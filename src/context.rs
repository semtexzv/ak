use crate::prelude::*;
use crate::actor::Actor;
use crate::types::Suspend;
use crate::addr::{Addr, Envelope};
use std::task;
use futures::{
    Stream,
    channel::mpsc::Receiver,
};
use std::sync::Arc;
use std::cell::RefCell;

// Heap-stored information about an actor.
// Should contain it's mailbox
pub struct RawContext<A: Actor> {
    _a: PhantomData<A>,
    mailbox: Receiver<Envelope<A>>,
    items: Vec<Item<A>>,
    actor: A,
}

struct Item<A> {
    _a: PhantomData<A>,
    f: LocalBoxFuture<'static, ()>,
}

impl<A: Actor + Unpin> Unpin for RawContext<A> {}

impl<A: Actor> Deref for RawContext<A> {
    type Target = A;
    fn deref(&self) -> &A {
        &self.actor
    }
}

impl<A: Actor> DerefMut for RawContext<A> {
    fn deref_mut(&mut self) -> &mut A {
        &mut self.actor
    }
}


impl<A: Actor> RawContext<A> {
    fn spawn<F>(&mut self, f: F)
        where F: Future<Output=()> + 'static
    {
        self.items.push(Item { _a: PhantomData, f: unsafe { Pin::new_unchecked(box f) } });
    }
}

// this struct contains pinned box to raw context values
pub struct Context<A: Actor>(Pin<Box<RawContext<A>>>);

impl<A: Actor + Unpin> Context<A> {
    pub fn new<F: FnOnce() -> A + Send + 'static>(create: F) -> Addr<A> {
        let (tx, rx) = futures::channel::mpsc::channel(1);
        std::thread::spawn(|| {
            let pinned = Pin::new(box RawContext {
                _a: PhantomData,
                actor: create(),
                mailbox: rx,
                items: vec![],
            });
            let ctx = Self(pinned);
            let mut rt = tokio::runtime::current_thread::Builder::new().build().unwrap();
            rt.spawn(unsafe { Pin::new_unchecked(box ctx) });
            rt.run();
        });
        return Addr { sender: Arc::new(RefCell::new(tx)) };
    }
    pub fn spawn(self: &mut Context<A>, f: impl Future<Output=()> + 'static) {
        self.0.spawn(f)
    }
    pub fn suspend<O, F>(self: Context<A>, f: F) -> Suspend<A, O, F>
        where F: Future<Output=O> + 'static
    {
        Suspend { ctx: Some(self), f, _a: PhantomData }
    }
}

impl<A: Actor> Future for Context<A> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, task: &mut task::Context<'_>) -> Poll<Self::Output> {
        let mut this = unsafe { self.get_unchecked_mut() };
        println!("Polled");
        unsafe {
            'outer: for i in 0 .. 10 {
                match Pin::new(&mut this.0.as_mut().get_unchecked_mut().mailbox).poll_next(task) {
                    Poll::Pending => println!("No messages"),
                    Poll::Ready(Some(msg)) => {

                    }
                    Poll::Ready(None) => {
                        println!("Finished")
                    }
                }
                for i in this.0.as_mut().get_unchecked_mut().items.iter_mut() {
                    match Future::poll(i.f.as_mut(), task) {
                        Poll::Pending => {
                            println!("Not ready")
                        },
                        Poll::Ready(()) => {
                            println!("Finished")
                        }
                    }
                }
            }
        }
        unimplemented!()
    }
}

impl<A: Actor> Deref for Context<A> {
    type Target = A;
    fn deref(&self) -> &A {
        &self.0.deref()
    }
}

// Actor must be movable - No internal references
impl<A: Actor + Unpin> DerefMut for Context<A> {
    fn deref_mut(&mut self) -> &mut A {
        self.0.deref_mut()
    }
}


