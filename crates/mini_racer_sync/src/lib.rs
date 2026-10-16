#![forbid(unsafe_code)]

mod channel;
mod lock;
mod parker;
pub mod primitives;
mod termination;

pub use channel::{Channel, ChannelError, Nesting, Protocol, Side, ToRuby, ToV8};
pub use lock::{lock, wait, wait_timeout};
pub use parker::Notifier;
pub use termination::{
    DispatchId, DispatchKind, Halt, Interrupt, TermCell, TermError, TerminationCause, Terminator,
};
