#![allow(improper_ctypes)]

use std::sync::atomic::AtomicUsize;
use std::sync::atomic::AtomicBool;

use bindgen::event;

#[repr(C)]
#[derive(Debug)]
pub struct RCUReaderData {
    pub ctr: AtomicUsize,
    pub waiting: AtomicBool,
    pub depth: ::std::os::raw::c_uint,
    _next: *mut RCUReaderData,
    _pprev: *mut *mut RCUReaderData,
}

pub type RCUCBFunc = unsafe extern "C" fn (head: *mut RCUCB);

#[repr(C)]
#[derive(Debug)]
pub struct RCUCB {
    pub next: *mut RCUCB,
    pub func: Option<RCUCBFunc>
}

impl Default for RCUCB {
    fn default() -> RCUCB {
        RCUCB {
            next: std::ptr::null_mut(),
            func: None
        }
    }
}

extern "C" {
    pub static rcu_gp_ctr : AtomicUsize;
    pub static rcu_gp_event : event::QemuEvent;
    pub fn get_thread_reader() -> *mut RCUReaderData;
    pub fn synchronize_rcu();
    pub fn call_rcu1(head : *mut RCUCB, func : RCUCBFunc);
}
