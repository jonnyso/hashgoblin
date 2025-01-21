mod handlers;

use crate::hashing::{self, Hash, Hashed};
use crate::Error;
use std::fmt::Display;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread::{self, ScopedJoinHandle},
};

pub use handlers::OutFile;

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

enum TryFromErr {
    EmptyDir,
    Error(Error),
}

impl HashData {
    fn try_from_string(value: String, empty_dirs: bool) -> Result<Self, TryFromErr> {
        let (path, hash) = value
            .split_once('|')
            .ok_or(TryFromErr::Error(Error::FileFormat))?;
        let path = PathBuf::from(path);
        match (hash.is_empty(), empty_dirs) {
            (true, true) => Ok(Self(path, None)),
            (true, false) => Err(TryFromErr::EmptyDir),
            (false, _) => Ok(Self(path, Some(hash.to_owned()))),
        }
    }
}

pub trait HashHandler {
    fn handle(&self, hash_data: HashData) -> Result<(), Error>;

    fn wrap_up(&self, handles: Vec<ScopedJoinHandle<Result<(), Error>>>) -> Result<(), Error> {
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .find(|handle| handle.is_err())
            .unwrap_or(Ok(()))
    }
}

pub fn create_hashes<T: HashHandler + Sync>(
    input: &[String],
    recursive: bool,
    max_threads: u8,
    hash: Hash,
    empty_dirs: bool,
    handler: T,
) -> Result<(), Error> {
    let queue = Queue::new(input, recursive)?;
    let cancel = AtomicBool::new(false);
    thread::scope(|s| {
        let mut handles = Vec::with_capacity(max_threads as usize);
        while handles.len() < max_threads as usize {
            handles.push(s.spawn(|| run(&hash, &queue, empty_dirs, &handler, &cancel)));
        }
        handler.wrap_up(handles)
    })
}

fn run<T: HashHandler>(
    hash: &Hash,
    queue: &Queue,
    empty_dirs: bool,
    handler: &T,
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let mut hasher = hashing::new_hasher(hash);
    while let Some(path) = queue.pop_front() {
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        if path.is_dir() {
            let is_empty = cancel_on_err(queue.push_dir(&path, cancel), cancel)?;
            if is_empty && empty_dirs {
                cancel_on_err(handler.handle(HashData(path, None)), cancel)?;
            }
        } else {
            let hash = match hashing::hash_file(&path, &mut *hasher, cancel) {
                Ok(Hashed::Value(value)) => Ok(value),
                Ok(Hashed::Canceled) => return Ok(()),
                Err(err) => Err(Error::Io((err, path.to_string_lossy().to_string()))),
            };
            let hash = cancel_on_err(hash, cancel)?;
            cancel_on_err(handler.handle(HashData(path, Some(hash))), cancel)?;
        }
    }
    Ok(())
}

fn cancel_on_err<T, E>(result: Result<T, E>, cancel: &AtomicBool) -> Result<T, E> {
    if result.is_err() {
        cancel.store(true, Ordering::Release);
    }
    result
}

struct Queue(Mutex<VecDeque<PathBuf>>);

impl Queue {
    fn new(input: &[String], recursive: bool) -> Result<Self, Error> {
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

    fn push_dir(&self, path: &Path, cancel: &AtomicBool) -> Result<bool, Error> {
        let mut is_empty = true;
        let mut queue = self.0.lock().unwrap();
        let reader = path
            .read_dir()
            .map_err(|err| Error::Io((err, path.to_string_lossy().to_string())))?;
        for entry in reader {
            is_empty = false;
            if cancel.load(Ordering::Acquire) {
                return Ok(is_empty);
            }
            queue.push_back(
                entry
                    .map_err(|err| Error::Io((err, path.to_string_lossy().to_string())))?
                    .path(),
            );
        }
        Ok(is_empty)
    }
}
