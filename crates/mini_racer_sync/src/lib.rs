#![forbid(unsafe_code)]

mod channel;
mod handoff;
mod lock;
mod parker;
pub mod primitives;
mod termination;

pub use channel::{Channel, ChannelError, Nesting, Protocol, Side, ToRuby, ToV8};
pub use handoff::{Handoff, Outcome};
pub use lock::lock;
pub use termination::{
    DispatchId, DispatchKind, Halt, Interrupt, TermCell, TermError, TerminationCause, Terminator,
};
