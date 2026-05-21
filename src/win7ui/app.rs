use std::sync::{Mutex, OnceLock};

pub struct AppStore<T> {
    inner: OnceLock<Mutex<T>>,
}

impl<T> AppStore<T> {
    pub const fn new() -> Self {
        Self {
            inner: OnceLock::new(),
        }
    }

    pub fn set(&self, value: T) -> Result<(), T> {
        self.inner.set(Mutex::new(value)).map_err(|mutex| {
            mutex
                .into_inner()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        })
    }

    pub fn init_default(&self)
    where
        T: Default,
    {
        let _ = self.set(T::default());
    }

    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        let lock = self.inner.get()?;
        let guard = lock.lock().unwrap();
        Some(f(&guard))
    }

    pub fn with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let lock = self.inner.get()?;
        let mut guard = lock.lock().unwrap();
        Some(f(&mut guard))
    }

    pub fn get(&self) -> Option<&Mutex<T>> {
        self.inner.get()
    }
}

impl<T> Default for AppStore<T> {
    fn default() -> Self {
        Self::new()
    }
}
