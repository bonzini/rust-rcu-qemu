use census::{Inventory, TrackedObject};

use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, AtomicPtr, Ordering};
use std::sync::atomic::fence;
use std::sync::Mutex;
use std::sync::{Once, ONCE_INIT};
use std::sync::mpsc;
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

trait FnBox {
    fn call_box(self: Box<Self>);
}

impl <F: FnOnce()> FnBox for F {
    fn call_box(self: Box<F>) {
        (*self)()
    }
}

type Destructor = Box<FnBox + Send>;

struct RCU {
    // Rust's mpsc::Sender design is not optimal here, because
    // while we want to send from multiple threads, we do not
    // really know from which threads until they call call_rcu,
    // so we cannot clone the sender in advance.  Oh well, this is a
    // toy so just wrap the sender with a mutex.
    tx: Mutex<mpsc::Sender<Destructor>>,
    rcu_readers: Inventory<Reader>,
    rcu_syncing: Mutex<()>,
}

const CALL_RCU_BATCH_SIZE: usize = 16;
const CALL_RCU_TRIES: usize = 3;

const MS: Duration = Duration::from_millis(1);

fn call_rcu_thread(rx: mpsc::Receiver<Destructor>) {
    type RCUBatch = [Destructor; CALL_RCU_BATCH_SIZE];
    let mut callbacks = arrayvec::ArrayVec::<RCUBatch>::new();
    while let Ok(f) = rx.recv() {
        assert_eq!(callbacks.len(), 0);
        callbacks.push(f);
        let mut try = 0;
        while !callbacks.is_full() && try < CALL_RCU_TRIES {
            match rx.try_recv() {
                Ok(f) => {
                    callbacks.push(f);
                },
                Err(mpsc::TryRecvError::Empty) => {
                    thread::sleep(MS * 200);
                    try += 1;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    break;
                }
            }
        }
        let n = callbacks.len();
        synchronize_rcu();
        for i in callbacks.drain(0..n) {
            i.call_box();
        }
    }
}

impl RCU {
    fn new() -> RCU {
        let (tx, rx) = channel::<Destructor>();
        let result = RCU {
            tx: Mutex::new(tx),
            rcu_readers: Inventory::new(),
            rcu_syncing: Mutex::new(()),
        };
        thread::spawn(move || { call_rcu_thread(rx); });
        result
    }

    fn track(&self, r: Reader) -> TrackedObject<Reader> {
        self.rcu_readers.track(r)
    }

    fn get_readers(&self) -> Vec<TrackedObject<Reader>> {
        self.rcu_readers.list()
    }

    fn remove_idle_readers(&self, active: &mut Vec<TrackedObject<Reader>>, gp_ctr: usize) {
        let mut i = 0;
        let mut len = active.len();
        while i < len {
            {
                let r = &active[i];
                if !r.is_idle(gp_ctr) {
                    i += 1;
                    continue;
                }
                r.waiting.store(null_mut(), Ordering::Relaxed);
            }
            active.swap_remove(i);
            len -= 1;
        }
        assert_eq!(len, active.len());
    }

    fn wait_for_readers(&self, gp_ctr: usize) {
        let mut thread = thread::current();

        GP_CTR.store(gp_ctr, Ordering::Relaxed);

        // Store GP_CTR before loading the readers' counters
        fence(Ordering::SeqCst);

        let mut active = self.get_readers();

        for r in active.iter() {
            r.waiting.store(&mut thread, Ordering::Relaxed);
        }

        // Store waiting before loading the readers' counters
        fence(Ordering::SeqCst);

        self.remove_idle_readers(&mut active, gp_ctr);
        while active.len() > 0 {
            thread::park();
            self.remove_idle_readers(&mut active, gp_ctr);
        }
    }

    fn synchronize_rcu(&self) {
        // Do not allow concurrent calls to synchronize_rcu
        let guard = self.rcu_syncing.lock().unwrap();
        self.wait_for_readers(1);
        self.wait_for_readers(0);
        drop(guard);
    }

    fn call_rcu<F>(&self, destructor: Box<F>) where F: FnOnce() -> () + Send + 'static {
        let tx = self.tx.lock().unwrap();
        tx.send(destructor).unwrap();
    }

    fn drop_rcu<T>(&self, obj: Box<T>) where T: Send + 'static {
        let destructor = Box::new(move || {
            std::mem::drop(obj);
        });
        self.call_rcu(destructor);
    }

}

static START: Once = ONCE_INIT;
static mut THE_RCU: Option<RCU> = None;
static GP_CTR: AtomicUsize = AtomicUsize::new(0);

fn rcu_get() -> &'static RCU {
    unsafe { 
        START.call_once(|| {
            THE_RCU = Some(RCU::new());
        });
        THE_RCU.as_ref().unwrap()
    }
}

pub fn synchronize_rcu() {
    rcu_get().synchronize_rcu();
}

pub fn call_rcu<F>(destructor: Box<F>) where F: FnOnce() -> () + Send + 'static {
    rcu_get().call_rcu(destructor);
}

pub fn drop_rcu<T>(obj: Box<T>) where T: Send + 'static {
    rcu_get().drop_rcu(obj);
}

pub struct Reader {
    ctr: AtomicUsize,
    waiting: AtomicPtr<thread::Thread>,
}

impl Reader {
    pub fn new() -> TrackedObject<Reader> {
        let result = Reader {
            ctr: AtomicUsize::new(0),
            waiting: AtomicPtr::new(null_mut()),
        };
        rcu_get().track(result)
    }

    fn is_idle(&self, gp_ctr: usize) -> bool {
        let ctr = self.ctr.load(Ordering::Relaxed);
        ctr == 0 || (ctr & 1) != gp_ctr
    }

    pub fn read_lock(&self) -> Guard {
        loop {
            let old_ctr = self.ctr.load(Ordering::Relaxed);
            let new_ctr = if old_ctr == 0 {
                2 | GP_CTR.load(Ordering::Relaxed)
            } else {
                old_ctr + 2
            };
            if self.ctr.compare_and_swap(old_ctr, new_ctr,
                                         Ordering::SeqCst) == old_ctr {
                break
            }
        }
        return Guard { reader: self }
    }

    pub unsafe fn read_unlock(&self) {
        loop {
            let old_ctr = self.ctr.load(Ordering::Relaxed);
            assert!(old_ctr >= 2);
            let new_ctr = if old_ctr >= 4 { old_ctr - 2 } else { 0 };
            if self.ctr.compare_and_swap(old_ctr, new_ctr,
                                         Ordering::SeqCst) == old_ctr {
                if new_ctr >= 2 {
                    return
                }
                break
            }
        }
        let ptr = self.waiting.load(Ordering::Relaxed);
        if !ptr.is_null() {
            let writer = &*ptr;
            writer.unpark();
        }
    }
}

pub struct Guard<'r> {
    reader: &'r Reader,
}

impl<'r> Drop for Guard<'r> {
    fn drop(&mut self) {
        unsafe {
            self.reader.read_unlock();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tests::RCUTest;

    #[test]
    fn empty_section_and_synchronize() {
        let r1 = Reader::new();
        let g = r1.read_lock();
        drop(g);
        synchronize_rcu();
    }

    #[test]
    fn nested_section_and_synchronize() {
        let r1 = Reader::new();
        let g1 = r1.read_lock();
        let g2 = r1.read_lock();
        drop(g1);
        drop(g2);
        synchronize_rcu();
    }

    #[test]
    fn call_rcu_ordering() {
        let reader = Reader::new();
        let (object, check) = RCUTest::pair();
        {
            // Set up a callback inside an RCU critical section
            let g = reader.read_lock();
            drop_rcu(object);

            std::thread::sleep(MS * 1000);
            check.rcu_done(g);
        }

        // Wait for the invocation of the callback.
        assert!(check.wait());
    }
}
