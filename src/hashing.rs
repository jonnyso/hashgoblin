use std::{
    fmt::Display,
    fs::File,
    io::{self, BufRead, BufReader},
    path::Path,
    str::FromStr,
    sync::atomic::{AtomicBool, Ordering},
};

use digest::DynDigest;

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

pub fn new_hasher(hash: &Hash) -> Box<dyn DynDigest> {
    match hash {
        Hash::MD5 => Box::new(md5::Md5::default()),
        Hash::SHA256 => Box::new(sha2::Sha256::default()),
        Hash::SHA1 => Box::new(sha1::Sha1::default()),
        Hash::Tiger => Box::new(tiger::Tiger::default()),
        Hash::Whirlpool => Box::new(whirlpool::Whirlpool::default()),
    }
}

pub enum Hashed {
    Value(String),
    Canceled,
}

pub fn hash_file(
    path: &Path,
    hasher: &mut dyn DynDigest,
    cancel: &AtomicBool,
) -> io::Result<Hashed> {
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
    Ok(Hashed::Value(hex::encode(hasher.finalize_reset())))
}
