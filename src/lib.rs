use std::{
    collections::VecDeque,
    fmt::{Debug, Display},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Lines, Write},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread::{self, ScopedJoinHandle},
    time::Duration,
};

use digest::DynDigest;
use hex::encode;

pub enum Hash {
    MD5,
    SHA256,
    SHA1,
    Tiger,
    Whirlpool,
}

impl FromStr for Hash {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "md5" => Ok(Hash::MD5),
            "sha1" => Ok(Hash::SHA1),
            "sha256" => Ok(Hash::SHA256),
            "tiger" => Ok(Hash::Tiger),
            "whirlpool" => Ok(Hash::Whirlpool),
            _ => Err(format!(
                "invalid hash: {s}, possible options are: sha256, tiger, whirlpool, sha1, md5"
            )),
        }
    }
}

impl Display for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Hash::MD5 => "md5",
                Hash::SHA256 => "sha256",
                Hash::SHA1 => "sha1",
                Hash::Tiger => "tiger",
                Hash::Whirlpool => "whirlpool",
            }
        )
    }
}

pub enum Error {
    IsDir(String),
    Io((io::Error, String)),
    Output(io::Error),
    FileFormat,
    InvalidHash(String),
    ReadLine(io::Error),
}

impl Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self, f)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IsDir(path) => write!(
                f,
                "`{path}` is a directory, try running with --recursive flag"
            ),
            Self::Io((error, path)) => {
                write!(f, "trying to read file or directory `{path}`: {error}",)
            }
            Self::Output(error) => write!(f, "failed to create output file: {error}"),
            Self::FileFormat => write!(f, "invalid hashes file format"),
            Self::InvalidHash(value) => write!(f, "{value}"),
            Self::ReadLine(error) => write!(f, "failed to read hashes file: {error}"),
        }
    }
}

impl std::error::Error for Error {}

fn new_hasher(hash: &Hash) -> Box<dyn DynDigest> {
    match hash {
        Hash::MD5 => Box::new(md5::Md5::default()),
        Hash::SHA256 => Box::new(sha2::Sha256::default()),
        Hash::SHA1 => Box::new(sha1::Sha1::default()),
        Hash::Tiger => Box::new(tiger::Tiger::default()),
        Hash::Whirlpool => Box::new(whirlpool::Whirlpool::default()),
    }
}

enum Hashed {
    Value(String),
    Canceled,
}

fn hash_file(path: &Path, hasher: &mut dyn DynDigest, cancel: &AtomicBool) -> io::Result<Hashed> {
    let mut reader = BufReader::new(File::open(path)?);
    loop {
        if cancel.load(Ordering::Acquire) {
            return Ok(Hashed::Canceled);
        }
        let data = reader.fill_buf()?;
        if data.is_empty() {
            break;
        }
        let length = data.len();
        hasher.update(data);
        reader.consume(length);
    }
    Ok(Hashed::Value(encode(hasher.finalize_reset())))
}

struct Queue(Mutex<VecDeque<PathBuf>>);

impl Queue {
    fn new(input: &[String], recursive: bool) -> Result<Self, Error> {
        let mut queue = VecDeque::with_capacity(input.len());
        for path in input {
            let pathbuf = PathBuf::from_str(path).unwrap(); // Infallible
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

    fn push_dir(&self, path: &Path, cancel: &AtomicBool) -> Result<(), Error> {
        let mut queue = self.0.lock().unwrap();
        let reader = path
            .read_dir()
            .map_err(|err| Error::Io((err, path.to_string_lossy().to_string())))?;
        for entry in reader {
            if cancel.load(Ordering::Acquire) {
                return Ok(());
            }
            queue.push_back(
                entry
                    .map_err(|err| Error::Io((err, path.to_string_lossy().to_string())))?
                    .path(),
            );
        }
        Ok(())
    }
}

struct OutFile(Mutex<BufWriter<File>>);

impl OutFile {
    fn new(path: Option<&Path>, hash: &Hash) -> Result<Self, Error> {
        let path = path
            .map(|p| p.to_owned())
            .unwrap_or(PathBuf::from_str("./hashes.txt").unwrap()); // Infallible
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(Error::Output)?;
        let mut writer = BufWriter::new(file);
        let version = env!("CARGO_PKG_VERSION");
        writer
            .write_all(format!("version {version}\nalgo {hash}\n").as_bytes())
            .map_err(Error::Output)?;
        Ok(Self(Mutex::new(writer)))
    }

    fn write_line(
        &self,
        path: &Path,
        hasher: &mut dyn DynDigest,
        cancel: &AtomicBool,
    ) -> Result<(), Error> {
        let hashed = match hash_file(&path, hasher, cancel) {
            Ok(Hashed::Value(value)) => value,
            Ok(Hashed::Canceled) => return Ok(()),
            Err(err) => return Err(Error::Io((err, path.to_string_lossy().to_string()))),
        };
        let line = format!("{}|{}\n", path.to_string_lossy(), hashed);
        self.0
            .lock()
            .unwrap()
            .write_all(line.as_bytes())
            .map_err(Error::Output)
    }
}

fn cancel_on_err<T, E>(result: Result<T, E>, cancel: &AtomicBool) -> Result<T, E> {
    if result.is_err() {
        cancel.store(true, Ordering::Release);
    }
    result
}

fn run(
    path: PathBuf,
    hash: &Hash,
    queue: &Queue,
    writer: &OutFile,
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let mut path = Some(path);
    let mut hasher = new_hasher(&hash);
    while let Some(path) = path.take().or_else(|| queue.pop_front()) {
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        if path.is_dir() {
            cancel_on_err(queue.push_dir(&path, &cancel), cancel)?;
        } else {
            cancel_on_err(writer.write_line(&path, &mut *hasher, cancel), cancel)?;
        }
    }
    Ok(())
}

fn err_cleanup(
    handles: Vec<ScopedJoinHandle<Result<(), Error>>>,
    output: Option<PathBuf>,
) -> Result<(), Error> {
    let result = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .find(|result| result.is_err())
        .unwrap_or(Ok(()));
    let outfile = output.unwrap_or(PathBuf::from_str("./hashes.txt").unwrap());
    if let Err(err) = fs::remove_file(outfile) {
        eprintln!("Failed to clean up output file: {err}");
    }
    result
}

pub fn create_hashes(
    input: &[String],
    recursive: bool,
    max_threads: u8,
    hash: Hash,
    output: Option<PathBuf>,
) -> Result<(), Error> {
    let queue = Queue::new(input, recursive)?;
    let writer = OutFile::new(output.as_deref(), &hash)?;
    let cancel = AtomicBool::new(false);
    thread::scope(|s| -> Result<(), Error> {
        let max_threads = max_threads - 1;
        let mut handles = Vec::with_capacity(max_threads as usize);
        let mut hasher = new_hasher(&hash);
        loop {
            while let Some(path) = queue.pop_front() {
                if cancel.load(Ordering::Acquire) {
                    return err_cleanup(handles, output);
                }
                if handles.len() < max_threads as usize {
                    handles.push(s.spawn(|| run(path, &hash, &queue, &writer, &cancel)));
                    continue;
                }
                if path.is_dir() {
                    queue.push_dir(&path, &cancel)?;
                } else {
                    writer.write_line(&path, &mut *hasher, &cancel)?;
                }
            }
            if handles.iter().all(|handle| handle.is_finished()) {
                return Ok(());
            } else {
                thread::sleep(Duration::from_millis(500));
            }
        }
    })
}

enum AuditError<T> {
    NotFound(T),
    Mismatch(T),
}

impl<T: Display> AuditError<T> {
    fn handle(&self, early: bool, cancel: &AtomicBool) {
        match self {
            AuditError::NotFound(path) => println!("err: \"{path}\" not found "),
            AuditError::Mismatch(path) => println!("err: \"{path}\" does not match"),
        };
        if early {
            cancel.store(true, Ordering::Release);
        }
    }
}

fn check(
    lines: &Mutex<Lines<BufReader<File>>>,
    hash: &Hash,
    early: bool,
    cancel: &AtomicBool,
) -> Result<bool, Error> {
    let mut hasher = new_hasher(hash);
    let mut audit_failed = false;
    loop {
        if cancel.load(Ordering::Acquire) {
            break Ok(audit_failed);
        }
        let next_line = {
            match lines.lock().unwrap().next() {
                Some(result) => cancel_on_err(result, cancel).map_err(Error::ReadLine)?,
                None => break Ok(audit_failed),
            }
        };
        let (path, hash_str) = next_line
            .rsplit_once('|')
            .map(|(s, v)| (PathBuf::from_str(s).unwrap(), v))
            .ok_or(Error::FileFormat)?;
        match hash_file(&path, &mut *hasher, cancel) {
            Ok(Hashed::Canceled) => break Ok(audit_failed),
            Ok(Hashed::Value(new_hash_str)) => {
                if hash_str != new_hash_str {
                    AuditError::Mismatch(path.to_string_lossy()).handle(early, cancel);
                    audit_failed = true;
                }
            }
            Err(err) => match err.kind() {
                io::ErrorKind::NotFound => {
                    AuditError::NotFound(path.to_string_lossy()).handle(early, cancel);
                    audit_failed = true;
                }
                _ => {
                    break cancel_on_err(
                        Err(Error::Io((err, path.to_string_lossy().to_string()))),
                        cancel,
                    );
                }
            },
        };
    }
}

pub fn audit(path: Option<PathBuf>, max_threads: u8, early: bool) -> Result<(), Error> {
    let path = path.unwrap_or(PathBuf::from_str("./hashes.txt").unwrap());
    let file =
        File::open(&path).map_err(|err| Error::Io((err, path.to_string_lossy().to_string())))?;
    let mut lines = BufReader::new(file).lines();
    match lines.next() {
        None => return Err(Error::FileFormat),
        Some(Err(err)) => return Err(Error::ReadLine(err)),
        Some(Ok(line)) => match line.split_once(char::is_whitespace) {
            Some(("version", version)) => {
                let current = env!("CARGO_PKG_VERSION");
                if current != version {
                    eprintln!("WARNING: the hashes file was created using a different version of this program: current version {current}, file version {version}");
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
    let lines = Mutex::new(lines);
    let cancel = AtomicBool::new(false);
    thread::scope(|s| -> Result<(), Error> {
        let mut handles = Vec::with_capacity(max_threads as usize);
        for _ in 0..max_threads {
            handles.push(s.spawn(|| check(&lines, &hash, early, &cancel)));
        }
        let mut handles = handles.into_iter().map(|handle| handle.join().unwrap());
        if let Some(result) = handles.find(|result| result.is_err()) {
            return Err(result.unwrap_err());
        }
        if !handles.any(|result| result.unwrap()) {
            println!("ok");
        }
        Ok(())
    })
}
