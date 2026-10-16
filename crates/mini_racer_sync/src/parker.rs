use std::time::Duration;

use crate::lock::{lock, wait, wait_timeout};
use crate::primitives::{Condvar, Mutex};

// A token parker: an unpark that arrives before the park is kept, so the waker
// never has to know whether the waiter is asleep yet.
pub struct Parker {
    token: Mutex<bool>,
    wake: Condvar,
}

impl Default for Parker {
    fn default() -> Self {
        Self {
            token: Mutex::new(false),
            wake: Condvar::new(),
        }
    }
}

impl Parker {
    pub fn park(&self, timeout: Option<Duration>) {
        let mut token = lock(&self.token);
        match timeout {
            None => {
                while !*token {
                    token = wait(&self.wake, token);
                }
            }
            Some(timeout) => {
                if !*token {
                    token = wait_timeout(&self.wake, token, timeout);
                }
            }
        }
        *token = false;
    }

    pub fn unpark(&self) {
        *lock(&self.token) = true;
        self.wake.notify_one();
    }
}
