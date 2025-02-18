mod error;
mod exec;
mod hashing;

use exec::{add_time_finish, run, AuditSrc, OutFile, Queue};
use std::{fs, path::PathBuf, thread};

pub use error::Error;
pub use hashing::HashType;

const DEFAULT_OUT: &str = "./hashes.txt";

pub fn create(
    input: &[String],
    recursive: bool,
    max_threads: u8,
    hash: HashType,
    output: Option<PathBuf>,
    empty_dirs: bool,
) -> Result<(), Error> {
    let outfile = OutFile::new(output, &hash)?;
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
        if let Err(err) = fs::remove_file(outfile.path()) {
            eprintln!("WARNING: Failed to clean up output file: {err}");
        }
    } else {
        if let Err(err) = add_time_finish(outfile.path()) {
            eprintln!("WARNING: Failed to include the time_finished into the hashes file: {err}");
        }
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
