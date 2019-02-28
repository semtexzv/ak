use crate::prelude::*;

use crate::actor::Actor;
use crate::context::Context;

pub struct Return<A: Actor, O> {
    _a: PhantomData<A>,
    o: Box<Future<Output=(Context<A>, O)>>,
}

impl<A: Actor, O> Return<A, O> {
    pub fn now(ctx: Context<A>, o: O) -> Self {
        unimplemented!()
    }
    pub fn fut(o: impl Future<Output=(Context<A>, O)> + 'static) -> Self {
        Return {
            _a: PhantomData,
            o: box o,
        }
    }
}

pub struct Suspend<A: Actor, O, F: Future<Output=O>> {
    _a: PhantomData<A>,
    ctx: Option<Context<A>>,
    f: F,
}


impl<A: Actor, O, F: Future<Output=O>> Future for Suspend<A, O, F> {
    type Output = (Context<A>, O);
    fn poll(self: Pin<&mut Self>, lw: &Waker) -> Poll<Self::Output> {
        unsafe {
            let this = Pin::get_unchecked_mut(self);
            match Future::poll(Pin::new_unchecked(&mut this.f), lw) {
                Poll::Ready(o) => Poll::Ready((this.ctx.take().unwrap(), o)),
                Poll::Pending => Poll::Pending
            }
        }
    }
}


