use bindgen;
use std::sync::atomic::fence;
use std::sync::atomic::Ordering;

// synchronize_rcu is always safe, it just needs a wrapper because it is
// implemented in C.

pub fn synchronize_rcu() {
    unsafe {
        bindgen::rcu::synchronize_rcu();
    }
}

// A wrapper around call_rcu that drops a Rust object

#[repr(C)]
struct RCUDropCB<T> {
    h: bindgen::rcu::RCUCB,
    o: Box<T>
}

extern "C" fn do_drop<T>(head: *mut bindgen::rcu::RCUCB)
{
    unsafe {
        let arg = Box::from_raw(head as *mut RCUDropCB<T>);
        // dropping arg also drops arg.o
        drop(arg);
    }
}

pub fn drop_rcu<T>(obj: Box<T>) where T: Send + 'static {
    let cb = Box::into_raw(Box::new(RCUDropCB::<T> {
        h: Default::default(),
        o: obj
    }));
    unsafe {
        bindgen::rcu::call_rcu1(&mut (*cb).h,
                                do_drop::<T>);
    }
}

// Because Rust cannot access __thread variables, we use a C function to
// retrieve it.  Reader is just a type-safe wrapper around a pointer to
// RCUReaderData.  It is always safe to dereference the pointer within the
// thread that received it, because Reader (just like pointer it contains)
// is neither Sync nor Send and the lifetime of the pointer is effectively
// 'static within the thread that received it.

pub struct Reader {
    data: *mut bindgen::rcu::RCUReaderData,
}

impl Reader {
    pub fn get() -> Reader {
        Reader {
            data: unsafe { bindgen::rcu::get_thread_reader() }
        }
    }

    pub fn read_lock(&self) -> Guard {
        unsafe {
            let data = &mut *self.data;
            data.depth += 1;
            if data.depth == 1 {
                let ctr = bindgen::rcu::rcu_gp_ctr.load(Ordering::Relaxed);
                data.ctr.store(ctr, Ordering::Relaxed);
                // FIXME: use membarrier::light() from the membarrier crate
                // if compiled with membarrier support
                fence(Ordering::SeqCst);
            }
        }
        return Guard { reader: self }
    }

    // unsafe because read_unlock should only happen when a guard's lifetime
    // ends, and with it the lifetimes of the references obtained through
    // the guard
    pub unsafe fn read_unlock(&self) {
        let data = &mut *self.data;
        if data.depth == 1 {
            data.ctr.store(0, Ordering::Relaxed);
            // FIXME: use membarrier::light() from the membarrier crate
            // if compiled with membarrier support
            fence(Ordering::SeqCst);
            if data.waiting.load(Ordering::Relaxed) {
                data.waiting.store(false, Ordering::Relaxed);
                bindgen::event::qemu_event_set(&bindgen::rcu::rcu_gp_event);
            }
        }
        data.depth -= 1;
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
    use std::time::Duration;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn empty_section_and_synchronize() {
        let r1 = Reader::get();
        let g = r1.read_lock();
        drop(g);
        synchronize_rcu();
    }

    #[test]
    fn nested_section_and_synchronize() {
        let r1 = Reader::get();
        let g1 = r1.read_lock();
        let g2 = r1.read_lock();
        drop(g1);
        drop(g2);
        synchronize_rcu();
    }

    #[test]
    fn drop_rcu_ordering() {
        let reader = Reader::get();
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
