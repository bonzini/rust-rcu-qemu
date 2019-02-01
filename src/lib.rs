mod period;
pub use period::synchronize_rcu;
pub use period::drop_rcu;
pub use period::Reader;
pub use period::Guard;

mod cell;
pub use cell::Cell;
pub use cell::Unlinked;

mod bindgen;

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Condvar, Mutex};
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[derive(Clone)]
    pub struct RCUTest {
        stage: Arc<Mutex<i32>>,
        cv: Arc<Condvar>,
        success: Arc<AtomicBool>
    }

    impl RCUTest {
        pub fn pair() -> (Box<RCUTest>, RCUTest) {
            let result = RCUTest {
                stage: Arc::new(Mutex::new(0)),
                cv: Arc::new(Condvar::new()),
                success: Arc::new(AtomicBool::new(false))
            };
            let result2 = result.clone();
            (Box::new(result), result2)
        }

        pub fn rcu_done(&self, rcu_guard: Guard) {
            let mut stage_guard = self.stage.lock().unwrap();
            drop(rcu_guard);
            *stage_guard = 1;
        }

        pub fn wait(&self) -> bool {
            // Wait for the invocation of the callback.
            let mut stage_guard = self.stage.lock().unwrap();
            while *stage_guard != 2 {
                stage_guard = self.cv.wait(stage_guard).unwrap();
            }
            self.success.load(Ordering::Relaxed)
        }
    }

    impl Drop for RCUTest {
        fn drop(&mut self) {
            // This must be called after the end of the RCU period
            let mut stage_guard = self.stage.lock().unwrap();
            self.success.store(*stage_guard == 1, Ordering::Relaxed);
            *stage_guard = 2;
            self.cv.notify_one();
        }
    }
}
