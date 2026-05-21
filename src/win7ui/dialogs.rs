use std::path::PathBuf;

use windows_sys::Win32::{
    Foundation::HWND,
    UI::Controls::Dialogs::{
        GetOpenFileNameW, GetSaveFileNameW, OPENFILENAMEW, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY,
        OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
    },
};

use super::text::wide;

pub unsafe fn choose_file(
    owner: HWND,
    save: bool,
    filter: &str,
    title: &str,
    def_ext: &str,
) -> Option<PathBuf> {
    let mut file_buf = vec![0u16; 1024];
    let filter = wide(filter);
    let title = wide(title);
    let def_ext = wide(def_ext);
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: filter.as_ptr(),
        lpstrFile: file_buf.as_mut_ptr(),
        nMaxFile: file_buf.len() as u32,
        lpstrTitle: title.as_ptr(),
        lpstrDefExt: def_ext.as_ptr(),
        Flags: OFN_HIDEREADONLY | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if save {
        ofn.Flags |= OFN_OVERWRITEPROMPT;
    } else {
        ofn.Flags |= OFN_FILEMUSTEXIST;
    }

    let ok = if save {
        GetSaveFileNameW(&mut ofn)
    } else {
        GetOpenFileNameW(&mut ofn)
    };
    if ok == 0 {
        return None;
    }
    let len = file_buf.iter().position(|ch| *ch == 0).unwrap_or(0);
    Some(PathBuf::from(String::from_utf16_lossy(&file_buf[..len])))
}
