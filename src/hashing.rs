use std::{fmt::Display, str::FromStr};

use digest::DynDigest;

pub enum HashType {
    MD5,
    SHA256,
    SHA1,
    Tiger,
    Whirlpool,
}

impl FromStr for HashType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "md5" => Ok(HashType::MD5),
            "sha1" => Ok(HashType::SHA1),
            "sha256" => Ok(HashType::SHA256),
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
        write!(
            f,
            "{}",
            match self {
                HashType::MD5 => "md5",
                HashType::SHA256 => "sha256",
                HashType::SHA1 => "sha1",
                HashType::Tiger => "tiger",
                HashType::Whirlpool => "whirlpool",
            }
        )
    }
}

pub fn new_hasher(hash: &HashType) -> Box<dyn DynDigest> {
    match hash {
        HashType::MD5 => Box::new(md5::Md5::default()),
        HashType::SHA256 => Box::new(sha2::Sha256::default()),
        HashType::SHA1 => Box::new(sha1::Sha1::default()),
        HashType::Tiger => Box::new(tiger::Tiger::default()),
        HashType::Whirlpool => Box::new(whirlpool::Whirlpool::default()),
    }
}
