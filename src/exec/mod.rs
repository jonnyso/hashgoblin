mod checker;
mod outfile;

use crate::hashing::{self, HashType, Hashed};
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
pub use outfile::OutFile;

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

pub fn run<T: HashHandler>(
    hash: &HashType,
    queue: &Queue,
    empty_dirs: bool,
    handler: &T,
) -> Result<(), Error> {
    let mut hasher = hashing::new_hasher(hash);
    while let Some(path) = queue.pop_front() {
        if is_canceled() {
            return Ok(());
        }
        if path.is_dir() {
            let is_empty = cancel_on_err(queue.push_dir(&path))?;
            if is_empty && empty_dirs {
                cancel_on_err(handler.handle(HashData(path, None)))?;
            }
        } else {
            let hash = match hashing::hash_file(&path, &mut *hasher) {
                Ok(Hashed::Value(value)) => Ok(value),
                Ok(Hashed::Canceled) => return Ok(()),
                Err(err) => Err(Error::Io((err, path_string(&path)))),
            };
            let hash = cancel_on_err(hash)?;
            cancel_on_err(handler.handle(HashData(path, Some(hash))))?;
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

pub struct Queue(Mutex<VecDeque<PathBuf>>);

impl Queue {
    pub fn new(input: &[String], recursive: bool) -> Result<Self, Error> {
        let mut queue = VecDeque::with_capacity(input.len());
        for path in input {
            let pathbuf = PathBuf::from(path);
            if pathbuf
                .metadata()
                .map_err(|err| Error::Io((err, path.to_owned())))?
                .is_dir()
                && !recursive
            {
                return Err(Error::IsDir(path.to_owned()));
            }
            queue.push_back(pathbuf);
        }
        Ok(Self(Mutex::new(queue)))
    }

    fn pop_front(&self) -> Option<PathBuf> {
        self.0.lock().unwrap().pop_front()
    }

    fn push_dir(&self, path: &Path) -> Result<bool, Error> {
        let mut is_empty = true;
        let mut queue = self.0.lock().unwrap();
        let reader = path
            .read_dir()
            .map_err(|err| Error::Io((err, path_string(path))))?;
        for entry in reader {
            is_empty = false;
            if is_canceled() {
                return Ok(is_empty);
            }
            queue.push_back(
                entry
                    .map_err(|err| Error::Io((err, path_string(path))))?
                    .path(),
            );
        }
        Ok(is_empty)
    }
}
