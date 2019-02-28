use crate::prelude::*;
use crate::actor::{Actor};
use crate::types::Suspend;

// Heap-stored information about an actor.
// Should contain it's mailbox
pub struct RawContext<A: Actor> {
    _a: PhantomData<A>,
    actor: A,
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

// this struct contains pinned box to raw context values
pub struct Context<A: Actor>(Pin<Box<RawContext<A>>>);

impl<A: Actor + Unpin> Context<A> {
    pub fn new(actor: A) -> Self {
        let pinned = Pin::new(box RawContext {
            _a: PhantomData,
            actor,
        });
        Self(pinned)
    }
    pub fn spawn(self: &mut Context<A>, f: impl Future<Output=()>) {

    }
    pub fn suspend<O, F>(self: Context<A>, f: F) -> Suspend<A, O, F>
        where F: Future<Output=O>
    {
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


