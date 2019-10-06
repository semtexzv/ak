use crate::prelude::*;
use crate::{Context, ContextRef};
use crate::addr::Message;

use async_trait::async_trait;
use std::ops::Generator;

pub trait Actor: Sized + 'static {}

pub trait Handler<M: Message>: Actor {
    type Future: Future<Output=M::Result> + 'static;
    fn handle(self: ContextRef<Self>, msg: M) -> Self::Future;
}
