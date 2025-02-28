use std::{
    fmt::Display,
    fs::File,
    io::{self, BufRead, BufReader},
    path::Path,
    str::FromStr,
};

use digest::DynDigest;

use crate::exec::is_canceled;

#[derive(Debug)]
pub enum HashType {
    Md5,
    Sha256,
    Sha1,
    Tiger,
    Whirlpool,
}

impl HashType {
    pub fn as_str(&self) -> &str {
        match self {
            HashType::Md5 => "md5",
            HashType::Sha256 => "sha256",
            HashType::Sha1 => "sha1",
            HashType::Tiger => "tiger",
            HashType::Whirlpool => "whirlpool",
        }
    }
}

impl FromStr for HashType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "md5" => Ok(HashType::Md5),
            "sha1" => Ok(HashType::Sha1),
            "sha256" => Ok(HashType::Sha256),
            "tiger" => Ok(HashType::Tiger),
            "whirlpool" => Ok(HashType::Whirlpool),
            _ => Err(format!(
                "invalid hash: {s}, possible options are: sha256, tiger, whirlpool, sha1, md5"
            )),
        }
    }
}

impl Display for HashType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

pub fn new_hasher(hash: &HashType) -> Box<dyn DynDigest> {
    match hash {
        HashType::Md5 => Box::new(md5::Md5::default()),
        HashType::Sha256 => Box::new(sha2::Sha256::default()),
        HashType::Sha1 => Box::new(sha1::Sha1::default()),
        HashType::Tiger => Box::new(tiger::Tiger::default()),
        HashType::Whirlpool => Box::new(whirlpool::Whirlpool::default()),
    }
}

pub enum Hashed {
    Value(String),
    Canceled,
}

pub fn hash_file(path: &Path, hasher: &mut dyn DynDigest) -> io::Result<Hashed> {
    let mut reader = BufReader::new(File::open(path)?);
    loop {
        if is_canceled() {
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
