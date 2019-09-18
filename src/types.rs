use crate::prelude::*;

use crate::actor::Actor;
use crate::context::Context;
use std::task;


pub struct Return<A: Actor, O> {
    _a: PhantomData<A>,
    o: Box<dyn Future<Output=(Context<A>, O)>>,
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

impl<A: Actor, F: Future<Output=(Context<A>, O)> + 'static, O> From<F> for Return<A, O> {
    fn from(f: F) -> Self {
        Return::fut(f)
    }
}

pub struct Suspend<A: Actor, O, F: Future<Output=O>> {
    pub(crate) _a: PhantomData<A>,
    pub(crate) ctx: Option<Context<A>>,
    pub(crate) f: F,
}


impl<A: Actor, O, F: Future<Output=O>> Future for Suspend<A, O, F> {
    type Output = (Context<A>, O);

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        unsafe {
            println!("Suspend polled");
            let this = Pin::get_unchecked_mut(self);
            match Future::poll(Pin::new_unchecked(&mut this.f), cx) {
                Poll::Ready(o) => Poll::Ready((this.ctx.take().unwrap(), o)),
                Poll::Pending => Poll::Pending
            }
        }
    }
}


