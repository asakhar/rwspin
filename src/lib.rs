#![cfg_attr(not(feature = "std"), no_std)]
#![feature(const_try)]

use core::cell::UnsafeCell;
use core::convert::From;
use core::default::Default;
use core::ops::{Deref, DerefMut, Drop};
use core::sync::atomic::{AtomicUsize, Ordering};

use core::{
  marker::{Send, Sync},
  option::Option::{self, None, Some},
};

use core::clone::Clone;

use core::fmt::Debug;
use core::result::Result::{self, Err, Ok};

pub struct RwSpin<T> {
  data: UnsafeCell<T>,
  rc: AtomicUsize,
}

use core::format_args;

impl<T: Debug> Debug for RwSpin<T> {
  fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
    match self.try_read() {
      Some(data) => f.write_fmt(format_args!("RwSpin({:?})", &*data)),
      None => f.write_str("RwLock(<Failed to lock>)"),
    }
  }
}

unsafe impl<T: Send + Sync> Sync for RwSpin<T> {}
unsafe impl<T: Send> Send for RwSpin<T> {}

impl<T> RwSpin<T> {
  pub const fn new(data: T) -> Self {
    Self {
      data: UnsafeCell::new(data),
      rc: AtomicUsize::new(0),
    }
  }

  pub fn read(&self) -> RwSpinReadGuard<T> {
    RwSpinReadGuard::new(self)
  }

  pub fn try_read(&self) -> Option<RwSpinReadGuard<T>> {
    RwSpinReadGuard::try_new(self)
  }

  pub fn write(&self) -> RwSpinWriteGuard<T> {
    RwSpinWriteGuard::new(self)
  }

  pub fn try_write(&self) -> Option<RwSpinWriteGuard<T>> {
    RwSpinWriteGuard::try_new(self)
  }

  pub fn share_count(&self) -> Option<usize> {
    let rc = self.rc.load(Ordering::Relaxed);
    if rc == usize::MAX {
      None
    } else {
      Some(rc)
    }
  }

  pub fn is_being_written(&self) -> bool {
    self.rc.load(Ordering::Relaxed) == usize::MAX
  }

  fn load_checked_inc_swap(&self) -> Option<()> {
    let rc = self.rc.load(Ordering::SeqCst);
    let _ = self
      .rc
      .compare_exchange(rc, rc.checked_add(1)?, Ordering::SeqCst, Ordering::Relaxed)
      .ok()?;
    Some(())
  }

  fn load_checked_inc_swap_weak(&self) -> Option<()> {
    let rc = self.rc.load(Ordering::SeqCst);
    let _ = self
      .rc
      .compare_exchange_weak(rc, rc.checked_add(1)?, Ordering::SeqCst, Ordering::Relaxed)
      .ok()?;
    Some(())
  }

  fn swap_exclusive(&self, rc: usize) -> Option<()> {
    let _ = self
      .rc
      .compare_exchange(rc, usize::MAX, Ordering::SeqCst, Ordering::Relaxed)
      .ok()?;
    Some(())
  }

  fn swap_exclusive_weak(&self, rc: usize) -> Option<()> {
    let _ = self
      .rc
      .compare_exchange_weak(rc, usize::MAX, Ordering::SeqCst, Ordering::Relaxed)
      .ok()?;
    Some(())
  }
}

#[derive(Debug)]
pub struct RwSpinReadGuard<'spin, T> {
  spin: &'spin RwSpin<T>,
}

impl<'spin, T> RwSpinReadGuard<'spin, T> {
  fn new(spin: &'spin RwSpin<T>) -> Self {
    while spin.load_checked_inc_swap_weak().is_none() {}
    Self { spin }
  }
  fn try_new(spin: &'spin RwSpin<T>) -> Option<Self> {
    spin.load_checked_inc_swap()?;
    Some(Self { spin })
  }
  pub fn into_write(self) -> RwSpinWriteGuard<'spin, T> {
    let spin = self.spin;
    core::mem::forget(self);
    while spin.swap_exclusive_weak(1).is_none() {}
    RwSpinWriteGuard { spin }
  }
  pub fn try_into_write(self) -> Result<RwSpinWriteGuard<'spin, T>, Self> {
    let spin = self.spin;
    match spin.swap_exclusive(1) {
      Some(()) => {
        core::mem::forget(self);
        Ok(RwSpinWriteGuard { spin })
      }
      None => Err(self),
    }
  }
}

impl<'spin, T> Deref for RwSpinReadGuard<'spin, T> {
  type Target = T;

  fn deref(&self) -> &Self::Target {
    // Safety: data is not borrowed exclusively
    unsafe { &*self.spin.data.get() }
  }
}

impl<'spin, T> Drop for RwSpinReadGuard<'spin, T> {
  fn drop(&mut self) {
    self.spin.rc.fetch_sub(1, Ordering::SeqCst);
  }
}

impl<'spin, T> Clone for RwSpinReadGuard<'spin, T> {
  fn clone(&self) -> Self {
    while self.spin.load_checked_inc_swap().is_none() {}
    Self {
      spin: self.spin.clone(),
    }
  }
}

#[derive(Debug)]
pub struct RwSpinWriteGuard<'spin, T> {
  spin: &'spin RwSpin<T>,
}

impl<'spin, T> RwSpinWriteGuard<'spin, T> {
  fn new(spin: &'spin RwSpin<T>) -> Self {
    while spin.swap_exclusive_weak(0).is_none() {}
    Self { spin }
  }
  fn try_new(spin: &'spin RwSpin<T>) -> Option<Self> {
    spin.swap_exclusive(0)?;
    Some(Self { spin })
  }
}

impl<'spin, T> Deref for RwSpinWriteGuard<'spin, T> {
  type Target = T;

  fn deref(&self) -> &Self::Target {
    // Safety: data is not borrowed yet
    unsafe { &*self.spin.data.get() }
  }
}

impl<'spin, T> DerefMut for RwSpinWriteGuard<'spin, T> {
  fn deref_mut(&mut self) -> &mut Self::Target {
    // Safety: data is not borrowed yet
    unsafe { &mut *self.spin.data.get() }
  }
}

impl<'spin, T> Drop for RwSpinWriteGuard<'spin, T> {
  fn drop(&mut self) {
    self.spin.rc.store(0, Ordering::SeqCst);
  }
}

impl<T: Default> Default for RwSpin<T> {
  fn default() -> Self {
    Self {
      data: Default::default(),
      rc: AtomicUsize::new(0),
    }
  }
}

impl<'spin, T> From<RwSpinWriteGuard<'spin, T>> for RwSpinReadGuard<'spin, T> {
  fn from(write: RwSpinWriteGuard<'spin, T>) -> Self {
    let spin = write.spin;
    core::mem::forget(write);
    spin.rc.store(1, Ordering::SeqCst);
    Self { spin }
  }
}
