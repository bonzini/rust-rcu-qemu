use std::mem;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, Ordering};

use period::Guard;
use period::drop_rcu;
use period::synchronize_rcu;

// An rcu::Cell is a nullable pointer.  In order to satisfy RCU
// properties, reads produce a reference whose lifetime is
// confined to the validity of a rcu::Guard.  Stores instead take
// ownership of the value, and return the previous value
// wrapped with an rcu::Unlinked smart pointer.  The purpose of
// rcu::Unlinked is to ensure that the value survives until the next
// grace period, even if the program drops the smart pointer earlier
// than that (as is the case when the caller does not need the
// previous value).

#[derive(Default)]
pub struct Cell<T: Send + 'static> {
    data: AtomicPtr<T>
}

impl<T: Send> Cell<T> {
    fn into_raw(data: Option<Box<T>>) -> *mut T
    {
        match data {
            None => null_mut(),
            Some(x) => Box::into_raw(x)
        }
    }

    pub fn new(data: Option<Box<T>>) -> Cell<T> {
        Cell {
            data: AtomicPtr::new(Cell::into_raw(data))
        }
    }

    pub fn as_raw(&self) -> *mut T {
        self.data.load(Ordering::Acquire)
    }

    pub unsafe fn as_unguarded_ref(&self) -> Option<&T> {
        self.as_raw().as_ref()
    }

    pub unsafe fn as_unguarded_mut(&self) -> Option<&mut T> {
        self.as_raw().as_mut()
    }

    pub fn is_none(&self) -> bool {
        self.data.load(Ordering::Relaxed).is_null()
    }

    pub fn is_some(&self) -> bool {
        !self.data.load(Ordering::Relaxed).is_null()
    }

    pub fn get<'g>(&self, _: &'g Guard) -> Option<&'g T> {
        unsafe {
            self.as_raw().as_ref()
        }
    }

    pub fn replace(&mut self, src: Option<Box<T>>) -> Unlinked<T> {
        let old = self.data.load(Ordering::Relaxed);
        self.data.store(Cell::into_raw(src), Ordering::Release);
        unsafe {
            Unlinked::new(old)
        }
    }

    pub fn clear(&mut self) -> Unlinked<T> {
        self.replace(None)
    }

    pub fn set(&mut self, src: Box<T>) -> Unlinked<T> {
        self.replace(Some(src))
    }
}

impl<T: Send> Drop for Cell<T> {
    fn drop(&mut self) {
        let old = self.data.load(Ordering::Relaxed);
        if !old.is_null() {
            unsafe {
                drop(Unlinked::new(old))
            }
        }
    }
}

pub struct Unlinked<T: Send + 'static> {
    data: *mut T
}

impl<T: Send> Unlinked<T> {
    unsafe fn new(data: *mut T) -> Unlinked<T> {
        Unlinked { data }
    }

    pub unsafe fn as_ref(&self) -> Option<&T> {
        self.data.as_ref()
    }

    pub unsafe fn as_mut(&mut self) -> Option<&mut T> {
        self.data.as_mut()
    }

    pub fn is_none(&self) -> bool {
        self.data.is_null()
    }

    pub fn is_some(&self) -> bool {
        !self.data.is_null()
    }

    pub unsafe fn into_box(self) -> Option<Box<T>> {
        let data = (&self).data;
        // Ownership is passed to the box we return, which has to
        // survive until after the next RCU grace period.  But,
        // it may also die before, which is why this function is
        // unsafe.
        mem::forget(self);
        if data.is_null() {
            None
        } else {
            Some(Box::from_raw(data))
        }
    }

    pub fn synchronize_rcu(self) -> Option<Box<T>> {
        unsafe {
            let data = self.into_box();
            if let Some(_) = data {
                synchronize_rcu();
            }
            // This Box can now be passed back (and possibly dropped)
            // safely, because we guarantee that all pending readers
            // have completed.
            data
        }
    }
}

impl<T: Send> PartialEq<Unlinked<T>> for Unlinked<T> {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

impl<T: Send> Eq for Unlinked<T> {}

impl<T: Send> Drop for Unlinked<T> {
    fn drop(&mut self) {
        if !self.data.is_null() {
            unsafe {
                drop_rcu(Box::from_raw(self.data));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use period::*;
    use tests::RCUTest;
    use std::time::Duration;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn synchronize_rcu() {
        let mut cell = Cell::<i32>::new(None);
        assert!(cell.is_none());

        let owned = Box::new(1);
        let unlinked = cell.set(owned);
        assert!(unlinked.is_none());
        assert!(cell.is_some());

        let unlinked = cell.clear();
        assert!(unlinked.is_some());
        assert!(cell.is_none());

        let owned = unlinked.synchronize_rcu().unwrap();
        assert_eq!(*owned, 1);
    }

    #[test]
    fn call_rcu_ordering() {
        let reader = Reader::new();
        let (object, check) = RCUTest::pair();
        let mut cell = Cell::<RCUTest>::new(Some(object));

        {
            // Modify the cell while inside an RCU critical section
            let g = reader.read_lock();
            {
                let shared = cell.get(&g);
                drop(shared);
            }

            // The old value is returned by clear() and dropped
            // immediately.
            cell.clear();

            std::thread::sleep(MS * 1000);
            check.rcu_done(g);
        }

        // Wait for the invocation of the callback.
        assert!(check.wait());
    }
}
