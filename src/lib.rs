#![feature(type_alias_impl_trait, arbitrary_self_types, impl_trait_in_assoc_type)]
#![allow(unused_imports, unused_mut, unused_variables, dead_code)]
#![feature(coroutines, coroutine_trait)]

pub mod prelude;
pub mod actor;

pub mod addr;
pub mod context;
pub mod types;

pub use rt;
pub use std::future::Future;
pub use crate::addr::{Addr, Message};
pub use crate::actor::{Actor, Handler};
pub use crate::context::{Context, ContextRef};
pub use codegen::suspend;