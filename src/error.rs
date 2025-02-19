use std::{
    fmt::{Debug, Display},
    io,
};

pub enum Error {
    IsDir(String),
    Io((io::Error, String)),
    OutputRead(io::Error),
    OutputWrite(io::Error),
    OutputFinish(String),
    FileFormat,
    InvalidHash(String),
    ReadLine(io::Error),
    AuditEmptyDir(String),
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
            Self::OutputRead(error) => write!(f, "failed to read output file: {error}"),
            Self::OutputWrite(error) => write!(f, "failed to write to output file: {error}"),
            Self::OutputFinish(_) => write!(f, "hashing concluded successfully but the program failed to insert the finish time into the file"),
            Self::FileFormat => write!(f, "invalid hashes file format"),
            Self::InvalidHash(value) => write!(f, "{value}"),
            Self::ReadLine(error) => write!(f, "failed to read hashes file: {error}"),
            Self::AuditEmptyDir(path) => write!(
                f,
                "empty directory: {path}\n - Because the hashes file was created with `empty-dirs` option enabled, this option must also be enabled when auditing"
            ),
        }
    }
}

impl std::error::Error for Error {}
