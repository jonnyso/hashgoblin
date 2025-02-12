use std::{
    collections::VecDeque,
    fmt::Display,
    fs::File,
    io::{BufRead, BufReader, Lines},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Mutex,
    thread::{self, ScopedJoinHandle},
    time::Duration,
};

use crate::{exec::cancel, Error, HashType, DEFAULT_OUT};

use super::{cancel_on_err, is_canceled, path_string, HashData, HashHandler};

enum AuditError {
    NotFound(String),
    Mismatch(String),
    Extra(String),
    EmptyDir(String),
}

impl Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditError::NotFound(path) => write!(f, "audit_err: \"{path}\" not found "),
            AuditError::Mismatch(path) => write!(f, "audit_err: \"{path}\" does not match"),
            AuditError::Extra(path) => {
                write!(f, "audit_err: aditional \"{path}\" found in audit source")
            }
            AuditError::EmptyDir(path) => {
                write!(f, "audit_err: directory \"{path}\" should not be empty")
            }
        }
    }
}

impl AuditError {
    fn print_and_cancel(&self, early: bool) {
        println!("{self}");
        if early {
            cancel();
        }
    }
}

type HashesFile = Lines<BufReader<File>>;

fn load_check_file(path: Option<PathBuf>) -> Result<(HashesFile, HashType), Error> {
    let path = path.unwrap_or(PathBuf::from(DEFAULT_OUT));
    let file = File::open(&path).map_err(|err| Error::Io((err, path_string(&path))))?;
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
            Some(("algo", hash)) => HashType::from_str(hash).map_err(Error::InvalidHash)?,
            _ => return Err(Error::FileFormat),
        },
    };
    Ok((lines, hash))
}

enum ComparedPath {
    Audit(AuditError),
    Unrelated,
}

fn compare_paths(reader_path: &Path, path: &Path) -> Result<(), ComparedPath> {
    if reader_path == path {
        return Ok(());
    }
    match (reader_path.is_dir(), path.is_dir()) {
        (true, false) => match path.ancestors().nth(1) {
            Some(ancestor) if ancestor == reader_path => {
                Err(ComparedPath::Audit(AuditError::Extra(path_string(path))))
            }
            _ => Err(ComparedPath::Unrelated),
        },
        (false, true) => match reader_path.ancestors().nth(1) {
            Some(ancestor) if ancestor == path => {
                Err(ComparedPath::Audit(AuditError::EmptyDir(path_string(path))))
            }
            _ => Err(ComparedPath::Unrelated),
        },
        _ => Err(ComparedPath::Unrelated),
    }
}

enum ReaderErr {
    Error(Error),
    Audit(AuditError),
}

pub struct AuditSrc {
    queue: Mutex<VecDeque<HashData>>,
    early: bool,
    empty_dirs: bool,
}

impl AuditSrc {
    pub fn new(
        path: Option<PathBuf>,
        early: bool,
        empty_dirs: bool,
    ) -> Result<(Self, HashType, HashesFile), Error> {
        let (reader, hash) = load_check_file(path)?;
        let src = Self {
            queue: Mutex::new(VecDeque::new()),
            early,
            empty_dirs,
        };
        Ok((src, hash, reader))
    }

    pub fn checker(&self, reader: HashesFile) -> Checker {
        Checker {
            source: self,
            reader,
            backlog: VecDeque::with_capacity(100),
            audit_err: false,
        }
    }

    fn push_back(&self, hash_data: HashData) {
        self.queue.lock().unwrap().push_back(hash_data);
    }

    fn pop_front(&self) -> Option<HashData> {
        self.queue.lock().unwrap().pop_front()
    }
}

impl HashHandler for AuditSrc {
    fn handle(&self, hash_data: HashData) -> Result<(), Error> {
        self.push_back(hash_data);
        Ok(())
    }
}

pub struct Checker<'a> {
    source: &'a AuditSrc,
    reader: HashesFile,
    backlog: VecDeque<HashData>,
    audit_err: bool,
}

impl Checker<'_> {
    fn read_next(&mut self) -> Result<Option<HashData>, Error> {
        let next_line = match self.reader.next() {
            Some(result) => result.map_err(Error::ReadLine)?,
            None => return Ok(None),
        };
        HashData::try_from_string(next_line, self.source.empty_dirs).map(Some)
    }

    fn search_backlog(&mut self, HashData(path, hash): &HashData) -> Result<bool, AuditError> {
        let len = self.backlog.len();
        for _ in 0..len {
            if is_canceled() {
                return Ok(false);
            }
            if let Some(HashData(list_path, list_hash)) = self.backlog.pop_front() {
                match compare_paths(&list_path, path) {
                    Ok(_) => {
                        return if &list_hash == hash {
                            Ok(true)
                        } else {
                            return Err(AuditError::Mismatch(path_string(path)));
                        }
                    }
                    Err(ComparedPath::Unrelated) => {
                        self.backlog.push_back(HashData(list_path, list_hash));
                        continue;
                    }
                    Err(ComparedPath::Audit(audit_err)) => {
                        if let AuditError::EmptyDir(_) = &audit_err {
                            self.backlog.push_back(HashData(list_path, list_hash));
                        }
                        return Err(audit_err);
                    }
                }
            }
        }
        Ok(false)
    }

    fn search_reader(&mut self, HashData(path, hash): &HashData) -> Option<Result<(), ReaderErr>> {
        loop {
            if is_canceled() {
                return Some(Ok(()));
            }
            break match self.read_next() {
                Err(err) => Some(Err(ReaderErr::Error(err))),
                Ok(None) => None,
                Ok(Some(HashData(reader_path, reader_hash))) => {
                    match compare_paths(&reader_path, path) {
                        Ok(_) => {
                            if &reader_hash == hash {
                                Some(Ok(()))
                            } else {
                                Some(Err(ReaderErr::Audit(AuditError::Mismatch(path_string(
                                    path,
                                )))))
                            }
                        }
                        Err(ComparedPath::Unrelated) => {
                            self.backlog.push_back(HashData(reader_path, reader_hash));
                            continue;
                        }
                        Err(ComparedPath::Audit(audit_err)) => {
                            if let AuditError::EmptyDir(_) = &audit_err {
                                self.backlog.push_back(HashData(reader_path, reader_hash));
                            }
                            Some(Err(ReaderErr::Audit(audit_err)))
                        }
                    }
                }
            };
        }
    }

    fn search(&mut self) -> Result<(), Error> {
        while let Some(hash_data) = self.source.pop_front() {
            if is_canceled() {
                return Ok(());
            }
            if !self.backlog.is_empty() {
                match self.search_backlog(&hash_data) {
                    Ok(true) => continue,
                    Ok(false) => (),
                    Err(err) => {
                        self.audit_err = true;
                        err.print_and_cancel(self.source.early)
                    }
                };
            }
            match self.search_reader(&hash_data) {
                Some(Ok(_)) => continue,
                Some(Err(err)) => match err {
                    ReaderErr::Error(err) => cancel_on_err(Err(err))?,
                    ReaderErr::Audit(err) => {
                        self.audit_err = true;
                        err.print_and_cancel(self.source.early);
                    }
                },
                None => {
                    self.audit_err = true;
                    AuditError::Extra(path_string(&hash_data.0))
                        .print_and_cancel(self.source.early);
                }
            };
        }
        Ok(())
    }

    pub fn check(
        &mut self,
        handles: &[ScopedJoinHandle<Result<(), Error>>],
        keep_state: bool,
    ) -> Result<bool, Error> {
        loop {
            let result = self.search();
            if handles.iter().all(|handle| !handle.is_finished()) {
                thread::sleep(Duration::from_millis(500));
                continue;
            }
            break result.map(|_| {
                if keep_state || self.backlog.is_empty() {
                    return self.audit_err;
                }
                for HashData(path, _) in self.backlog.drain(..) {
                    if is_canceled() {
                        return self.audit_err;
                    }
                    AuditError::NotFound(path_string(&path)).print_and_cancel(self.source.early);
                }
                true
            });
        }
    }
}
