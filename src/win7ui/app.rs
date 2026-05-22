use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::Gdi::HBRUSH,
    UI::WindowsAndMessaging::WNDPROC,
};

use super::{create_main_window, register_class, HotKey};

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

pub struct AppShell {
    classes: Vec<WindowClassSpec>,
    main_window: Option<MainWindowSpec>,
    hotkeys: Vec<HotKey>,
}

pub struct WindowClassSpec {
    name: String,
    proc: WNDPROC,
    background: HBRUSH,
}

pub struct MainWindowSpec {
    class_name: String,
    title: String,
    width: i32,
    height: i32,
}

pub struct AppShellStart {
    pub hwnd: HWND,
    pub failed_hotkeys: Vec<HotKey>,
}

impl AppShell {
    pub fn new() -> Self {
        Self {
            classes: Vec::new(),
            main_window: None,
            hotkeys: Vec::new(),
        }
    }

    pub fn class(mut self, name: impl Into<String>, proc: WNDPROC, background: HBRUSH) -> Self {
        self.classes.push(WindowClassSpec {
            name: name.into(),
            proc,
            background,
        });
        self
    }

    pub fn main_window(
        mut self,
        class_name: impl Into<String>,
        title: impl Into<String>,
        width: i32,
        height: i32,
    ) -> Self {
        self.main_window = Some(MainWindowSpec {
            class_name: class_name.into(),
            title: title.into(),
            width,
            height,
        });
        self
    }

    pub fn hotkey(mut self, hotkey: HotKey) -> Self {
        self.hotkeys.push(hotkey);
        self
    }

    pub unsafe fn start_with_store<T: Default>(self, store: &AppStore<T>) -> Option<AppShellStart> {
        for class in &self.classes {
            register_class(&class.name, class.proc, class.background);
        }

        store.init_default();

        let main = self.main_window?;
        let hwnd = create_main_window(&main.class_name, &main.title, main.width, main.height);
        if hwnd.is_null() {
            return None;
        }

        let mut failed_hotkeys = Vec::new();
        for hotkey in self.hotkeys {
            if !hotkey.register(hwnd) {
                failed_hotkeys.push(hotkey);
            }
        }

        Some(AppShellStart {
            hwnd,
            failed_hotkeys,
        })
    }
}

impl Default for AppShell {
    fn default() -> Self {
        Self::new()
    }
}
