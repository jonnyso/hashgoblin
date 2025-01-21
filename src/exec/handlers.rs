use crate::{hashing::Hash, Error, DEFAULT_OUT};
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, LineWriter, Lines, Write},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread::ScopedJoinHandle,
};

use super::{HashData, HashHandler, TryFromErr};

pub struct OutFile {
    writer: Mutex<BufWriter<File>>,
    path: PathBuf,
}

impl OutFile {
    pub fn new(path: Option<PathBuf>, hash: &Hash) -> Result<Self, Error> {
        let path = path.unwrap_or(PathBuf::from(DEFAULT_OUT));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(Error::Output)?;
        let mut writer = BufWriter::new(file);
        let version = env!("CARGO_PKG_VERSION");
        writer
            .write_all(format!("version {version}\nalgo {hash}\n").as_bytes())
            .map_err(Error::Output)?;
        Ok(Self {
            writer: Mutex::new(writer),
            path,
        })
    }
}

impl HashHandler for OutFile {
    fn handle(&self, hash_data: HashData) -> Result<(), Error> {
        let line = format!("{hash_data}\n");
        self.writer
            .lock()
            .unwrap()
            .write_all(line.as_bytes())
            .map_err(Error::Output)
    }

    fn wrap_up(&self, handles: Vec<ScopedJoinHandle<Result<(), Error>>>) -> Result<(), Error> {
        let result = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .find(|result| result.is_err())
            .unwrap_or(Ok(()));
        if let Err(err) = fs::remove_file(&self.path) {
            eprintln!("Failed to clean up output file: {err}");
        }
        result
    }
}

enum AuditError {
    NotFound(String),
    Mismatch(String),
    Extra(String),
    EmptyDir(String),
}

impl AuditError {
    fn handle(&self, early: bool, cancel: &AtomicBool) {
        match self {
            AuditError::NotFound(path) => println!("audit_err: \"{path}\" not found "),
            AuditError::Mismatch(path) => println!("audit_err: \"{path}\" does not match"),
            AuditError::Extra(path) => {
                println!("audit_err: aditional \"{path}\" file found in audit source")
            }
            AuditError::EmptyDir(path) => {
                println!("audit_err: directory \"{path}\" should not be empty")
            }
        };
        if early {
            cancel.store(true, Ordering::Release);
        }
    }
}

fn compare_hash(
    path: &Path,
    backlog_hash: Option<&str>,
    checklist_hash: Option<&str>,
) -> Result<(), AuditError> {
    match (backlog_hash, checklist_hash) {
        (None, None) => Ok(()),
        (Some(h), Some(lh)) => {
            if h == lh {
                Ok(())
            } else {
                Err(AuditError::Mismatch(path.to_string_lossy().to_string()))
            }
        }
        (None, Some(_)) => Err(AuditError::Extra(path.to_string_lossy().to_string())),
        (Some(_), None) => Err(AuditError::EmptyDir(path.to_string_lossy().to_string())),
    }
}

enum ReaderErr {
    Error(Error),
    Audit(AuditError),
}

struct Checker<'a> {
    backlog: Mutex<VecDeque<HashData>>,
    reader: Lines<BufReader<File>>,
    checklist: VecDeque<HashData>,
    cancel: &'a AtomicBool,
    early: bool,
    empty_dirs: bool,
}

impl<'a> Checker<'a> {
    pub fn new(
        path: Option<PathBuf>,
        cancel: &'a AtomicBool,
        early: bool,
        empty_dirs: bool,
    ) -> Result<(Self, Hash), Error> {
        let path = path.unwrap_or(PathBuf::from(DEFAULT_OUT));
        let file = File::open(&path)
            .map_err(|err| Error::Io((err, path.to_string_lossy().to_string())))?;
        let mut lines = BufReader::new(file).lines();
        match lines.next() {
            None => return Err(Error::FileFormat),
            Some(Err(err)) => return Err(Error::ReadLine(err)),
            Some(Ok(line)) => match line.split_once(char::is_whitespace) {
                Some(("version", version)) => {
                    let current = env!("CARGO_PKG_VERSION");
                    if current != version {
                        eprintln!(
                            "WARNING: the hashes file was created using a different version of this program"
                        );
                    }
                }
                _ => return Err(Error::FileFormat),
            },
        };
        let hash = match lines.next() {
            None => return Err(Error::FileFormat),
            Some(Err(err)) => return Err(Error::ReadLine(err)),
            Some(Ok(line)) => match line.split_once(char::is_whitespace) {
                Some(("algo", hash)) => Hash::from_str(hash).map_err(Error::InvalidHash)?,
                _ => return Err(Error::FileFormat),
            },
        };
        let checker = Self {
            backlog: Mutex::new(VecDeque::new()),
            reader: lines,
            checklist: VecDeque::with_capacity(100),
            cancel,
            early,
            empty_dirs,
        };
        Ok((checker, hash))
    }

    fn push_to_backlog(&self, hash_data: HashData) {
        self.backlog.lock().unwrap().push_back(hash_data);
    }

    fn from_backlog(&self) -> Option<HashData> {
        self.backlog.lock().unwrap().pop_front()
    }

    fn from_reader(&mut self) -> Result<Option<HashData>, TryFromErr> {
        let next_line = match self.reader.next() {
            Some(result) => result.map_err(|err| TryFromErr::Error(Error::ReadLine(err)))?,
            None => return Ok(None),
        };
        HashData::try_from_string(next_line, self.empty_dirs).map(Some)
    }

    fn search_checklist(&mut self, HashData(path, hash): HashData) -> Result<bool, AuditError> {
        let len = self.checklist.len();
        for _ in 0..len {
            if self.cancel.load(Ordering::Acquire) {
                return Ok(false);
            }
            if let Some(HashData(list_path, list_hash)) = self.checklist.pop_front() {
                if list_path != path {
                    self.checklist.push_back(HashData(list_path, list_hash));
                    continue;
                }
                return compare_hash(&path, hash.as_deref(), list_hash.as_deref()).map(|_| true);
            }
        }
        Ok(false)
    }

    fn search_reader(&mut self, HashData(path, hash): HashData) -> Option<Result<bool, ReaderErr>> {
        loop {
            break match self.from_reader() {
                Err(TryFromErr::EmptyDir) => continue,
                Err(TryFromErr::Error(err)) => Some(Err(ReaderErr::Error(err))),
                Ok(None) => None,
                Ok(Some(HashData(reader_path, reader_hash))) => {
                    if reader_path != path {
                        self.checklist.push_back(HashData(reader_path, reader_hash));
                        continue;
                    }
                    Some(
                        compare_hash(&path, hash.as_deref(), reader_hash.as_deref())
                            .map(|_| true)
                            .map_err(ReaderErr::Audit),
                    )
                }
            };
        }
    }

    pub fn check(&mut self) -> Result<bool, Error> {
        let mut audit_err = false;
        while let Some(hash_data) = self.from_backlog() {
            if !self.checklist.is_empty() {
                if self.cancel.load(Ordering::Acquire) {
                    return Ok(false);
                }
                match self.search_checklist(hash_data) {
                    Ok(true) => continue,
                    Ok(false) => (),
                    Err(err) => {
                        audit_err = true;
                        err.handle(self.early, &self.cancel)
                    }
                };
            }
            todo!()
        }
        Ok(audit_err)
    }
}

impl HashHandler for Checker<'_> {
    fn handle(&self, hash_data: HashData) -> Result<(), Error> {
        self.push_to_backlog(hash_data);
        Ok(())
    }
}
