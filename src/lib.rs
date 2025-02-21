mod error;
mod exec;
mod hashing;

use exec::{AuditSrc, OutFile, Queue, run};
use std::{
    fmt::Display,
    fs,
    io::{self, BufWriter, Stdout, Write},
    path::PathBuf,
    sync::{Mutex, OnceLock},
    thread::{self},
};

pub use error::Error;
pub use hashing::HashType;

const DEFAULT_OUT: &str = "./hashes.txt";

pub static STDOUT_BUF: OnceLock<StdoutBuf> = OnceLock::new();

#[derive(Debug)]
pub struct StdoutBuf {
    verbose: bool,
    writer: Mutex<BufWriter<Stdout>>,
}

impl StdoutBuf {
    pub fn new(verbose: bool) -> Self {
        Self {
            verbose,
            writer: Mutex::new(BufWriter::new(io::stdout())),
        }
    }

    pub fn print<M: Display>(&self, message: M, verbose: bool) {
        if self.verbose || !verbose {
            let thread_id = thread::current().id();
            let mut writer = self.writer.lock().unwrap();
            let result = if self.verbose {
                write!(&mut writer, "{:?}: {message}\n", thread_id)
            } else {
                write!(&mut writer, "{message}\n")
            };
            if let Err(err) = result {
                eprintln!("failed to print message: {err}");
            }
        }
    }
}

pub fn message_out<M: Display>(message: M, verbose: bool) {
    STDOUT_BUF
        .get()
        .expect("OnceLock cell should already be set")
        .print(message, verbose);
}

pub fn stdout_buf_init(verbose: bool) {
    STDOUT_BUF
        .set(StdoutBuf::new(verbose))
        .expect("OnceLock cell should be empty");
}

pub fn create(
    input: &[String],
    recursive: bool,
    max_threads: u8,
    hash: HashType,
    output: Option<PathBuf>,
    empty_dirs: bool,
) -> Result<(), Error> {
    let path = output
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_OUT));
    let outfile = OutFile::new(&path, &hash)?;
    let queue = Queue::new(input, recursive)?;
    let result = thread::scope(|s| {
        let mut handles = Vec::with_capacity(max_threads as usize);
        while handles.len() < max_threads as usize {
            handles.push(s.spawn(|| run(&hash, &queue, empty_dirs, &outfile)));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .find(|handle| handle.is_err())
            .unwrap_or(Ok(()))
    });
    if result.is_err() {
        if let Err(err) = fs::remove_file(&path) {
            eprintln!("WARNING: Failed to clean up output file: {err}");
        }
    } else {
        outfile.finish()?;
        println!("done")
    }
    result
}

pub fn audit(
    input: &[String],
    recursive: bool,
    max_threads: u8,
    hashes_file: Option<PathBuf>,
    early: bool,
    empty_dirs: bool,
) -> Result<(), Error> {
    let (checkfile, hash, reader) = AuditSrc::new(hashes_file, early, empty_dirs)?;
    let queue = Queue::new(input, recursive)?;
    let audit_err = thread::scope(|s| {
        let mut handles = Vec::with_capacity(max_threads as usize);
        while handles.len() < max_threads as usize {
            handles.push(s.spawn(|| run(&hash, &queue, empty_dirs, &checkfile)));
        }
        let mut checker = checkfile.checker(reader);
        let audit_err = checker.check(&handles)?;
        let err = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .find(|result| result.is_err());
        match err {
            Some(result) => Err(result.err().unwrap()),
            None => Ok(audit_err),
        }
    })?;
    if !audit_err {
        println!("ok");
    }
    Ok(())
}
