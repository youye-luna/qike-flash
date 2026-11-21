//! 镜像源：原始 ISO / IMG，以及流式解码的 `.img.xz`（不落地临时文件）。

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use xz2::read::XzDecoder;

use super::error::{BurnError, Result};
use super::traits::ImageSource;

/// 镜像元信息：名称、文件哈希、解码后的字节数。
#[derive(Debug, Clone)]
pub struct ImageInfo {
    pub path: PathBuf,
    pub name: String,
    /// 源文件（.xz 为压缩文件本身）的 SHA-256。
    pub sha256: String,
    /// 解码后的镜像字节数。
    pub size: u64,
}

impl ImageInfo {
    pub fn from_path(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path).map_err(|error| {
            BurnError::context(format!("读取镜像信息 {} 失败", path.display()), error)
        })?;
        if !metadata.is_file() {
            return Err(BurnError::message(format!(
                "镜像不是文件: {}",
                path.display()
            )));
        }

        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let is_xz = path
            .extension()
            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("xz"))
            .unwrap_or(false);

        let size = if is_xz {
            xz_uncompressed_size(path)?
        } else {
            metadata.len()
        };
        let sha256 = sha256_file(path)?;

        Ok(Self {
            path: path.to_path_buf(),
            name,
            sha256,
            size,
        })
    }

    /// 打开可顺序读取的镜像源。
    pub fn open(&self) -> Result<Box<dyn ImageSource>> {
        let is_xz = self
            .path
            .extension()
            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("xz"))
            .unwrap_or(false);
        if is_xz {
            Ok(Box::new(XzImage::open(&self.path, &self.name, self.size)?))
        } else {
            Ok(Box::new(RawImage::open(
                &self.path,
                &self.name,
                self.size,
            )?))
        }
    }
}

/// 计算源文件 SHA-256。
pub fn sha256_file(path: &Path) -> Result<String> {
    let file = File::open(path)
        .map_err(|error| BurnError::context(format!("打开文件 {} 失败", path.display()), error))?;
    let mut reader = BufReader::with_capacity(4 * 1024 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 4 * 1024 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| BurnError::context("计算镜像哈希失败", error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(to_hex(&hasher.finalize()))
}

fn to_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(hex_char(byte >> 4));
        text.push(hex_char(byte & 0x0F));
    }
    text
}

fn hex_char(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'a' + (value - 10)) as char,
    }
}

struct RawImage {
    file: File,
    name: String,
    size: u64,
}

impl RawImage {
    fn open(path: &Path, name: &str, size: u64) -> Result<Self> {
        let file = File::open(path)
            .map_err(|error| BurnError::context(format!("打开镜像 {} 失败", path.display()), error))?;
        Ok(Self {
            file,
            name: name.to_string(),
            size,
        })
    }
}

impl ImageSource for RawImage {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        self.file
            .read(buffer)
            .map_err(|error| BurnError::context("读取镜像失败", error))
    }

    fn rewind(&mut self) -> Result<()> {
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|error| BurnError::context("回绕镜像失败", error))?;
        Ok(())
    }
}

struct XzImage {
    path: PathBuf,
    name: String,
    size: u64,
    decoder: XzDecoder<BufReader<File>>,
}

impl XzImage {
    fn open(path: &Path, name: &str, size: u64) -> Result<Self> {
        let decoder = open_decoder(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            name: name.to_string(),
            size,
            decoder,
        })
    }
}

fn open_decoder(path: &Path) -> Result<XzDecoder<BufReader<File>>> {
    let file = File::open(path)
        .map_err(|error| BurnError::context(format!("打开镜像 {} 失败", path.display()), error))?;
    Ok(XzDecoder::new(BufReader::with_capacity(1024 * 1024, file)))
}

impl ImageSource for XzImage {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        self.decoder
            .read(buffer)
            .map_err(|error| BurnError::context("解码 .xz 镜像失败", error))
    }

    fn rewind(&mut self) -> Result<()> {
        self.decoder = open_decoder(&self.path)?;
        Ok(())
    }
}

/// 解析 `.xz` 索引，得到解压后的字节数（无需解压整个镜像）。
///
/// 只支持单流 .xz（树莓派官方镜像即单流）；索引损坏时返回错误而不是猜测。
pub fn xz_uncompressed_size(path: &Path) -> Result<u64> {
    let mut file = File::open(path)
        .map_err(|error| BurnError::context(format!("打开镜像 {} 失败", path.display()), error))?;
    let file_len = file
        .metadata()
        .map_err(|error| BurnError::context("读取镜像大小失败", error))?
        .len();
    if file_len < 12 {
        return Err(BurnError::message("不是有效的 .xz 文件（长度不足）"));
    }

    file.seek(SeekFrom::End(-12))
        .map_err(|error| BurnError::context("定位 .xz 尾部失败", error))?;
    let mut footer = [0u8; 12];
    file.read_exact(&mut footer)
        .map_err(|error| BurnError::context("读取 .xz 尾部失败", error))?;
    if footer[10] != b'Y' || footer[11] != b'Z' {
        return Err(BurnError::message("不是有效的 .xz 文件（缺少 YZ 魔数）"));
    }

    let backward_size = u32::from_le_bytes([footer[4], footer[5], footer[6], footer[7]]) as u64;
    let index_size = (backward_size + 1) * 4;
    if file_len < 12 + index_size {
        return Err(BurnError::message("不是有效的 .xz 文件（索引越界）"));
    }

    let index_offset = file_len - 12 - index_size;
    file.seek(SeekFrom::Start(index_offset))
        .map_err(|error| BurnError::context("定位 .xz 索引失败", error))?;
    let mut index = vec![0u8; index_size as usize];
    file.read_exact(&mut index)
        .map_err(|error| BurnError::context("读取 .xz 索引失败", error))?;

    parse_xz_index(&index)
}

fn parse_xz_index(index: &[u8]) -> Result<u64> {
    if index.first() != Some(&0x00) {
        return Err(BurnError::message("不是有效的 .xz 索引（指示字节错误）"));
    }
    let mut cursor = 1usize;
    let (records, used) = read_vli(&index[cursor..])?;
    cursor += used;
    if records > 1_000_000 {
        return Err(BurnError::message("不是有效的 .xz 索引（记录数异常）"));
    }

    let mut total: u64 = 0;
    for _ in 0..records {
        let (_unpadded, used) = read_vli(&index[cursor..])?;
        cursor += used;
        let (uncompressed, used) = read_vli(&index[cursor..])?;
        cursor += used;
        total = total
            .checked_add(uncompressed)
            .ok_or_else(|| BurnError::message("镜像解压后大小溢出"))?;
    }

    Ok(total)
}

fn read_vli(bytes: &[u8]) -> Result<(u64, usize)> {
    let mut value: u64 = 0;
    for index in 0..9usize {
        let byte = *bytes
            .get(index)
            .ok_or_else(|| BurnError::message("不是有效的 .xz 索引（VLI 截断）"))?;
        let shift = (index * 7) as u32;
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, index + 1));
        }
    }
    Err(BurnError::message("不是有效的 .xz 索引（VLI 过长）"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn test_path(name: &str) -> PathBuf {
        let dir = PathBuf::from("target").join("test-tmp");
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir.join(format!("{}-{}", std::process::id(), name))
    }

    #[test]
    fn sha256_matches_known_vector() {
        let path = test_path("hash.bin");
        std::fs::write(&path, b"abc").expect("write");
        let hash = sha256_file(&path).expect("hash");
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn xz_size_and_stream_roundtrip() {
        let path = test_path("image.img.xz");
        let mut source = Vec::new();
        for index in 0..300_000u32 {
            source.push((index % 251) as u8);
        }

        let mut encoder = xz2::write::XzEncoder::new(Vec::new(), 6);
        encoder.write_all(&source).expect("encode");
        let compressed = encoder.finish().expect("finish");
        std::fs::write(&path, &compressed).expect("write xz");

        let size = xz_uncompressed_size(&path).expect("parse index");
        assert_eq!(size, source.len() as u64);

        let info = ImageInfo::from_path(&path).expect("info");
        assert_eq!(info.size, source.len() as u64);

        let mut image = info.open().expect("open");
        let mut decoded = Vec::new();
        let mut buffer = vec![0u8; 8192];
        loop {
            let read = image.read(&mut buffer).expect("read");
            if read == 0 {
                break;
            }
            decoded.extend_from_slice(&buffer[..read]);
        }
        assert_eq!(decoded, source);

        image.rewind().expect("rewind");
        let read = image.read(&mut buffer).expect("reread");
        assert_eq!(&buffer[..read], &source[..read]);

        let _ = std::fs::remove_file(path);
    }
}
