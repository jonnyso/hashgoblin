#[cfg(target_os = "windows")]
mod windows_path {
    use std::fs::File;
    use std::io;
    use std::mem::MaybeUninit;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
    use std::path::Path;
    use std::ptr;
    use std::{iter::once, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::Foundation::{
        GetLastError, INVALID_HANDLE_VALUE, MAX_PATH, WIN32_ERROR,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetVolumePathNameW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Ioctl::{
        PropertyStandardQuery, StorageDeviceSeekPenaltyProperty, DEVICE_SEEK_PENALTY_DESCRIPTOR,
        IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_PROPERTY_QUERY,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    enum Error {
        Win32(WIN32_ERROR),
        Io(io::Error),
    }

    fn get_volume_handle(path: &Path) -> Result<File, Error> {
        let s_path: Vec<u16> = path
            .canonicalize()
            .map_err(Error::Io)?
            .as_os_str()
            .encode_wide()
            .chain(once(0))
            .collect();
        let mut buf = [0u16; MAX_PATH as usize];
        if unsafe { GetVolumePathNameW(s_path.as_ptr(), buf.as_mut_ptr(), MAX_PATH) } == 0 {
            return Err(Error::Win32(unsafe { GetLastError() }));
        }
        if let Some(c) = buf.iter_mut().rfind(|c| **c != 0) {
            if *c == b'\\'.into() {
                *c = 0
            }
        }
        let handle = unsafe {
            CreateFileW(
                buf.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(Error::Win32(unsafe { GetLastError() }));
        }
        Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
    }

    fn is_ssd(volume: &File) -> Result<bool, WIN32_ERROR> {
        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceSeekPenaltyProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        let mut result = MaybeUninit::<DEVICE_SEEK_PENALTY_DESCRIPTOR>::uninit();
        let mut bytes_returned = MaybeUninit::<u32>::uninit();
        let ok = unsafe {
            DeviceIoControl(
                volume.as_raw_handle() as _,
                IOCTL_STORAGE_QUERY_PROPERTY,
                (&query as *const STORAGE_PROPERTY_QUERY).cast(),
                size_of::<STORAGE_PROPERTY_QUERY>().try_into().unwrap(),
                result.as_mut_ptr().cast(),
                size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>()
                    .try_into()
                    .unwrap(),
                bytes_returned.as_mut_ptr(),
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(unsafe { GetLastError() });
        }
        let result = unsafe { result.assume_init() };
        Ok(result.IncursSeekPenalty == 0)
    }
}
