use std::path::Path;

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

    #[derive(Debug)]
    pub enum Error {
        Win32(WIN32_ERROR),
        Io(io::Error),
    }

    pub fn get_volume_handle(path: &Path) -> Result<File, Error> {
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

    pub fn drive_incurs_penalty(volume: &File) -> Result<bool, WIN32_ERROR> {
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
        Ok(result.IncursSeekPenalty == 1)
    }
}

#[cfg(target_os = "linux")]
mod linux_path {
    use core::panic;
    use std::{
        ffi::OsString,
        fs::File,
        io::{self, Read},
        os::unix::ffi::OsStringExt,
        path::{Path, PathBuf},
        process::{Command, ExitStatus},
    };

    #[derive(Debug)]
    pub enum Error {
        Command(ExitStatus, OsString),
        Io(io::Error),
    }

    // /dev/sda -> sda
    // /dev/nvme0n1p3[/subovol] -> nvme0n1
    fn extract_device_name(mut name: String) -> String {
        if let Some(index) = name.rfind('[') {
            name.truncate(index);
        }
        if let Some(index) = name.rfind('/') {
            name = name.split_off(index + 1);
        }
        if name.starts_with("nvme") {
            if let Some(index) = name.rfind('p') {
                name.truncate(index);
            }
        }
        name
    }

    pub fn find_volume_name(path: &Path) -> Result<String, Error> {
        let path = path.canonicalize().map_err(Error::Io)?;
        let mut output = Command::new("findmnt")
            .args(["-no", "source", "-T", path.to_str().expect("invalid path")])
            .output()
            .map_err(Error::Io)?;
        if !output.stderr.is_empty() {
            return Err(Error::Command(
                output.status,
                OsString::from_vec(output.stderr),
            ));
        }
        output.stdout.pop(); //removing line break
        Ok(extract_device_name(
            String::from_utf8(output.stdout).expect("expected volume name to be utf-8"),
        ))
    }

    pub fn is_rotational(volume: String) -> Result<bool, io::Error> {
        let mut path = PathBuf::from("/sys/block");
        path.push(volume);
        path.push("queue/rotational");
        let mut file = File::open(path)?;
        let mut value = String::new();
        file.read_to_string(&mut value)?;
        match value.trim() {
            "1" => Ok(true),
            "0" => Ok(false),
            val => panic!("unexpected content for rotational file: `{val}`"),
        }
    }
}

pub fn path_on_fast_drive(path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let volume = linux_path::find_volume_name(path).unwrap();
        !linux_path::is_rotational(volume).unwrap()
    }

    #[cfg(target_os = "windows")]
    {
        let handle = windows_path::get_volume_handle(path).unwrap();
        !windows_path::drive_incurs_penalty(&handle).unwrap()
    }
}
