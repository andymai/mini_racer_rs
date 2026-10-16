use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::OnceLock;

use mini_racer_sync::{Channel, Notifier, Side};

use crate::control::Wire;

/// A context's notifier for hosts that wait on a file descriptor: its read end
/// becomes readable whenever the runner wakes the Ruby side. Made on first use;
/// a context that cannot make one keeps waiting the other way.
#[derive(Default)]
pub struct NotifyFd(OnceLock<Option<UnixStream>>);

struct Doorbell(UnixStream);

impl Notifier for Doorbell {
    fn notify(&self) {
        // A full buffer already holds unread wakes, so a failed write loses none.
        (&self.0).write_all(&[1]).ok();
    }
}

fn doorbell(channel: &Channel<Wire>) -> io::Result<UnixStream> {
    let (read, write) = UnixStream::pair()?;
    read.set_nonblocking(true)?;
    write.set_nonblocking(true)?;
    channel.set_notifier(Side::Ruby, Box::new(Doorbell(write)));
    Ok(read)
}

impl NotifyFd {
    /// Discards earlier wakes and returns the read end.
    pub fn drained(&self, channel: &Channel<Wire>) -> io::Result<RawFd> {
        let read = self
            .0
            .get_or_init(|| doorbell(channel).ok())
            .as_ref()
            .ok_or_else(|| io::Error::other("no notifier"))?;
        let mut buffer = [0; 64];
        loop {
            match (&*read).read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        Ok(read.as_raw_fd())
    }
}
