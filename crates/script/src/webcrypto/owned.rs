use std::ptr::NonNull;

pub(super) struct Owned<T> {
    ptr: NonNull<T>,
    free: unsafe extern "C" fn(*mut T),
}
impl<T> Owned<T> {
    pub(super) fn new(ptr: *mut T, free: unsafe extern "C" fn(*mut T)) -> Option<Self> {
        Some(Self {
            ptr: NonNull::new(ptr)?,
            free,
        })
    }
    pub(super) fn get(&self) -> *mut T {
        self.ptr.as_ptr()
    }
    pub(super) fn transfer(self) {
        std::mem::forget(self);
    }
}
impl<T> Drop for Owned<T> {
    fn drop(&mut self) {
        // Every constructor supplies the matching destructor for this owned reference.
        unsafe { (self.free)(self.get()) }
    }
}
