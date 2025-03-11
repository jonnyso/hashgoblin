use std::{
    collections::VecDeque,
    fmt::Display,
    fs::File,
    io::{BufRead, BufReader, Lines},
    path::{Path, PathBuf},
    str::FromStr,
    sync::mpsc::{Receiver, Sender},
};

use jiff::civil::Date;

use crate::{DEFAULT_OUT, Error, HashType, exec::cancel, verbose_print};

use super::{
    HASH_ALGO_STR, HashData, HashHandler, NO_DATE_STR, TIME_FINISH_STR, TIME_START_STR,
    VERSION_STR, cancel_on_err, is_canceled, path_string,
};

enum AuditError {
    NotFound(String),
    Mismatch(String),
    Extra(String),
    EmptyDir(String),
}

impl Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditError::NotFound(path) => write!(f, "audit_err: \"{path}\" not found"),
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
        verbose_print(|| self, false);
        if early {
            cancel();
        }
    }
}

type HashesFile = Lines<BufReader<File>>;

pub fn load_check_file(path: Option<PathBuf>) -> Result<(HashesFile, Vec<HashType>), Error> {
    verbose_print(|| "loading check file", true);
    let path = path.unwrap_or(PathBuf::from(DEFAULT_OUT));
    let file = File::open(&path).map_err(|err| Error::Io((err, path_string(&path))))?;
    let mut lines = BufReader::new(file).lines();
    match lines.next() {
        None => return Err(Error::FileFormat),
        Some(Err(err)) => return Err(Error::ReadLine(err)),
        Some(Ok(line)) => match line.rsplit_once(char::is_whitespace) {
            Some((VERSION_STR, version)) => {
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
    let hashes = match lines.next() {
        None => return Err(Error::FileFormat),
        Some(Err(err)) => return Err(Error::ReadLine(err)),
        Some(Ok(line)) => match line.split_once(char::is_whitespace) {
            Some((HASH_ALGO_STR, hashes)) => {
                let mut hash_list = vec![];
                for hash in hashes.split(',') {
                    hash_list.push(HashType::from_str(hash).map_err(Error::InvalidHash)?);
                }
                hash_list
            }
            _ => return Err(Error::FileFormat),
        },
    };
    match lines.next() {
        None => return Err(Error::FileFormat),
        Some(Err(err)) => return Err(Error::ReadLine(err)),
        Some(Ok(line)) => {
            let (start, finish) = line.split_once(" - ").ok_or(Error::FileFormat)?;
            match start.split_once(char::is_whitespace) {
                Some((TIME_START_STR, NO_DATE_STR)) => (),
                Some((TIME_START_STR, time_start)) => {
                    time_start.parse::<Date>().map_err(|_| Error::FileFormat)?;
                }
                _ => return Err(Error::FileFormat),
            }
            match finish.split_once(char::is_whitespace) {
                Some((TIME_FINISH_STR, NO_DATE_STR)) => (),
                Some((TIME_FINISH_STR, time_finish)) => {
                    time_finish.parse::<Date>().map_err(|_| Error::FileFormat)?;
                }
                _ => return Err(Error::FileFormat),
            }
        }
    };
    Ok((lines, hashes))
}

enum ComparedPath {
    Audit(AuditError),
    Unrelated,
}

fn compare_paths(reader_path: &Path, path: &Path) -> Result<(), ComparedPath> {
    verbose_print(
        || format!("comparing path {:?} to {:?}", reader_path, path),
        true,
    );
    if reader_path == path {
        return Ok(());
    }
    // This assumes that this program implementation cannot create a hashes file
    // describing the same directory being empty and filled at the same time, i.e.:
    // both `/dir|` and `/dir|file.txt` simultaneausly.
    // If the hashes file has been altered or malformed, the auditing process may return
    // an incorrect result.
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

impl HashHandler for Sender<HashData> {
    fn handle(&self, hash_data: HashData) -> Result<(), Error> {
        self.send(hash_data).unwrap();
        Ok(())
    }
}

pub struct Checker {
    source: Receiver<HashData>,
    reader: Lines<BufReader<File>>,
    backlog: VecDeque<HashData>,
    audit_err: bool,
    early: bool,
    empty_dirs: bool,
}

impl Checker {
    pub fn new(
        reader: HashesFile,
        source: Receiver<HashData>,
        early: bool,
        empty_dirs: bool,
    ) -> Self {
        Self {
            source,
            reader,
            backlog: VecDeque::with_capacity(100),
            audit_err: false,
            early,
            empty_dirs,
        }
    }

    fn read_next(&mut self) -> Result<Option<HashData>, Error> {
        let next_line = match self.reader.next() {
            Some(result) => result.map_err(Error::ReadLine)?,
            None => return Ok(None),
        };
        verbose_print(
            || format!("reading next line from hashes file: {:?}", &next_line),
            true,
        );
        HashData::try_from_string(next_line, self.empty_dirs).map(Some)
    }

    fn search_backlog(&mut self, hd @ HashData(path, hash): &HashData) -> Result<bool, AuditError> {
        verbose_print(|| format!("searching {hd} in backlog"), true);
        let len = self.backlog.len();
        for _ in 0..len {
            if is_canceled() {
                return Ok(false);
            }
            if let Some(HashData(list_path, list_hash)) = self.backlog.pop_front() {
                match compare_paths(&list_path, path) {
                    Ok(_) => {
                        verbose_print(|| format!("found {hd} in backlog"), true);
                        return if &list_hash == hash {
                            Ok(true)
                        } else {
                            Err(AuditError::Mismatch(path_string(path)))
                        };
                    }
                    Err(ComparedPath::Unrelated) => {
                        verbose_print(|| format!("pushing {hd} to backlog"), true);
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

    fn search_reader(
        &mut self,
        hd @ HashData(path, hash): &HashData,
    ) -> Option<Result<(), ReaderErr>> {
        verbose_print(|| format!("searching {hd} on hashes file"), true);
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
                            verbose_print(|| format!("found {hd} in hashes file"), true);
                            if &reader_hash == hash {
                                Some(Ok(()))
                            } else {
                                Some(Err(ReaderErr::Audit(AuditError::Mismatch(path_string(
                                    path,
                                )))))
                            }
                        }
                        Err(ComparedPath::Unrelated) => {
                            verbose_print(|| format!("pushing {hd} to backlog"), true);
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
        verbose_print(|| "begin search", true);
        while let Ok(hash_data) = self.source.recv() {
            if is_canceled() {
                return Ok(());
            }
            if !self.backlog.is_empty() {
                match self.search_backlog(&hash_data) {
                    Ok(true) => continue,
                    Ok(false) => (),
                    Err(err) => {
                        self.audit_err = true;
                        err.print_and_cancel(self.early)
                    }
                };
            }
            match self.search_reader(&hash_data) {
                Some(Ok(_)) => continue,
                Some(Err(err)) => match err {
                    ReaderErr::Error(err) => cancel_on_err(Err(err))?,
                    ReaderErr::Audit(err) => {
                        self.audit_err = true;
                        err.print_and_cancel(self.early);
                    }
                },
                None => {
                    self.audit_err = true;
                    AuditError::Extra(path_string(&hash_data.0)).print_and_cancel(self.early);
                }
            };
        }
        self.flush_reader()
    }

    fn flush_reader(&mut self) -> Result<(), Error> {
        while let Some(hash_data) = self.read_next()? {
            self.backlog.push_back(hash_data);
        }
        Ok(())
    }

    pub fn check(&mut self) -> Result<bool, Error> {
        self.search()?;
        if self.backlog.is_empty() {
            verbose_print(|| "search done, backlog is empty", true);
            return Ok(self.audit_err);
        }
        verbose_print(|| "search done, backlog not empty", true);
        for HashData(path, _) in self.backlog.drain(..) {
            if is_canceled() {
                return Ok(self.audit_err);
            }
            AuditError::NotFound(path_string(&path)).print_and_cancel(self.early)
        }
        Ok(true)
    }
}
