mod error;
mod exec;
mod hashing;

use exec::{AuditSrc, OutFile, Queue, run};
use std::{
    fmt::Display,
    fs,
    path::PathBuf,
    sync::OnceLock,
    thread::{self},
};

pub use error::Error;
pub use hashing::HashType;

const DEFAULT_OUT: &str = "./hashes.txt";

pub static VERBOSE: OnceLock<bool> = OnceLock::new();

pub fn verbose_init(value: bool) {
    VERBOSE.set(value).expect("OnceLock cell should be empty");
}

pub fn verbose_print<M: Display>(message: M, is_verbose: bool) {
    let verbose = *VERBOSE.get().unwrap();
    if verbose || !is_verbose {
        if verbose {
            let thread_id = thread::current().id();
            println!("{:?}: {message}", thread_id);
        } else {
            println!("{message}");
        }
    }
}

pub fn create(
    input: &[String],
    recursive: bool,
    max_threads: u8,
    mut hashes: Vec<HashType>,
    output: Option<PathBuf>,
    empty_dirs: bool,
) -> Result<(), Error> {
    if hashes.is_empty() {
        hashes.push(HashType::Sha256);
    }
    let path = output.unwrap_or_else(|| PathBuf::from(DEFAULT_OUT));
    let outfile = OutFile::new(&path, &hashes)?;
    let queue = Queue::new(input, recursive)?;
    let result = thread::scope(|s| {
        let mut handles = Vec::with_capacity(max_threads as usize);
        while handles.len() < max_threads as usize {
            handles.push(s.spawn(|| run(&hashes, &queue, empty_dirs, &outfile)));
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
