use libspa as spa;
use libspa_sys as spa_sys;
use std::mem::MaybeUninit;

fn main() {
    let mut buf = Vec::with_capacity(1024);
    let mut builder = spa::pod::builder::Builder::new(&mut buf);
    unsafe {
        let mut array_frame: MaybeUninit<spa_sys::spa_pod_frame> = MaybeUninit::uninit();
        // Intentionally calling with 1 argument to see if it fails or what it expects
        builder.push_array(&mut array_frame);
        builder.add_array(std::ptr::null(), 0); 
    }
}
