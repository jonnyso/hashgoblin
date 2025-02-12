use rustc_hash::{FxBuildHasher, FxHashMap};
use std::{
    collections::{hash_map::Entry, HashMap},
    fmt::{Debug, Display},
    io,
    path::PathBuf,
};

#[derive(Debug)]
#[allow(dead_code)]
pub enum Error<T: Debug> {
    Platform(T),
    Io(io::Error),
    PathToStr,
    Utf8,
}

impl<T: Display + Debug> Display for Error<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Platform(err) => write!(f, "failed to determine if drive is ssd: {err}"),
            Error::Io(err) => write!(f, "failed to determine if drive is ssd: {err}"),
            Error::PathToStr | Error::Utf8 => {
                write!(f, "failed to determine if drive is ssd: invalid path")
            }
        }
    }
}

pub enum PathList<T> {
    Ssd(Vec<T>),
    Hdd(Vec<T>),
}

#[cfg(target_os = "windows")]
mod windows_path {
    use super::Error;
    use std::ffi::OsString;
    use std::fmt::Display;
    use std::fs::File;
    use std::mem::MaybeUninit;
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
    use std::path::Path;
    use std::ptr;
    use std::{iter::once, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::Foundation::{
        GetLastError, ERROR_INVALID_FUNCTION, INVALID_HANDLE_VALUE, MAX_PATH, WIN32_ERROR,
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
    pub enum Win32Err {
        VolumePathName(WIN32_ERROR),
        CreateFileW(WIN32_ERROR),
        DeviceIoCtl(WIN32_ERROR),
    }

    impl Display for Win32Err {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::VolumePathName(code) => {
                    write!(f, "trying to get the volume path name, error code ({code})")
                }
                Self::CreateFileW(code) => {
                    write!(f, "trying to create device handle, error code ({code})")
                }
                Self::DeviceIoCtl(code) => {
                    write!(f, "trying to query device info, error code ({code})")
                }
            }
        }
    }

    pub fn get_volume_handle(path: &Path) -> Result<(OsString, File), Error<Win32Err>> {
        let s_path: Vec<u16> = path
            .canonicalize()
            .map_err(Error::Io)?
            .as_os_str()
            .encode_wide()
            .chain(once(0))
            .collect();
        let mut buf = [0u16; MAX_PATH as usize];
        if unsafe { GetVolumePathNameW(s_path.as_ptr(), buf.as_mut_ptr(), MAX_PATH) } == 0 {
            return Err(Error::Platform(unsafe {
                Win32Err::VolumePathName(GetLastError())
            }));
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
            return Err(Error::Platform(unsafe {
                Win32Err::CreateFileW(GetLastError())
            }));
        }
        Ok(unsafe {
            (
                OsString::from_wide(&buf),
                File::from_raw_handle(handle as RawHandle),
            )
        })
    }

    pub fn drive_incurs_penalty(volume: &File) -> Result<bool, Win32Err> {
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
            let err_code = unsafe { GetLastError() };
            if err_code == ERROR_INVALID_FUNCTION {
                return Ok(true);
            } else {
                return Err(Win32Err::DeviceIoCtl(err_code));
            }
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
        fmt::Display,
        fs::File,
        io::{self, Read},
        os::unix::ffi::OsStringExt,
        path::{Path, PathBuf},
        process::Command,
    };

    #[derive(Debug)]
    pub struct ErrMessage(OsString);

    impl Display for ErrMessage {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{:?}", self.0)
        }
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

    pub fn find_volume_name(path: &Path) -> Result<String, Error<ErrMessage>> {
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
            return Err(Error::Platform(ErrMessage(OsString::from_vec(
                output.stderr,
            ))));
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

type PathResult<T> = Result<Vec<PathList<PathBuf>>, (Vec<PathBuf>, Error<T>)>;

#[cfg(target_os = "linux")]
pub fn path_on_fast_drive(paths: Vec<PathBuf>) -> PathResult<linux_path::ErrMessage> {
    use std::rc::Rc;

    let mut index_map = HashMap::with_capacity_and_hasher(paths.len(), FxBuildHasher);
    let mut paths = FxHashMap::from_iter(paths.into_iter().enumerate());
    for (index, path) in paths.iter() {
        let volume_name = match linux_path::find_volume_name(path) {
            Ok(name) => Rc::from(name),
            Err(err) => return Err((paths.into_values().collect(), err)),
        };
        match index_map.entry(Rc::clone(&volume_name)) {
            Entry::Occupied(entry) => match entry.into_mut() {
                PathList::Ssd(list) | PathList::Hdd(list) => list.push(*index),
            },
            Entry::Vacant(entry) => {
                let new = match linux_path::is_rotational(&volume_name) {
                    Ok(true) => PathList::Hdd(vec![*index]),
                    Ok(false) => PathList::Ssd(vec![*index]),
                    Err(err) => return Err((paths.into_values().collect(), Error::Io(err))),
                };
                entry.insert(new);
            }
        }
    }
    let index_map = index_map
        .into_values()
        .map(|path_list| match path_list {
            PathList::Ssd(indexes) => PathList::Ssd(
                indexes
                    .into_iter()
                    .map(|index| paths.remove(&index).unwrap())
                    .collect(),
            ),
            PathList::Hdd(indexes) => PathList::Hdd(
                indexes
                    .into_iter()
                    .map(|index| paths.remove(&index).unwrap())
                    .collect(),
            ),
        })
        .collect();
    Ok(index_map)
}

#[cfg(target_os = "windows")]
pub fn path_on_fast_drive(paths: Vec<PathBuf>) -> PathResult<windows_path::Win32Err> {
    let mut index_map = HashMap::with_capacity_and_hasher(paths.len(), FxBuildHasher);
    let mut paths = FxHashMap::from_iter(paths.into_iter().enumerate());
    for (index, path) in paths.iter() {
        let (drive_path, handle) = match windows_path::get_volume_handle(path) {
            Ok(path_info) => path_info,
            Err(err) => return Err((paths.into_values().collect(), err)),
        };
        match index_map.entry(drive_path) {
            Entry::Occupied(entry) => match entry.into_mut() {
                PathList::Ssd(list) | PathList::Hdd(list) => list.push(*index),
            },
            Entry::Vacant(entry) => {
                let new = match windows_path::drive_incurs_penalty(&handle) {
                    Ok(true) => PathList::Hdd(vec![*index]),
                    Ok(false) => PathList::Ssd(vec![*index]),
                    Err(err) => return Err((paths.into_values().collect(), Error::Platform(err))),
                };
                entry.insert(new);
            }
        }
    }
    let index_map = index_map
        .into_values()
        .map(|path_list| match path_list {
            PathList::Ssd(list) => PathList::Ssd(
                list.into_iter()
                    .map(|index| paths.remove(&index).unwrap())
                    .collect(),
            ),
            PathList::Hdd(list) => PathList::Hdd(
                list.into_iter()
                    .map(|index| paths.remove(&index).unwrap())
                    .collect(),
            ),
        })
        .collect();
    Ok(index_map)
}
