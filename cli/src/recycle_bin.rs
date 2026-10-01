//! Recycle Bin delete that can never delete permanently (Windows).
//!
//! `IFileOperation` is told to recycle, and a progress sink guards the result:
//! `PreDeleteItem` vetoes the delete (`E_ABORT`) when Windows has cleared
//! `TSF_DELETE_RECYCLE_IF_POSSIBLE`, which means it would delete permanently;
//! `PostDeleteItem` must report a new item in the bin, or the call is an error.

use std::io;
use std::path::Path;

#[cfg(windows)]
use std::cell::Cell;
#[cfg(windows)]
use windows::core::{Ref, HRESULT, PCWSTR};
#[cfg(windows)]
use windows::Win32::Foundation::E_ABORT;
#[cfg(windows)]
use windows::Win32::UI::Shell::{
    IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem, TSF_DELETE_RECYCLE_IF_POSSIBLE,
};

/// What the delete sink saw for the one item.
#[cfg(windows)]
#[derive(Default)]
struct Seen {
    vetoed: Cell<bool>,
    post_hr: Cell<Option<i32>>,
    post_in_bin: Cell<Option<bool>>,
}

#[cfg(windows)]
#[windows::core::implement(IFileOperationProgressSink)]
struct Sink(std::rc::Rc<Seen>);

#[cfg(windows)]
#[allow(non_snake_case)]
impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _hr: HRESULT) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(&self, _f: u32, _i: Ref<'_, IShellItem>, _n: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostRenameItem(&self, _f: u32, _i: Ref<'_, IShellItem>, _n: &PCWSTR, _hr: HRESULT, _new: Ref<'_, IShellItem>) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreMoveItem(&self, _f: u32, _i: Ref<'_, IShellItem>, _d: Ref<'_, IShellItem>, _n: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostMoveItem(&self, _f: u32, _i: Ref<'_, IShellItem>, _d: Ref<'_, IShellItem>, _n: &PCWSTR, _hr: HRESULT, _new: Ref<'_, IShellItem>) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreCopyItem(&self, _f: u32, _i: Ref<'_, IShellItem>, _d: Ref<'_, IShellItem>, _n: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostCopyItem(&self, _f: u32, _i: Ref<'_, IShellItem>, _d: Ref<'_, IShellItem>, _n: &PCWSTR, _hr: HRESULT, _new: Ref<'_, IShellItem>) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreDeleteItem(&self, flags: u32, _i: Ref<'_, IShellItem>) -> windows::core::Result<()> {
        // Windows clears TSF_DELETE_RECYCLE_IF_POSSIBLE when it cannot recycle this item and
        // would delete it permanently. A failure here cancels the delete before it starts.
        if flags & TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 == 0 {
            self.0.vetoed.set(true);
            return Err(E_ABORT.into());
        }
        Ok(())
    }
    fn PostDeleteItem(&self, _f: u32, _i: Ref<'_, IShellItem>, hr: HRESULT, new: Ref<'_, IShellItem>) -> windows::core::Result<()> {
        self.0.post_hr.set(Some(hr.0));
        // A recycled item has a new item in the bin; a permanently deleted one has none.
        self.0.post_in_bin.set(Some(new.is_some()));
        Ok(())
    }
    fn PreNewItem(&self, _f: u32, _d: Ref<'_, IShellItem>, _n: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostNewItem(&self, _f: u32, _d: Ref<'_, IShellItem>, _n: &PCWSTR, _t: &PCWSTR, _a: u32, _hr: HRESULT, _new: Ref<'_, IShellItem>) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _t: u32, _s: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Send one file to the Recycle Bin. Never falls back to a permanent delete.
#[cfg(windows)]
pub fn recycle(path: &Path) -> io::Result<()> {
    let abs = std::path::absolute(path)?;
    if abs.as_os_str().len() >= 260 {
        return Err(io::Error::other("path is 260+ characters, which the Recycle Bin call cannot take; left in place"));
    }
    // COM state is per thread: use a thread of our own so the caller's apartment is untouched.
    let p = abs.clone();
    std::thread::spawn(move || recycle_com(&p))
        .join()
        .map_err(|_| io::Error::other("Recycle Bin thread panicked"))??;
    if abs.exists() {
        return Err(io::Error::other("still present after the Recycle Bin call"));
    }
    Ok(())
}

#[cfg(windows)]
fn recycle_com(abs: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        FileOperation, IFileOperation, SHCreateItemFromParsingName, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
    };
    let e = |what: &str, err: windows::core::Error| io::Error::other(format!("{what}: {err}"));
    let w: Vec<u16> = abs.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: plain COM calls on this thread; every pointer passed is valid for the call.
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if init.is_err() {
            return Err(e("COM init", windows::core::Error::from(init)));
        }
        let seen = std::rc::Rc::new(Seen::default());
        let res = (|| {
            let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(|x| e("create IFileOperation", x))?;
            op.SetOperationFlags(FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).map_err(|x| e("set flags", x))?;
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).map_err(|x| e("open item", x))?;
            let sink: IFileOperationProgressSink = Sink(seen.clone()).into();
            op.DeleteItem(&item, &sink).map_err(|x| e("queue delete", x))?;
            let performed = op.PerformOperations();
            if seen.vetoed.get() {
                return Err(io::Error::other(
                    "Windows cannot send this file to the Recycle Bin (no bin for this location, or the bin cannot take it); left in place",
                ));
            }
            performed.map_err(|x| e("Recycle Bin call", x))?;
            if op.GetAnyOperationsAborted().map_err(|x| e("abort check", x))?.as_bool() {
                return Err(io::Error::other("the Recycle Bin call was aborted"));
            }
            match (seen.post_hr.get(), seen.post_in_bin.get()) {
                (Some(hr), Some(true)) if hr >= 0 => Ok(()),
                other => Err(io::Error::other(format!("the Recycle Bin call did not confirm a bin copy ({other:?})"))),
            }
        })();
        CoUninitialize();
        res
    }
}

#[cfg(not(windows))]
pub fn recycle(_path: &Path) -> io::Result<()> {
    Err(io::Error::other("the Recycle Bin is Windows-only"))
}
