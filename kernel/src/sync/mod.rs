use core::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

pub mod mpsc;
pub mod relax;
pub mod rwlock;
pub mod seqlock;
pub mod spinlock;

pub unsafe trait Lock<T> {
    unsafe fn get_data_unchecked(&self) -> *mut T;

    unsafe fn unlock_from_reader(&self);
    unsafe fn unlock_from_writer(&self);
}

pub struct WriteLockGuard<'a, L, T>
where
    L: Lock<T>,
{
    lock: &'a L,
    _data: PhantomData<&'a mut T>,
}

impl<L: Lock<T>, T> Deref for WriteLockGuard<'_, L, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.get_data_unchecked() }
    }
}

impl<L: Lock<T>, T> DerefMut for WriteLockGuard<'_, L, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.get_data_unchecked() }
    }
}

impl<L: Lock<T>, T> Drop for WriteLockGuard<'_, L, T> {
    fn drop(&mut self) {
        unsafe { self.lock.unlock_from_writer() };
    }
}

pub struct ReadLockGuard<'a, L, T>
where
    L: Lock<T>,
{
    lock: &'a L,
    _data: PhantomData<&'a T>,
}

impl<L: Lock<T>, T> Deref for ReadLockGuard<'_, L, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.get_data_unchecked() }
    }
}

impl<L: Lock<T>, T> Drop for ReadLockGuard<'_, L, T> {
    fn drop(&mut self) {
        unsafe { self.lock.unlock_from_reader() };
    }
}
