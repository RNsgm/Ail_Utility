use std::fs::File;
use std::io::{Read, Seek, SeekFrom, BufReader};
use std::path::Path;

use crate::encoder::format::{AilHeader, AilEntry, CompressionType, EntryType};
use crate::encoder::{EncoderError, AIL_MAGIC};

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
        
        // Читаем заголовок
        let header = AilHeader::read(&mut reader).map_err(|e| DecoderError::InvalidFormat(e.to_string()))?;
        
        // В версии 2 индекс идет после превью, если оно есть в начале
        let index_start = if header.thumbnail_size > 0 && header.thumbnail_offset == AilHeader::SIZE as u64 {
            header.thumbnail_offset + header.thumbnail_size
        } else {
            AilHeader::SIZE as u64
        };
        
        reader.seek(SeekFrom::Start(index_start))?;
        
        // Читаем индекс
        let entries = Self::read_index(&mut reader, header.entry_count)?;
        
        Ok(Self {
            file: reader,
            header,
            entries,
        })
    }

    /// Читает заголовок из файла
    fn read_header(file: &mut File) -> Result<AilHeader, DecoderError> {
        AilHeader::read(file).map_err(|e| DecoderError::InvalidFormat(e.to_string()))
    }
    /// Читает индекс записей
    fn read_index<R: Read>(file: &mut R, count: u32) -> Result<Vec<AilEntry>, DecoderError> {
        let mut entries = Vec::with_capacity(count as usize);

        for _ in 0..count {
            // 1. Читаем имя файла (null-terminated)
            let mut name_bytes = Vec::new();
            let mut found_null = false;
            for _ in 0..1024 {
                let mut byte = [0u8; 1];
                file.read_exact(&mut byte)?;
                if byte[0] == 0 {
                    found_null = true;
                    break;
                }
                name_bytes.push(byte[0]);
            }
            
            if !found_null {
                return Err(DecoderError::InvalidFormat("Filename too long or not null-terminated".to_string()));
            }
            let original_name = String::from_utf8_lossy(&name_bytes).into_owned();

            // 2. Читаем тип записи (1 байт)
            let mut entry_type_byte = [0u8; 1];
            file.read_exact(&mut entry_type_byte)?;
            let entry_type = EntryType::from_u8(entry_type_byte[0])
                .unwrap_or(EntryType::Image);

            // 3. Читаем тип сжатия (1 байт)
            let mut compression_byte = [0u8; 1];
            file.read_exact(&mut compression_byte)?;
            let compression = CompressionType::from_u8(compression_byte[0])
                .ok_or_else(|| DecoderError::InvalidFormat(format!("Invalid compression type: {} for file {}", compression_byte[0], original_name)))?;

            // 4. Пропускаем reserved (2 байта)
            let mut reserved = [0u8; 2];
            file.read_exact(&mut reserved)?;
            // 4. Читаем размеры (u64 LE)
            let mut u64_buf = [0u8; 8];
            
            file.read_exact(&mut u64_buf)?;
            let original_size = u64::from_le_bytes(u64_buf);

            file.read_exact(&mut u64_buf)?;
            let compressed_size = u64::from_le_bytes(u64_buf);

            file.read_exact(&mut u64_buf)?;
            let data_offset = u64::from_le_bytes(u64_buf);

            // 5. Читаем MD5 (16 байт)
            let mut md5_hash = [0u8; 16];
            file.read_exact(&mut md5_hash)?;

            entries.push(AilEntry {
                original_name,
                entry_type,
                compression,
                reserved,
                original_size,
                compressed_size,
                data_offset,
                md5_hash,
            });        }

        Ok(entries)
    }

    /// Возвращает количество изображений
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Возвращает информацию о записи
    pub fn get_entry(&self, index: usize) -> Option<&AilEntry> {
        self.entries.get(index)
    }

    /// Возвращает список всех записей
    pub fn entries(&self) -> &[AilEntry] {
        &self.entries
    }

    /// Читает изображение по индексу
    pub fn read_image(&mut self, index: usize) -> Result<Vec<u8>, DecoderError> {
        let entry = self.entries.get(index)
            .ok_or(DecoderError::IndexOutOfBounds(index))?;

        self.file.seek(SeekFrom::Start(entry.data_offset))?;

        let mut data = vec![0u8; entry.compressed_size as usize];
        self.file.read_exact(&mut data)?;

        // 1. Дешифрование (если включено)
        let data = if self.header.encrypted == 1 {
            let key = crate::crypto::get_or_create_key();
            crate::crypto::decrypt(&data, &key)
                .map_err(|e| DecoderError::Decompression(format!("Decryption error: {}", e)))?
        } else {
            data
        };

        // 2. Декомпрессия
        let data = match entry.compression {
            CompressionType::None => data,
            CompressionType::Zstd => {
                zstd::decode_all(&data[..])
                    .map_err(|e| DecoderError::Decompression(e.to_string()))?
            }
            CompressionType::Lz4 => {
                let mut decoder = lz4::Decoder::new(&data[..])
                    .map_err(|e| DecoderError::Decompression(e.to_string()))?;
                let mut decompressed = Vec::with_capacity(entry.original_size as usize);
                std::io::copy(&mut decoder, &mut decompressed)?;
                decompressed
            }
        };

        Ok(data)
    }

    /// Читает встроенное превью (v2)
    pub fn read_thumbnail(&mut self) -> Result<Vec<u8>, DecoderError> {
        if self.header.thumbnail_size == 0 {
            return Err(DecoderError::InvalidFormat("No thumbnail in this file".to_string()));
        }

        self.file.seek(SeekFrom::Start(self.header.thumbnail_offset))?;
        
        // Читаем заголовок превью (20 байт)
        let mut thumb_header = [0u8; 20];
        self.file.read_exact(&mut thumb_header)?;
        
        let compressed_size = u64::from_le_bytes(thumb_header[12..20].try_into().unwrap());
        
        let mut data = vec![0u8; compressed_size as usize];
        self.file.read_exact(&mut data)?;

        // 1. Дешифрование превью
        let data = if self.header.encrypted == 1 {
            let key = crate::crypto::get_or_create_key();
            crate::crypto::decrypt(&data, &key)
                .map_err(|e| DecoderError::Decompression(format!("Thumbnail decryption error: {}", e)))?
        } else {
            data
        };

        // 2. Декомпрессия (превью в v2 всегда Zstd)
        let jpeg_data = zstd::decode_all(&data[..])
            .map_err(|e| DecoderError::Decompression(e.to_string()))?;

        Ok(jpeg_data)
    }

    /// Читает первое изображение (для превью)
    pub fn read_first_image(&mut self) -> Result<Vec<u8>, DecoderError> {
        if self.entries.is_empty() {
            return Err(DecoderError::NoImages);
        }
        self.read_image(0)
    }

    /// Возвращает имя файла .ail (без расширения)
    pub fn name(&self) -> String {
        // Это нужно будет передать извне
        "Unknown".to_string()
    }
}

/// Ошибки декодера
#[derive(Debug)]
pub enum DecoderError {
    Io(std::io::Error),
    InvalidFormat(String),
    IndexOutOfBounds(usize),
    NoImages,
    Decompression(String),
}

impl std::fmt::Display for DecoderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecoderError::Io(e) => write!(f, "IO error: {}", e),
            DecoderError::InvalidFormat(s) => write!(f, "Invalid format: {}", s),
            DecoderError::IndexOutOfBounds(i) => write!(f, "Index out of bounds: {}", i),
            DecoderError::NoImages => write!(f, "No images in file"),
            DecoderError::Decompression(s) => write!(f, "Decompression error: {}", s),
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

impl From<std::io::Error> for DecoderError {
    fn from(e: std::io::Error) -> Self {
        DecoderError::Io(e)
    }
}

impl From<EncoderError> for DecoderError {
    fn from(e: EncoderError) -> Self {
        DecoderError::InvalidFormat(e.to_string())
    }
}

/// Информация о .ail файле для галереи
#[derive(Debug, Clone)]
pub struct AilInfo {
    pub path: std::path::PathBuf,
    pub name: String,
    pub image_count: usize,
    pub preview_data: Option<Vec<u8>>,
}

impl AilInfo {
    /// Быстрая загрузка без чтения изображений
    pub fn from_path(path: std::path::PathBuf) -> Result<Self, DecoderError> {
        let name = path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();

        let mut decoder = AilDecoder::open(&path)?;
        let image_count = decoder.len();

        let preview_data = if decoder.header.thumbnail_size > 0 {
            match decoder.read_thumbnail() {
                Ok(data) => Some(data),
                Err(e) => {
                    eprintln!("Failed to read thumbnail for {:?}: {}", path, e);
                    None
                }
            }
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

    /// Загружает превью для этого файла
    pub fn load_preview(&mut self) -> Result<(), DecoderError> {
        if self.preview_data.is_some() {
            return Ok(());
        }

        let mut decoder = AilDecoder::open(&self.path)?;
        
        // Читаем превью из заголовка (v2)
        if decoder.header.thumbnail_size > 0 {
            decoder.file.seek(SeekFrom::Start(decoder.header.thumbnail_offset))?;
            
            // Читаем заголовок превью
            let mut thumb_header = [0u8; 20];
            decoder.file.read_exact(&mut thumb_header)?;
            
            let compressed_size = u64::from_le_bytes(thumb_header[12..20].try_into().unwrap());
            let mut compressed_data = vec![0u8; compressed_size as usize];
            decoder.file.read_exact(&mut compressed_data)?;
            
            // Декомпрессия превью
            let jpeg_data = zstd::decode_all(&compressed_data[..])
                .map_err(|e| DecoderError::Decompression(e.to_string()))?;
            
            self.preview_data = Some(jpeg_data);
        } else if self.image_count > 0 {
            // Fallback для v1: читаем первое полное изображение
            self.preview_data = Some(decoder.read_first_image()?);
        }
        Ok(())
    }}
/// Сканирует папку на наличие .ail файлов
pub fn scan_ail_files<P: AsRef<Path>>(dir: P) -> Vec<AilInfo> {
    let mut files = Vec::new();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        
        if !path.is_file() {
            continue;
        }

        if path.extension().and_then(|e| e.to_str()) == Some("ail") {
            if let Ok(info) = AilInfo::from_path(path) {
                files.push(info);
            }
        }
    }

    // Сортируем по имени
    files.sort_by(|a, b| alphanumeric_sort::compare_str(&a.name, &b.name));
    
    files
}
