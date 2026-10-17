#![forbid(unsafe_code)]

mod channel;
mod fork;
mod lock;
mod parker;
pub mod primitives;
mod tasks;
mod termination;

pub use channel::{Channel, ChannelError, Nesting, Notifier, Protocol, Side, ToRuby, ToV8};
pub use fork::{ForkGate, Phase, RunnerPhase};
pub use lock::{lock, wait, wait_timeout};
pub use tasks::TaskQueue;
pub use termination::{
    DispatchId, DispatchKind, Halt, Interrupt, TermCell, TermError, TerminationCause, Terminator,
};
