use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::Mutex;

use mini_racer_sync::{Channel, Notifier, Side, lock};

use crate::control::Wire;

/// A context's notifier for hosts that wait on a file descriptor: it becomes
/// readable whenever the runner rings the Ruby side. Made on first use, again
/// after a failed attempt, and released by `dispose`.
#[derive(Default)]
pub struct NotifyFd(Mutex<Option<UnixStream>>);

struct Doorbell(UnixStream);

impl Notifier for Doorbell {
    fn notify(&self) {
        // A full buffer already holds unread rings, so a failed write loses none.
        (&self.0).write_all(&[1]).ok();
    }
}

impl NotifyFd {
    /// Discards earlier rings.
    pub fn drain(&self, channel: &Channel<Wire>) -> io::Result<()> {
        let mut slot = lock(&self.0);
        let read = Self::open(&mut slot, channel)?;
        let mut buffer = [0; 64];
        let drained = loop {
            match (&*read).read(&mut buffer) {
                Ok(0) => break Ok(()),
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => break Ok(()),
                Err(error) => break Err(error),
            }
        };
        drop(slot);
        drained
    }

    /// A descriptor of its own for the host, readable whenever this one is.
    pub fn duplicate(&self, channel: &Channel<Wire>) -> io::Result<OwnedFd> {
        let mut slot = lock(&self.0);
        let copy = Self::open(&mut slot, channel)?.try_clone();
        drop(slot);
        Ok(copy?.into())
    }

    pub fn release(&self, channel: &Channel<Wire>) {
        channel.set_notifier(Side::Ruby, None);
        lock(&self.0).take();
    }

    fn open<'s>(
        slot: &'s mut Option<UnixStream>,
        channel: &Channel<Wire>,
    ) -> io::Result<&'s UnixStream> {
        if let Some(read) = slot {
            return Ok(read);
        }
        let (read, write) = UnixStream::pair()?;
        read.set_nonblocking(true)?;
        write.set_nonblocking(true)?;
        channel.set_notifier(Side::Ruby, Some(Box::new(Doorbell(write))));
        Ok(slot.insert(read))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readable(fd: &OwnedFd) -> bool {
        let mut stream = UnixStream::from(fd.try_clone().expect("dup"));
        stream.set_nonblocking(true).expect("nonblocking");
        matches!(stream.read(&mut [0]), Ok(1))
    }

    #[test]
    fn a_ring_is_readable_on_the_duplicate_until_drained() {
        let channel = Channel::<Wire>::default();
        let notify = NotifyFd::default();
        let fd = notify.duplicate(&channel).expect("pair");
        assert!(!readable(&fd));
        channel.wake(Side::Ruby);
        channel.wake(Side::Ruby);
        assert!(readable(&fd));
        notify.drain(&channel).expect("drain");
        assert!(!readable(&fd));
    }

    #[test]
    fn release_closes_the_pair_and_a_later_wait_makes_another() {
        let channel = Channel::<Wire>::default();
        let notify = NotifyFd::default();
        let old = notify.duplicate(&channel).expect("pair");
        notify.release(&channel);
        channel.wake(Side::Ruby);
        assert!(!readable(&old));
        let new = notify.duplicate(&channel).expect("pair");
        channel.wake(Side::Ruby);
        assert!(readable(&new));
    }
}
