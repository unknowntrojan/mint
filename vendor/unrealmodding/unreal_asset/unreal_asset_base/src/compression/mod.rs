//! Unreal decompression

#[cfg(feature = "flate2")]
use {
    flate2::bufread::{GzDecoder, ZlibDecoder},
    std::io::Read,
};

use crate::Error;

/// Compression method
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub enum CompressionMethod {
    /// None
    #[default]
    None,
    /// Zlib compression
    #[cfg(feature = "flate2")]
    Zlib,
    /// Gzip compression
    #[cfg(feature = "flate2")]
    Gzip,
    /// Lz4 compression
    #[cfg(feature = "lz4")]
    Lz4,
    /// Unknown compression format
    Unknown(Box<str>),
}

impl CompressionMethod {
    /// Create a new `CompressionMethod` from the method name
    pub fn new(name: &str) -> Self {
        match name {
            "None" => Self::None,
            #[cfg(feature = "flate2")]
            "Zlib" => Self::Zlib,
            #[cfg(feature = "flate2")]
            "Gzip" => Self::Gzip,
            #[cfg(feature = "lz4")]
            "LZ4" => Self::Lz4,
            _ => Self::Unknown(name.to_string().into_boxed_str()),
        }
    }
}

impl ToString for CompressionMethod {
    fn to_string(&self) -> String {
        match self {
            CompressionMethod::None => String::from("None"),
            #[cfg(feature = "flate2")]
            CompressionMethod::Zlib => String::from("Zlib"),
            #[cfg(feature = "flate2")]
            CompressionMethod::Gzip => String::from("Gzip"),
            #[cfg(feature = "lz4")]
            CompressionMethod::Lz4 => String::from("LZ4"),
            CompressionMethod::Unknown(e) => e.to_string(),
        }
    }
}

/// Decompress data with the given compression method
pub fn decompress(
    method: CompressionMethod,
    compressed: &[u8],
    decompressed: &mut [u8],
) -> Result<(), Error> {
    match method {
        CompressionMethod::None => {
            decompressed.copy_from_slice(&compressed[..decompressed.len()]);
            Ok(())
        }
        #[cfg(feature = "flate2")]
        CompressionMethod::Zlib => Ok(ZlibDecoder::new(compressed).read_exact(decompressed)?),
        #[cfg(feature = "flate2")]
        CompressionMethod::Gzip => Ok(GzDecoder::new(compressed).read_exact(decompressed)?),
        #[cfg(feature = "lz4")]
        CompressionMethod::Lz4 => {
            lz4_flex::block::decompress_into(compressed, decompressed)?;
            Ok(())
        }
        CompressionMethod::Unknown(name) => Err(Error::UnknownCompressionMethod(name)),
    }
}
