mod checker;
mod mtreader;
mod outfile;
mod streader;

const BUF_SIZE: usize = 36 * 1024;
const HANDLE_BUF_SIZE: usize = 8 * 1024;

use crate::hashing::{self, HashType, Hashed};
use crate::path::{path_on_fast_drive, PathList};
use crate::Error;
use std::fmt::Display;
use std::fs::File;
use std::io::{BufReader, Read};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

pub use checker::AuditSrc;
pub use mtreader::MTReader;
pub use outfile::OutFile;
pub use streader::{STReader, STReaderHandle};

static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn cancel() {
    CANCEL.store(true, Ordering::Release);
}

pub fn is_canceled() -> bool {
    CANCEL.load(Ordering::Acquire)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

pub struct HashData(PathBuf, Option<String>);

impl Display for HashData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}|{}",
            self.0.to_string_lossy(),
            self.1.as_deref().unwrap_or("")
        )
    }
}

impl HashData {
    fn try_from_string(value: String, empty_dirs: bool) -> Result<Self, Error> {
        let (path, hash) = value.split_once('|').ok_or(Error::FileFormat)?;
        let path = PathBuf::from(path);
        match (hash.is_empty(), empty_dirs) {
            (true, true) => Ok(Self(path, None)),
            (true, false) => Err(Error::AuditEmptyDir(path_string(&path))),
            (false, _) => Ok(Self(path, Some(hash.to_owned()))),
        }
    }
}

pub trait HashHandler {
    fn handle(&self, hash_data: HashData) -> Result<(), Error>;
}

enum ReadData {
    EmptyDir(PathBuf),
    File(Option<[u8; HANDLE_BUF_SIZE]>),
}

trait ExecReaderHandle {
    fn try_read(&mut self) -> Result<Option<ReadData>, Error>;
}

trait ExecReader {
    fn get_handle(&self) -> Option<impl ExecReaderHandle>;
}

pub enum ReaderType {
    MT(MTReader),
    ST(Mutex<STReader>),
}

struct ReaderQueue(VecDeque<ReaderType>);

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl ReaderQueue {
    fn new(paths: Vec<PathBuf>, max_threads: u8) -> Result<Self, ()> {
        let path_list = match path_on_fast_drive(paths) {
            Ok(paths) => paths,
            Err(_) => return Err(()),
        };
        let path_list = path_list.into_iter().map(|pl| match pl {
            PathList::SSD(list) => ReaderType::MT(MTReader::new(list)),
            PathList::HDD(list) => ReaderType::ST(Mutex::new(STReader::new(list, max_threads))),
        });
        Ok(Self(VecDeque::from_iter(path_list)))
    }
}

pub fn run<T: HashHandler, R: ExecReaderHandle>(
    hash: &HashType,
    reader: R,
    empty_dirs: bool,
    handler: &T,
) -> Result<(), Error> {
    // let mut hasher = hashing::new_hasher(hash);
    // while let Some(path) = queue.pop_front() {
    //     if is_canceled() {
    //         return Ok(());
    //     }
    //     if path.is_dir() {
    //         let is_empty = cancel_on_err(queue.push_dir(&path))?;
    //         if is_empty && empty_dirs {
    //             cancel_on_err(handler.handle(HashData(path, None)))?;
    //         }
    //     } else {
    //         let hash = match hashing::hash_file(&path, &mut *hasher) {
    //             Ok(Hashed::Value(value)) => Ok(value),
    //             Ok(Hashed::Canceled) => return Ok(()),
    //             Err(err) => Err(Error::Io((err, path_string(&path)))),
    //         };
    //         let hash = cancel_on_err(hash)?;
    //         cancel_on_err(handler.handle(HashData(path, Some(hash))))?;
    //     }
    // }
    Ok(())
}

fn cancel_on_err<T, E>(result: Result<T, E>) -> Result<T, E> {
    if result.is_err() {
        cancel();
    }
    result
}

fn push_dir(dir: &Path, queue: &mut VecDeque<PathBuf>) -> Result<bool, Error> {
    let mut is_empty = true;
    let entries =
        cancel_on_err(dir.read_dir()).map_err(|err| Error::Io((err, path_string(dir))))?;
    for entry in entries {
        is_empty = false;
        let entry = cancel_on_err(entry).map_err(|err| Error::Io((err, path_string(dir))))?;
        queue.push_back(entry.path());
    }
    Ok(is_empty)
}
