use std::{
    collections::{hash_map::Entry, HashMap},
    fmt::Debug,
    io,
    path::PathBuf,
};

#[derive(Debug)]
pub enum Error<T: Debug> {
    Platform(T),
    Io(io::Error),
    PathToStr,
    Utf8,
}

pub enum PathList {
    SSD(Vec<PathBuf>),
    HDD(Vec<PathBuf>),
}

#[cfg(target_os = "windows")]
mod windows_path {
    use super::Error;
    use std::ffi::OsString;
    use std::fs::File;
    use std::mem::MaybeUninit;
    use std::os::windows::ffi::OsStringExt;
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

    pub fn get_volume_handle(path: &Path) -> Result<(OsString, File), Error<WIN32_ERROR>> {
        let s_path: Vec<u16> = path
            .canonicalize()
            .map_err(Error::Io)?
            .as_os_str()
            .encode_wide()
            .chain(once(0))
            .collect();
        let mut buf = [0u16; MAX_PATH as usize];
        if unsafe { GetVolumePathNameW(s_path.as_ptr(), buf.as_mut_ptr(), MAX_PATH) } == 0 {
            return Err(Error::Platform(unsafe { GetLastError() }));
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
            return Err(Error::Platform(unsafe { GetLastError() }));
        }
        Ok(unsafe {
            (
                OsString::from_wide(&buf),
                File::from_raw_handle(handle as RawHandle),
            )
        })
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
    use super::Error;
    use core::panic;
    use std::{
        ffi::OsString,
        fs::File,
        io::{self, Read},
        os::unix::ffi::OsStringExt,
        path::{Path, PathBuf},
        process::Command,
    };

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

    pub fn find_volume_name(path: &Path) -> Result<String, Error<OsString>> {
        let path = path.canonicalize().map_err(Error::Io)?;
        let mut output = Command::new("findmnt")
            .args([
                "-no",
                "source",
                "-T",
                path.to_str().ok_or(Error::PathToStr)?,
            ])
            .output()
            .map_err(Error::Io)?;
        if !output.stderr.is_empty() {
            return Err(Error::Platform(OsString::from_vec(output.stderr)));
        }
        output.stdout.pop(); //removing line break
        Ok(extract_device_name(
            String::from_utf8(output.stdout).map_err(|_| Error::Utf8)?,
        ))
    }

    pub fn is_rotational(volume: &str) -> Result<bool, io::Error> {
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

#[cfg(target_os = "linux")]
pub fn path_on_fast_drive(paths: Vec<PathBuf>) -> Result<Vec<PathList>, Error<std::ffi::OsString>> {
    use std::rc::Rc;

    let mut map = HashMap::with_capacity(paths.len());
    for path in paths {
        let volume_name = Rc::from(linux_path::find_volume_name(&path)?);
        match map.entry(Rc::clone(&volume_name)) {
            Entry::Occupied(entry) => match entry.into_mut() {
                PathList::SSD(list) | PathList::HDD(list) => list.push(path),
            },
            Entry::Vacant(entry) => {
                let is_rotational = linux_path::is_rotational(&volume_name).map_err(Error::Io)?;
                let new = if is_rotational {
                    PathList::HDD(vec![path])
                } else {
                    PathList::SSD(vec![path])
                };
                entry.insert(new);
            }
        }
    }
    Ok(map.into_values().collect())
}

#[cfg(target_os = "windows")]
pub fn path_on_fast_drive(
    paths: Vec<PathBuf>,
) -> Result<Vec<PathList>, Error<windows_sys::Win32::Foundation::WIN32_ERROR>> {
    let mut map = HashMap::with_capacity(paths.len());
    for path in paths {
        let (drive_path, handle) = windows_path::get_volume_handle(&path)?;
        match map.entry(drive_path) {
            Entry::Occupied(entry) => match entry.into_mut() {
                PathList::SSD(list) | PathList::HDD(list) => list.push(path),
            },
            Entry::Vacant(entry) => {
                let incurs_penalty =
                    windows_path::drive_incurs_penalty(&handle).map_err(Error::Platform)?;
                let new = if incurs_penalty {
                    PathList::HDD(vec![path])
                } else {
                    PathList::SSD(vec![path])
                };
                entry.insert(new);
            }
        }
    }
    Ok(map.into_values().collect())
}
