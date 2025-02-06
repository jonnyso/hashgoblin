mod error;
mod exec;
mod hashing;
mod path;

use exec::{run, AuditSrc, MTReader, OutFile, STReader};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
};

pub use error::Error;
pub use hashing::HashType;

const DEFAULT_OUT: &str = "./hashes.txt";

type SourceReader = (Option<Mutex<MTReader>>, Option<Mutex<STReader>>);

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn get_reader(input: &[String], recursive: bool, max_threads: u8) -> Result<SourceReader, Error> {
    let paths = paths_from_input(input, recursive)?;
    let reader = STReader::new(paths, max_threads);
    Ok((None, Mutex::new(reader)))
}

fn get_reader(input: &[String], recursive: bool, max_threads: u8) -> Result<SourceReader, Error> {
    let (mt, st) = paths_from_input(input, recursive)?.into_iter().fold(
        (vec![], vec![]),
        |(mut mt, mut st), path| {
            match path::path_on_fast_drive(&path) {
                Ok(true) => mt.push(path),
                Ok(false) => st.push(path),
                Err(_) => {
                    eprintln!(
                        "WARNING: Could not detect if drive is an HDD, defaulting to Multithread reader"
                    );
                    mt.push(path);
                }
            };
            (mt, st)
        },
    );
    let mt = (!mt.is_empty()).then_some(Mutex::new(MTReader::new(mt)));
    let st = (!st.is_empty()).then_some(Mutex::new(STReader::new(st, max_threads)));
    Ok((mt, st))
}

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
    // let outfile = OutFile::new(output, &hash)?;
    // let queue = Queue::new(input, recursive)?;
    // thread::scope(|s| {
    //     let mut handles = Vec::with_capacity(max_threads as usize);
    //     while handles.len() < max_threads as usize {
    //         handles.push(s.spawn(|| run(&hash, &queue, empty_dirs, &outfile)));
    //     }
    //     let result = handles
    //         .into_iter()
    //         .map(|handle| handle.join().unwrap())
    //         .find(|handle| handle.is_err())
    //         .unwrap_or(Ok(()));
    //     if result.is_err() {
    //         if let Err(err) = fs::remove_file(outfile.path()) {
    //             eprintln!("Failed to clean up output file: {err}");
    //         }
    //     }
    //     result
    // })
    todo!()
}

pub fn audit(
    input: &[String],
    recursive: bool,
    max_threads: u8,
    hashes_file: Option<PathBuf>,
    early: bool,
    empty_dirs: bool,
) -> Result<(), Error> {
    // let (checkfile, hash, reader) = AuditSrc::new(hashes_file, early, empty_dirs)?;
    // let queue = Queue::new(input, recursive)?;
    // thread::scope(|s| {
    //     let mut handles = Vec::with_capacity(max_threads as usize);
    //     while handles.len() < max_threads as usize {
    //         handles.push(s.spawn(|| run(&hash, &queue, empty_dirs, &checkfile)));
    //     }
    //     let mut checker = checkfile.checker(reader);
    //     let audit_err = checker.check(&handles)?;
    //     if !audit_err {
    //         println!("ok");
    //     }
    //     handles
    //         .into_iter()
    //         .map(|handle| handle.join().unwrap())
    //         .find(|result| result.is_err())
    //         .unwrap_or(Ok(()))
    // })
    todo!()
}
