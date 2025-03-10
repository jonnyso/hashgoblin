mod checker;
mod outfile;

use crate::hashing::{self, HashType, Hashed};
use crate::{Error, verbose_print};
use std::fmt::Display;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub use checker::{Checker, load_check_file};
pub use outfile::OutFile;

const NO_DATE_STR: &str = "[NO DATE]";
const TIME_START_STR: &str = "time_start";
const TIME_FINISH_STR: &str = "time_finish";
const VERSION_STR: &str = "version";
const HASH_ALGO_STR: &str = "algo";

static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn cancel() {
    CANCEL.store(true, Ordering::Release);
    verbose_print(|| "CANCELED", true);
}

pub fn is_canceled() -> bool {
    CANCEL.load(Ordering::Acquire)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

pub struct HashData(PathBuf, Option<String>);

impl HashData {
    fn new(path: PathBuf) -> Self {
        Self(path, None)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn push_hash(&mut self, hash_str: String) {
        match &mut self.1 {
            Some(hashes) => hashes.push_str(format!(",{hash_str}").as_str()),
            None => self.1 = Some(hash_str),
        }
    }
}

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
    hashes: &[HashType],
    queue: &Queue,
    empty_dirs: bool,
    handler: T,
) -> Result<(), Error> {
    while let Some(path) = queue.pop_front() {
        if is_canceled() {
            return Ok(());
        }
        if path.is_dir() {
            verbose_print(|| format!("hashing: reading dir {:?}", &path), true);
            let is_empty = cancel_on_err(queue.push_dir(&path))?;
            if is_empty && empty_dirs {
                cancel_on_err(handler.handle(HashData(path, None)))?;
            }
        } else {
            verbose_print(|| format!("hashing file: {:?}", &path), true);
            let mut hash_data = HashData::new(path);
            for hash in hashes {
                let mut hasher = hashing::new_hasher(hash);
                let hash = match hashing::hash_file(hash_data.path(), &mut *hasher) {
                    Ok(Hashed::Value(value)) => Ok(value),
                    Ok(Hashed::Canceled) => return Ok(()),
                    Err(err) => Err(Error::Io((err, path_string(hash_data.path())))),
                };
                let hash = cancel_on_err(hash)?;
                hash_data.push_hash(hash);
            }
            cancel_on_err(handler.handle(hash_data))?;
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
