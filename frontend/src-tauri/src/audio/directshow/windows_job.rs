use std::io;

#[cfg(target_os = "windows")]
pub(crate) struct KillOnCloseJob {
    handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(target_os = "windows")]
impl KillOnCloseJob {
    pub(crate) fn assign(child: &tokio::process::Child) -> io::Result<Self> {
        use std::ffi::c_void;
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|_| io::Error::other("could not create capture job object"))?;
        let mut information = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&raw const information).cast::<c_void>(),
                std::mem::size_of_val(&information) as u32,
            )
        };
        if configured.is_err() {
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
            return Err(io::Error::other("could not configure capture job object"));
        }

        let process = child
            .raw_handle()
            .map(|raw| HANDLE(raw.cast()))
            .ok_or_else(|| io::Error::other("capture process handle was unavailable"));
        let assigned = process.and_then(|process| {
            unsafe { AssignProcessToJobObject(handle, process) }
                .map_err(|_| io::Error::other("could not assign capture process to job object"))
        });
        if let Err(error) = assigned {
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
            return Err(error);
        }
        Ok(Self { handle })
    }
}

#[cfg(target_os = "windows")]
impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.handle) };
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) struct KillOnCloseJob;

#[cfg(not(target_os = "windows"))]
impl KillOnCloseJob {
    pub(crate) fn assign(_child: &tokio::process::Child) -> io::Result<Self> {
        Ok(Self)
    }
}
