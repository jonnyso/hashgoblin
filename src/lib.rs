mod error;
mod exec;
mod hashing;

use exec::OutFile;
use std::path::PathBuf;

pub use error::Error;
pub use hashing::Hash;

const DEFAULT_OUT: &'static str = "./hashes.txt";

pub fn create(
    input: &[String],
    recursive: bool,
    max_threads: u8,
    hash: Hash,
    output: Option<PathBuf>,
    empty_dirs: bool,
) -> Result<(), Error> {
    let outfile = OutFile::new(output, &hash)?;
    exec::create_hashes(input, recursive, max_threads, hash, empty_dirs, outfile)
}
