use crate::prelude::*;
use crate::context::Context;
use crate::types::Return;

pub trait Actor: Sized + 'static {}


pub trait Handler<M>: Actor {
    type Result;
    fn handle(self: Context<Self>, m: M) -> Return<Self, Self::Result>;
}
