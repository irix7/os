// SPDX-License-Identifier: GPL-3.0-only
//! Single-threaded pthread and synchronization stubs.
//!
//! Rust `std` uses pthread mutexes, condition variables and keys even in a
//! single-threaded hello.  These implementations preserve the ABI and the
//! single-threaded semantics; real threads land with the native scheduler.

type KeyDestructor = Option<unsafe extern "C" fn(*mut u8)>;

const KEY_COUNT: usize = 64;

static mut KEY_VALUES: [*mut u8; KEY_COUNT] = [core::ptr::null_mut(); KEY_COUNT];
static mut KEY_DESTRUCTORS: [KeyDestructor; KEY_COUNT] = [None; KEY_COUNT];
static mut KEY_USED: [bool; KEY_COUNT] = [false; KEY_COUNT];

#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_init(_mutex: *mut u8, _attr: *const u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_destroy(_mutex: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_lock(_mutex: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_trylock(_mutex: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_unlock(_mutex: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_init(_attr: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_destroy(_attr: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_settype(_attr: *mut u8, _kind: i32) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_condattr_init(_attr: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_condattr_destroy(_attr: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_condattr_setclock(_attr: *mut u8, _clock: i32) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_init(_cond: *mut u8, _attr: *const u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_destroy(_cond: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_signal(_cond: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_wait(_cond: *mut u8, _mutex: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_timedwait(
    _cond: *mut u8,
    _mutex: *mut u8,
    _timeout: *const u8,
) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_key_create(key: *mut u32, destructor: KeyDestructor) -> i32 {
    let mut slot = 0;
    while slot < KEY_COUNT {
        if !KEY_USED[slot] {
            *key = slot as u32;
            KEY_USED[slot] = true;
            KEY_DESTRUCTORS[slot] = destructor;
            return 0;
        }
        slot += 1;
    }
    11
}

#[no_mangle]
pub unsafe extern "C" fn pthread_key_delete(key: u32) -> i32 {
    if (key as usize) < KEY_COUNT {
        KEY_USED[key as usize] = false;
        KEY_DESTRUCTORS[key as usize] = None;
        KEY_VALUES[key as usize] = core::ptr::null_mut();
        0
    } else {
        22
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_getspecific(key: u32) -> *mut u8 {
    if (key as usize) < KEY_COUNT {
        KEY_VALUES[key as usize]
    } else {
        core::ptr::null_mut()
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_setspecific(key: u32, value: *const u8) -> i32 {
    if (key as usize) < KEY_COUNT {
        KEY_VALUES[key as usize] = value as *mut u8;
        0
    } else {
        22
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_self() -> usize {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_equal(left: usize, right: usize) -> i32 {
    (left == right) as i32
}

#[no_mangle]
pub unsafe extern "C" fn pthread_setname_np(_thread: usize, _name: *const u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_detach(_thread: usize) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn sched_yield() -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn sysconf(name: i32) -> i64 {
    match name {
        30 => 4096, // _SC_PAGESIZE
        84 => 1,    // _SC_NPROCESSORS_ONLN
        _ => -1,
    }
}
