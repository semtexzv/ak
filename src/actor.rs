use crate::prelude::*;
use crate::context::{Context, ContextRef};
use crate::addr::Message;

use async_trait::async_trait;
use std::ops::Generator;
use crate::Addr;

pub trait Actor: Sized + 'static {
    fn started(self: &mut ContextRef<Self>) {}
    fn stopping(self: &mut ContextRef<Self>) {}
    fn start<F>(f: F) -> Addr<Self>
        where
            F: FnOnce(Addr<Self>) -> Self + Send + 'static
    {
        Context::create(|addr| async { f(addr) })
    }
    fn start_async<F, Fut>(f: F) -> Addr<Self>
        where
            F: FnOnce(Addr<Self>) -> Fut + Send + 'static,
            Fut: Future<Output=Self>  + 'static
    {
        Context::create(|addr| f(addr))
    }
}

pub trait Handler<M: Message>: Actor {
    type Future: Future<Output=M::Result> + 'static;
    fn handle(self: ContextRef<Self>, msg: M) -> Self::Future;
}
