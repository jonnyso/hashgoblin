mod error;
mod exec;
mod hashing;
mod path;

use exec::{reader_list, run, AuditSrc, ExecReader, OutFile, ReaderType};
use std::{fs, path::PathBuf, thread};

pub use error::Error;
pub use hashing::HashType;

const DEFAULT_OUT: &str = "./hashes.txt";

fn paths_from_input(input: &[String], recursive: bool) -> Result<Vec<PathBuf>, Error> {
    let mut paths = Vec::with_capacity(input.len());
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
        paths.push(pathbuf);
    }
    Ok(paths)
}

pub fn create(
    input: &[String],
    recursive: bool,
    max_threads: u8,
    hash: HashType,
    output: Option<PathBuf>,
    empty_dirs: bool,
) -> Result<(), Error> {
    let outfile = OutFile::new(output, &hash)?;
    let readers = reader_list(paths_from_input(input, recursive)?, max_threads);
    for reader in readers {
        thread::scope(|s| {
            let mut handles = Vec::with_capacity(max_threads as usize);
            while handles.len() < max_threads as usize {
                match &reader {
                    ReaderType::MT(r) => {
                        handles.push(s.spawn(|| run(&hash, r.get_handle(), empty_dirs, &outfile)))
                    }
                    ReaderType::ST(r) => {
                        handles.push(s.spawn(|| run(&hash, r.get_handle(), empty_dirs, &outfile)))
                    }
                }
            }
            let result = handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .find(|handle| handle.is_err())
                .unwrap_or(Ok(()));
            if result.is_err() {
                if let Err(err) = fs::remove_file(outfile.path()) {
                    eprintln!("Failed to clean up output file: {err}");
                }
                return result;
            }
            Ok(())
        })?;
    }
    Ok(())
}

pub fn audit(
    input: &[String],
    recursive: bool,
    max_threads: u8,
    hashes_file: Option<PathBuf>,
    early: bool,
    empty_dirs: bool,
) -> Result<(), Error> {
    let (checkfile, hash, hashes) = AuditSrc::new(hashes_file, early, empty_dirs)?;
    let readers = reader_list(paths_from_input(input, recursive)?, max_threads);
    let reader_count = readers.len();
    let mut checker = checkfile.checker(hashes);
    let mut audit_err = false;
    for (index, reader) in readers.into_iter().enumerate() {
        thread::scope(|s| {
            let mut handles = Vec::with_capacity(max_threads as usize);
            while handles.len() < max_threads as usize {
                match &reader {
                    ReaderType::MT(r) => {
                        handles.push(s.spawn(|| run(&hash, r.get_handle(), empty_dirs, &checkfile)))
                    }
                    ReaderType::ST(r) => {
                        handles
                            .push(s.spawn(|| run(&hash, r.get_handle(), empty_dirs, &checkfile)));
                    }
                }
            }
            let has_error = checker.check(&handles, index + 1 < reader_count)?;
            if !audit_err && has_error {
                audit_err = true;
            }
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .find(|result| result.is_err())
                .unwrap_or(Ok(()))
        })?;
    }
    if !audit_err {
        println!("ok");
    }
    Ok(())
}
