#[repr(C)]
#[derive(Debug)]
pub struct QemuEvent { _unused:[u8;0], }

extern "C" {
    pub fn qemu_event_set(ev: *const QemuEvent);
}

