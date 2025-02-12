mod checker;
mod mtreader;
mod outfile;
mod streader;

const BUF_SIZE: usize = 36 * 1024;
const HANDLE_BUF_SIZE: usize = 8 * 1024;

use crate::hashing::{self, HashType};
use crate::path::{path_on_fast_drive, PathList};
use crate::Error;
use std::fmt::Display;
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
pub use streader::STReader;

static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn cancel() {
    println!("CANCELED");
    CANCEL.store(true, Ordering::Release);
}

pub fn is_canceled() -> bool {
    CANCEL.load(Ordering::Acquire)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[derive(Debug)]
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

pub enum ReadData<'a> {
    EmptyDir(PathBuf),
    OpenFile(&'a [u8]),
    FileDone(PathBuf),
}

pub trait ExecReader {
    fn get_handle(&self) -> impl ExecReaderHandle;
}

pub trait ExecReaderHandle {
    fn try_read(&mut self) -> Result<Option<ReadData>, Error>;
}

pub enum ReaderType {
    MT(Mutex<MTReader>),
    ST(Mutex<STReader>),
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn reader_list(paths: Vec<PathBuf>, max_threads: u8) -> Vec<ReaderType> {
    vec![ReaderType::MT(Mutex::new(MTReader::new(paths)))]
}

pub fn reader_list(paths: Vec<PathBuf>, max_threads: u8) -> Vec<ReaderType> {
    match path_on_fast_drive(paths) {
        Ok(path_list) => path_list
            .into_iter()
            .map(|pl| match pl {
                PathList::Ssd(list) => ReaderType::MT(Mutex::new(MTReader::new(list))),
                PathList::Hdd(list) => ReaderType::ST(Mutex::new(STReader::new(list, max_threads))),
            })
            .collect(),
        Err((paths, err)) => {
            eprintln!("{err},\ndefaulting to multithread reader");
            vec![ReaderType::MT(Mutex::new(MTReader::new(paths)))]
        }
    }
}

pub fn run<T: HashHandler, R: ExecReaderHandle>(
    hash: &HashType,
    mut reader: R,
    empty_dirs: bool,
    handler: &T,
) -> Result<(), Error> {
    let mut hasher = hashing::new_hasher(hash);
    while let Some(read_data) = reader.try_read()? {
        if is_canceled() {
            return Ok(());
        }
        match read_data {
            ReadData::EmptyDir(path) => {
                if empty_dirs {
                    cancel_on_err(handler.handle(HashData(path, None)))?;
                }
            }
            ReadData::OpenFile(buf) => hasher.update(buf),
            ReadData::FileDone(path) => {
                let hash = hex::encode(hasher.finalize_reset());
                cancel_on_err(handler.handle(HashData(path, Some(hash))))?;
            }
        }
    }
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
