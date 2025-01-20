use crate::{hashing::Hash, Error, DEFAULT_OUT};
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Lines, Write},
    path::PathBuf,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread::ScopedJoinHandle,
};

use super::HashHandler;

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
    fn handle(&self, path: PathBuf, hash: String) -> Result<(), Error> {
        let line = format!("{}|{}\n", path.to_string_lossy(), hash);
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
}

impl AuditError {
    fn handle(&self, early: bool, cancel: &AtomicBool) {
        match self {
            AuditError::NotFound(path) => println!("audit_err: \"{path}\" not found "),
            AuditError::Mismatch(path) => println!("audit_err: \"{path}\" does not match"),
            AuditError::Extra(path) => {
                println!("audit_err: aditional \"{path}\" file found in audit source")
            }
        };
        if early {
            cancel.store(true, Ordering::Release);
        }
    }
}

type HashData = (PathBuf, String);

struct Checker<'a> {
    backlog: Mutex<VecDeque<HashData>>,
    reader: Lines<BufReader<File>>,
    checklist: VecDeque<HashData>,
    cancel: &'a AtomicBool,
    early: bool,
}

impl<'a> Checker<'a> {
    pub fn new(
        path: Option<PathBuf>,
        cancel: &'a AtomicBool,
        early: bool,
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
            checklist: VecDeque::new(),
            cancel,
            early,
        };
        Ok((checker, hash))
    }

    fn push_to_backlog(&self, hash_data: HashData) {
        self.backlog.lock().unwrap().push_back(hash_data);
    }

    fn from_backlog(&self) -> Option<HashData> {
        self.backlog.lock().unwrap().pop_front()
    }

    fn from_reader(&mut self) -> Result<Option<HashData>, Error> {
        let next_line = match self.reader.next() {
            Some(result) => result.map_err(Error::ReadLine)?,
            None => return Ok(None),
        };
        next_line
            .rsplit_once('|')
            .map(|(s, v)| Some((PathBuf::from(s), v.to_owned())))
            .ok_or(Error::FileFormat)
    }

    pub fn check(&mut self) -> bool {
        let mut audit_err = false;
        while let Some((path, hash)) = self.from_backlog() {}
        audit_err
    }
}

impl HashHandler for Checker<'_> {
    fn handle(&self, path: PathBuf, hash: String) -> Result<(), Error> {
        self.push_to_backlog((path, hash));
        Ok(())
    }
}
