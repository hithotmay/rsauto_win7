use std::sync::mpsc::{self, Receiver, Sender};

use windows_sys::Win32::{Foundation::HWND, UI::WindowsAndMessaging::PostMessageW};

use super::{hwnd_value, to_hwnd, RawHwnd};

#[derive(Debug)]
pub struct UiEventSender<T> {
    tx: Sender<T>,
    hwnd: RawHwnd,
    wake_msg: u32,
}

impl<T> Clone for UiEventSender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            hwnd: self.hwnd,
            wake_msg: self.wake_msg,
        }
    }
}

impl<T> UiEventSender<T> {
    pub fn new(tx: Sender<T>, hwnd: HWND, wake_msg: u32) -> Self {
        Self {
            tx,
            hwnd: hwnd_value(hwnd),
            wake_msg,
        }
    }

    pub fn send(&self, event: T) -> Result<(), mpsc::SendError<T>> {
        self.tx.send(event)
    }

    pub unsafe fn wake(&self) {
        PostMessageW(to_hwnd(self.hwnd), self.wake_msg, 0, 0);
    }

    pub unsafe fn send_and_wake(&self, event: T) -> Result<(), mpsc::SendError<T>> {
        let result = self.send(event);
        self.wake();
        result
    }
}

pub fn event_channel<T>(hwnd: HWND, wake_msg: u32) -> (UiEventSender<T>, Receiver<T>) {
    let (tx, rx) = mpsc::channel();
    (UiEventSender::new(tx, hwnd, wake_msg), rx)
}

pub unsafe fn wake_window(hwnd: HWND, wake_msg: u32) {
    PostMessageW(hwnd, wake_msg, 0, 0);
}
