#![no_core]
#![feature(no_core, lang_items)]

#[lang = "pointee_sized"]
pub trait PointeeSized {}

#[lang = "meta_sized"]
pub trait MetaSized: PointeeSized {}

#[lang = "sized"]
pub trait Sized: MetaSized {}

#[lang = "copy"]
pub trait Copy: Sized {}

#[no_mangle]
pub extern "C" fn load(p: *const u64) -> u64 {
    unsafe { *p }
}

#[no_mangle]
pub extern "C" fn store(p: *mut u32, v: u32) {
    unsafe {
        *p = v;
    }
}
