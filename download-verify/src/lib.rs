use std::io::{Read, Write};

use sha2::Digest;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DlError {
    #[error("ureq error")]
    UreqError,
    #[error("IO error")]
    IOError(#[from] std::io::Error),
    #[error("Hash mismatch (expected: {expected}, found: {found}")]
    HashMismatch { expected: String, found: String },
    #[error("Hex decode error")]
    HexDecodeError(#[from] hex::FromHexError),
}

pub enum Hash {
    Sha256(String),
}

/// Download a file to disk if neccessary and validate it.
///
/// Currently, this is done in one big chuck and thus enough memory
/// is necessary to load the entire file. A future update could
/// read individual chunks and thus reduce the memory footprint.
pub fn download_verify<P: AsRef<std::path::Path>>(
    url: &str,
    dest: P,
    hash: &Hash,
) -> Result<(), DlError> {
    // If the file already exists,
    if dest.as_ref().exists() {
        // // read it,
        // let mut bytes: Vec<u8> = Vec::new();
        // std::fs::File::open(dest)?.read_to_end(&mut bytes)?;
        // // and validate that it matches the checksum.
        // validate(&bytes, &hash)?;
    } else {
        // If the file does not exist, download the contents,
        let response = ureq::get(url)
            .timeout_connect(10_000) // max 10 seconds
            .call();

        if response.error() {
            return Err(DlError::UreqError);
        };

        let mut rdr = response.into_reader();

        let mut bytes = vec![];
        rdr.read_to_end(&mut bytes)?;

        // validate them,
        validate(bytes.as_ref(), &hash)?;
        // and save them to disk.
        let mut fd = std::fs::File::create(dest)?;
        fd.write(bytes.as_ref())?;
        fd.sync_all()?;
    }
    Ok(())
}

fn validate(bytes: &[u8], hash: &Hash) -> Result<(), DlError> {
    match hash {
        &Hash::Sha256(ref sum) => {
            let expected = hex::decode(sum.as_bytes())?;
            let digest = sha2::Sha256::digest(bytes);
            if &digest[..] == expected.as_slice() {
                Ok(())
            } else {
                let found = format!("{:x}", digest);
                Err(DlError::HashMismatch {
                    expected: sum.clone(),
                    found,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn it_works() {
        crate::download_verify(
            "https://ajax.googleapis.com/ajax/libs/jquery/1.12.4/jquery.min.js",
            "jquery.min.js",
            &crate::Hash::Sha256(
                "668b046d12db350ccba6728890476b3efee53b2f42dbb84743e5e9f1ae0cc404".into(),
            ),
        )
        .unwrap();
    }
}
