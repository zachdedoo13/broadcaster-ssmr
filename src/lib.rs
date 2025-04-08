//! A Broadcaster (ssmr = single sender multi receiver) is a channel where one sender sends duplicate data to multiple receiver's.
//! the data is buffered (as specified by Broadcaster::subscribe(buffer_size: usize)) at the receiver and can be pulled with 'recv' or 'try_recv'.
//!

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

pub struct Broadcaster<T: Clone> {
    children: Mutex<Vec<Weak<Receiver<T>>>>, // inside a mutex to it can be subscribed to from multiple threads 'concurrently'
}
impl<T: Clone> Broadcaster<T> {
    pub const fn new() -> Self {
        Self {
            children: Mutex::new(vec![]),
        }
    }

    pub fn subscribe(&self, buffer_size: usize) -> Arc<Receiver<T>> {
        let mut c = self.children.lock().unwrap();
        let receiver = Arc::new(Receiver::new(buffer_size));
        c.push(Arc::downgrade(&receiver));
        receiver
    }

    /// alias for self.subscribe(usize::MAX)
    pub fn subscribe_unbound(&self) {
        self.subscribe(usize::MAX);
    }

    /// returns the amount of receivers the data was sent to
    pub fn send(&self, data: T) -> usize {
        let mut c = self.children.lock().unwrap();
        c.retain(|r| {
            if let Some(r) = r.upgrade() {
                r.push(data.clone());
                true
            } else {
                false
            }
        });

        c.len()
    }

    /// if it timeouts the data is never sent to select receiver
    /// returns the amount of receivers the data was sent to
    pub fn send_timeout(&self, data: T, timeout: Duration) -> Result<usize, &'static str> {
        let mut c = self.children.lock().unwrap();
        let mut res = Ok(());
        c.retain(|r| {
            if let Some(r) = r.upgrade() {
                if let Err(e) = r.push_timeout(data.clone(), timeout) {
                    res = Err(e);
                }
                true
            } else {
                false
            }
        });

        match res {
            Ok(_) => {
                Ok(c.len())
            }
            Err(e) => {
                Err(e)
            }
        }
    }
}
impl<T: Clone> Drop for Broadcaster<T> {
    fn drop(&mut self) {
        let children = self.children.lock().unwrap();
        for c in children.iter() {
            if let Some(r) = c.upgrade() {
                r.sender_alive.store(false, Ordering::SeqCst);
            }
        }
    }
}

pub struct Receiver<T> {
    buffer: Mutex<VecDeque<T>>,
    condvar: Condvar,
    space_available: Condvar,
    buffer_size: usize,
    sender_alive: AtomicBool,
}
impl<T> Receiver<T> {
    fn new(size: usize) -> Self {
        Self {
            buffer: Mutex::new(VecDeque::new()),
            condvar: Condvar::new(),
            space_available: Condvar::new(),
            buffer_size: size,
            sender_alive: AtomicBool::new(true),
        }
    }

    pub(crate) fn push(&self, val: T) {
        let mut buffer = self.buffer.lock().unwrap();
        while buffer.len() >= self.buffer_size {
            buffer = self.space_available.wait(buffer).unwrap();
        }
        buffer.push_back(val);
        self.condvar.notify_one();
    }

    pub(crate) fn push_timeout(&self, val: T, timeout: Duration) -> Result<(), &'static str> {
        let buffer = self.buffer.lock().unwrap();
        let (mut buffer, res) = self
            .space_available
            .wait_timeout_while(buffer, timeout, |buffer| buffer.len() >= self.buffer_size)
            .unwrap();

        if res.timed_out() {
            return Err("Timeout");
        }

        buffer.push_back(val);
        self.condvar.notify_one();

        Ok(())
    }

    /// Blocking
    pub fn recv(&self) -> T {
        let mut buffer = self.buffer.lock().unwrap();
        while buffer.is_empty() {
            buffer = self.condvar.wait(buffer).unwrap();
        }
        let val = buffer.pop_front().unwrap();
        self.space_available.notify_one();
        val
    }

    /// Blocking
    pub fn recv_all(&self) -> Vec<T> {
        let mut buffer = self.buffer.lock().unwrap();
        while buffer.is_empty() {
            buffer = self.condvar.wait(buffer).unwrap();
        }
        // let val = buffer.pop_front().unwrap();

        let mut out = Vec::with_capacity(buffer.len());
        for _ in 0..buffer.len() {
            out.push(buffer.pop_front().expect("This should not happen"))
        }
        self.space_available.notify_one();
        out
    }

    pub fn try_recv(&self) -> Option<T> {
        let mut buffer = self.buffer.lock().unwrap();
        while buffer.is_empty() {
            return None;
        }

        let out = buffer.pop_front().expect("wont happen");
        self.space_available.notify_one();
        Some(out)
    }

    pub fn try_recv_all(&self) -> Option<Vec<T>> {
        let mut buffer = self.buffer.lock().unwrap();
        while buffer.is_empty() {
            // buffer = self.condvar.wait(buffer).unwrap();
            return None;
        }
        // let val = buffer.pop_front().unwrap();

        let mut out = Vec::with_capacity(buffer.len());
        for _ in 0..buffer.len() {
            out.push(buffer.pop_front().expect("This should not happen"))
        }
        self.space_available.notify_one();
        Some(out)
    }

    pub fn sender_alive(&self) -> bool {
        self.sender_alive.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::{sleep, spawn};

    #[test]
    fn easy() {
        let broadcaster = Broadcaster::new();
        let mut seen = vec![];
        for _ in 0..3 {
            let rv = broadcaster.subscribe(5);
            let v = spawn(move || {
                assert_eq!(rv.recv(), 3);
            });
            seen.push(v);
        }

        broadcaster.send(3);

        sleep(Duration::from_millis(50));
        for s in seen.iter() {
            if !s.is_finished() {
                panic!();
            }
        }
    }

    #[test]
    fn timeout() {
        let sender = Broadcaster::new();

        let r1 = sender.subscribe(1);
        let one = spawn(move || {
            // sleep(Duration::from_millis(500));
            assert_eq!(r1.recv(), 3);
            assert_eq!(r1.recv(), 3);
        });

        let r2 = sender.subscribe(1);
        let two = spawn(move || {
            let _local = r2;
            sleep(Duration::from_millis(500));
        });

        sender.send(3);
        assert_eq!(
            Err("Timeout"),
            sender.send_timeout(3, Duration::from_millis(200))
        );

        one.join().unwrap();
        two.join().unwrap();
    }

    #[test]
    fn clearing() {
        let broadcaster = Broadcaster::new();
        let mut seen = vec![];
        for _ in 0..5 {
            let rv = broadcaster.subscribe(3);
            let s = spawn(move || {
                for i in 0..10 {
                    assert_eq!(rv.recv(), i);
                }
            });
            seen.push(s);
        }

        for i in 0..10 {
            broadcaster
                .send_timeout(i, Duration::from_millis(50))
                .unwrap();
        }

        for s in seen {
            s.join().unwrap();
        }
    }

    #[test]
    fn removed_receivers() {
        let broadcaster = Broadcaster::new();

        let r1 = broadcaster.subscribe(1);

        let r2 = broadcaster.subscribe(1);
        drop(r2);

        broadcaster.send(3);
        r1.recv();
        broadcaster
            .send_timeout(4, Duration::from_millis(500))
            .unwrap();

        assert_eq!(r1.recv(), 4);
    }

    #[test]
    fn removed_sender() {
        let broadcaster: Broadcaster<()> = Broadcaster::new();
        let r = broadcaster.subscribe(1);
        assert_eq!(r.sender_alive(), true);
        drop(broadcaster);
        assert_eq!(r.sender_alive(), false);
    }

    #[test]
    fn stat_ic() {
        static INFO_BOT: Broadcaster<()> = Broadcaster::new();

        let one = spawn(|| {
            let r = INFO_BOT.subscribe(usize::MAX);
            r.recv()
        });

        loop {
            let v = INFO_BOT.send(());
            if v != 0 { break }
        }


        one.join().unwrap();
        // two.join().unwrap();
    }
}
