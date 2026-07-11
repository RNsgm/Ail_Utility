use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::encoder::format::{
    index_offset_from_header, read_index_entries, AilEntry, AilHeader, CompressionType,
};
use crate::encoder::{EncoderError};

/// Декодированный .ail файл
pub struct AilDecoder {
    file: BufReader<File>,
    header: AilHeader,
    entries: Vec<AilEntry>,
}

impl AilDecoder {
    /// Открывает .ail файл для чтения
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, DecoderError> {
        let file = File::open(path)?;
        let mut reader = BufReader::with_capacity(128 * 1024, file);

        let header = AilHeader::read(&mut reader).map_err(DecoderError::from_encoder)?;
        reader.seek(SeekFrom::Start(index_offset_from_header(&header)))?;
        let entries = read_index_entries(&mut reader, header.entry_count)
            .map_err(DecoderError::from_encoder)?;

        Ok(Self {
            file: reader,
            header,
            entries,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get_entry(&self, index: usize) -> Option<&AilEntry> {
        self.entries.get(index)
    }

    pub fn entries(&self) -> &[AilEntry] {
        &self.entries
    }

    pub fn header(&self) -> &AilHeader {
        &self.header
    }

    /// Читает и проверяет данные записи по индексу
    pub fn read_image(&mut self, index: usize) -> Result<Vec<u8>, DecoderError> {
        let entry = self
            .entries
            .get(index)
            .ok_or(DecoderError::IndexOutOfBounds(index))?;

        self.file.seek(SeekFrom::Start(entry.data_offset))?;

        let mut stored = vec![0u8; entry.compressed_size as usize];
        self.file.read_exact(&mut stored)?;

        let decrypted = if self.header.encrypted == 1 {
            let key = crate::crypto::get_or_create_key();
            crate::crypto::decrypt(&stored, &key)
                .map_err(|e| DecoderError::Decompression(e.to_string()))?
        } else {
            stored
        };

        let data = decompress_entry(entry, &decrypted)?;

        let computed: [u8; 16] = md5::compute(&data).into();
        if computed != entry.md5_hash {
            return Err(DecoderError::IntegrityCheck {
                index,
                name: entry.original_name.clone(),
            });
        }

        Ok(data)
    }

    /// Читает встроенное превью (v2)
    pub fn read_thumbnail(&mut self) -> Result<Vec<u8>, DecoderError> {
        if self.header.thumbnail_size == 0 {
            return Err(DecoderError::InvalidFormat(
                "No thumbnail in this file".to_string(),
            ));
        }

        self.file
            .seek(SeekFrom::Start(self.header.thumbnail_offset))?;

        let mut thumb_header = [0u8; 20];
        self.file.read_exact(&mut thumb_header)?;

        let compressed_size = u64::from_le_bytes(thumb_header[12..20].try_into().unwrap());

        let mut data = vec![0u8; compressed_size as usize];
        self.file.read_exact(&mut data)?;

        let decrypted = if self.header.encrypted == 1 {
            let key = crate::crypto::get_or_create_key();
            crate::crypto::decrypt(&data, &key)
                .map_err(|e| DecoderError::Decompression(e.to_string()))?
        } else {
            data
        };

        zstd::decode_all(&decrypted[..]).map_err(|e| DecoderError::Decompression(e.to_string()))
    }
}

fn decompress_entry(entry: &AilEntry, data: &[u8]) -> Result<Vec<u8>, DecoderError> {
    match entry.compression {
        CompressionType::None => Ok(data.to_vec()),
        CompressionType::Zstd => {
            zstd::decode_all(data).map_err(|e| DecoderError::Decompression(e.to_string()))
        }
        CompressionType::Lz4 => {
            let mut decoder =
                lz4::Decoder::new(data).map_err(|e| DecoderError::Decompression(e.to_string()))?;
            let mut decompressed = Vec::with_capacity(entry.original_size as usize);
            std::io::copy(&mut decoder, &mut decompressed)?;
            Ok(decompressed)
        }
    }
}

/// Ошибки декодера
#[derive(Debug)]
pub enum DecoderError {
    Io(std::io::Error),
    InvalidFormat(String),
    IndexOutOfBounds(usize),
    Decompression(String),
    IntegrityCheck { index: usize, name: String },
}

impl std::fmt::Display for DecoderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecoderError::Io(e) => write!(f, "IO error: {e}"),
            DecoderError::InvalidFormat(s) => write!(f, "Invalid format: {s}"),
            DecoderError::IndexOutOfBounds(i) => write!(f, "Index out of bounds: {i}"),
            DecoderError::Decompression(s) => write!(f, "Decompression error: {s}"),
            DecoderError::IntegrityCheck { index, name } => {
                write!(f, "Integrity check failed for entry {index} ({name})")
            }
        }
    }
}

impl std::error::Error for DecoderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DecoderError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl DecoderError {
    fn from_encoder(error: EncoderError) -> Self {
        DecoderError::InvalidFormat(error.to_string())
    }
}

impl From<std::io::Error> for DecoderError {
    fn from(e: std::io::Error) -> Self {
        DecoderError::Io(e)
    }
}

/// Информация о .ail файле для галереи
#[derive(Debug, Clone)]
pub struct AilInfo {
    pub path: PathBuf,
    pub name: String,
    pub image_count: usize,
    pub preview_data: Option<Vec<u8>>,
}

impl AilInfo {
    /// Быстрая загрузка метаданных и превью
    pub fn from_path(path: PathBuf) -> Result<Self, DecoderError> {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();

        let mut decoder = AilDecoder::open(&path)?;
        let image_count = decoder.len();

        let preview_data = if decoder.header().thumbnail_size > 0 {
            decoder.read_thumbnail().ok()
        } else {
            None
        };

        Ok(Self {
            path,
            name,
            image_count,
            preview_data,
        })
    }
}
