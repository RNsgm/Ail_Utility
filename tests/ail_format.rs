use std::io::Cursor;

use tempfile::TempDir;

use ArchiveUtils::decoder::AilDecoder;
use ArchiveUtils::encoder::format::{
    AilHeader, CompressionType, index_offset_from_header, read_index_entries,
};
use ArchiveUtils::encoder::{AilBuilder, EncoderError};

fn tiny_png() -> Vec<u8> {
    // 1x1 white PNG generated for tests
    let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
    let mut bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut bytes);
    img.write_to(&mut cursor, image::ImageFormat::Png)
        .expect("encode test png");
    bytes
}

fn with_test_key<F>(test: F) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce() -> Result<(), Box<dyn std::error::Error>>,
{
    let dir = TempDir::new()?;
    let original_dir = std::env::current_dir()?;
    std::env::set_current_dir(dir.path())?;
    std::fs::write(dir.path().join("key.bin"), [7u8; 32])?;
    let result = test();
    std::env::set_current_dir(original_dir)?;
    result
}

#[test]
fn ail_header_roundtrip() -> Result<(), EncoderError> {
    let header = AilHeader::new(3, 32, 128);
    let bytes = header.to_bytes();
    let parsed = AilHeader::from_bytes(&bytes)?;
    assert_eq!(parsed.entry_count, 3);
    assert_eq!(parsed.thumbnail_offset, 32);
    assert_eq!(parsed.thumbnail_size, 128);
    Ok(())
}

#[test]
fn ail_roundtrip_preserves_payload() -> Result<(), Box<dyn std::error::Error>> {
    with_test_key(|| {
        let original = tiny_png();
        let mut builder = AilBuilder::new().with_thumbnail_size(64);
        builder.add_image("page.png".to_string(), original.clone(), CompressionType::Zstd)?;

        let mut output = Vec::new();
        builder.build(&mut output)?;

        let ail_path = std::env::current_dir()?.join("roundtrip.ail");
        std::fs::write(&ail_path, &output)?;

        let mut decoder = AilDecoder::open(&ail_path)?;
        assert_eq!(decoder.len(), 1);
        assert_eq!(decoder.read_image(0)?, original);
        assert!(!decoder.read_thumbnail()?.is_empty());
        Ok(())
    })
}

#[test]
fn read_index_entries_from_bytes() -> Result<(), Box<dyn std::error::Error>> {
    with_test_key(|| {
        let mut builder = AilBuilder::new();
        builder.add_image("a.png".to_string(), tiny_png(), CompressionType::None)?;
        builder.add_image("b.png".to_string(), tiny_png(), CompressionType::Zstd)?;

        let mut file_bytes = Vec::new();
        builder.build(&mut file_bytes)?;

        let header = AilHeader::from_bytes(&file_bytes[..AilHeader::SIZE].try_into().unwrap())?;
        let index_start = index_offset_from_header(&header) as usize;
        let mut cursor = Cursor::new(&file_bytes[index_start..]);
        let entries = read_index_entries(&mut cursor, header.entry_count)?;

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].original_name, "a.png");
        assert_eq!(entries[1].original_name, "b.png");
        Ok(())
    })
}
