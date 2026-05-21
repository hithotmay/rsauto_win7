use windows_sys::Win32::{
    Foundation::HWND,
    UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT},
};

#[derive(Debug, Clone, Copy)]
pub struct HotKey {
    pub id: i32,
    pub modifiers: u32,
    pub vk: u32,
}

impl HotKey {
    pub const fn new(id: i32, vk: u32) -> Self {
        Self {
            id,
            modifiers: MOD_NOREPEAT,
            vk,
        }
    }

    pub unsafe fn register(self, hwnd: HWND) -> bool {
        RegisterHotKey(hwnd, self.id, self.modifiers, self.vk) != 0
    }

    pub unsafe fn unregister(self, hwnd: HWND) {
        UnregisterHotKey(hwnd, self.id);
    }
}
