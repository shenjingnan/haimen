//! 二进制音频协议编解码
//!
//! 支持三种协议格式：
//! - BinaryProtocol2: 16 字节头部（Version/Type/Reserved/Timestamp/Size）+ 负载
//! - BinaryProtocol3: 4 字节头部（Type/Reserved/Size）+ 负载
//! - Raw Opus: 无头部，直接是 Opus 帧
//!
//! 所有多字节字段均为大端序（网络字节序）。

/// 协议检测结果
#[derive(Debug, Clone, PartialEq)]
pub enum AudioProtocol {
    /// BinaryProtocol2: 16 字节头部 + 负载
    Protocol2 { timestamp: u32, payload: Vec<u8> },
    /// BinaryProtocol3: 4 字节头部 + 负载
    Protocol3 { payload: Vec<u8> },
    /// Raw Opus: 无头部，直接是 Opus 帧
    RawOpus(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProtocolError {
    /// 数据不足以解析头部
    InsufficientData,
    /// 无法识别协议格式
    UnknownProtocol,
    /// 头部字段无效（如 Version != 2）
    InvalidHeader,
}

// ─── 协议头部大小 ──────────────────────────────────────────

const PROTOCOL2_HEADER: usize = 16; // 2 + 2 + 4 + 4 + 4
const PROTOCOL3_HEADER: usize = 4; // 1 + 1 + 2

// ─── 协议检测 ──────────────────────────────────────────────

/// 自动检测音频协议并解析（参考 xiaozhi-client `detectAudioProtocol`）
///
/// 检测顺序：
/// 1. Protocol2: 长度 ≥ 16, Version=2, Type∈{0,1}
/// 2. Protocol3: 长度 ≥ 4, Type∈{0,1}
/// 3. Raw Opus: 有效 Opus TOC（config ≤ 23, channels ≤ 1）
pub fn detect_and_parse(data: &[u8]) -> Result<AudioProtocol, ProtocolError> {
    // 优先检测 Protocol2
    if let Ok(p) = try_parse_protocol2(data) {
        return Ok(p);
    }

    // 检测 Protocol3
    if let Ok(p) = try_parse_protocol3(data) {
        return Ok(p);
    }

    // 回退 Raw Opus
    if is_valid_opus_data(data) {
        return Ok(AudioProtocol::RawOpus(data.to_vec()));
    }

    Err(ProtocolError::UnknownProtocol)
}

// ─── BinaryProtocol2 ───────────────────────────────────────

/// 尝试解析 BinaryProtocol2
fn try_parse_protocol2(data: &[u8]) -> Result<AudioProtocol, ProtocolError> {
    if data.len() < PROTOCOL2_HEADER {
        return Err(ProtocolError::InsufficientData);
    }

    // 检查 Version（uint16 BE）= 2
    let version = u16::from_be_bytes([data[0], data[1]]);
    if version != 2 {
        return Err(ProtocolError::InvalidHeader);
    }

    // 检查 Type（uint16 BE）∈ {0, 1}
    let type_value = u16::from_be_bytes([data[2], data[3]]);
    if type_value != 0 && type_value != 1 {
        return Err(ProtocolError::InvalidHeader);
    }

    // 读取 Timestamp（uint32 BE）
    let timestamp = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

    // 读取 Payload Size（uint32 BE）
    let payload_size = u32::from_be_bytes([data[12], data[13], data[14], data[15]]) as usize;

    // 验证负载长度
    if data.len() < PROTOCOL2_HEADER + payload_size {
        return Err(ProtocolError::InsufficientData);
    }

    let payload = data[PROTOCOL2_HEADER..PROTOCOL2_HEADER + payload_size].to_vec();

    Ok(AudioProtocol::Protocol2 { timestamp, payload })
}

/// 编码 BinaryProtocol2 帧（服务器→设备）
///
/// # 格式
/// ```text
/// [0-1]   Version (u16 BE)       = 2
/// [2-3]   Type (u16 BE)          = 0 (opus)
/// [4-7]   Reserved (u32 BE)      = 0
/// [8-11]  Timestamp (u32 BE)
/// [12-15] Payload Size (u32 BE)
/// [16+]   Opus 帧数据
/// ```
pub fn encode_protocol2(payload: &[u8], timestamp: u32) -> Vec<u8> {
    let total_len = PROTOCOL2_HEADER + payload.len();
    let mut buf = Vec::with_capacity(total_len);

    // Version = 2
    buf.extend_from_slice(&2u16.to_be_bytes());
    // Type = 0 (opus)
    buf.extend_from_slice(&0u16.to_be_bytes());
    // Reserved = 0
    buf.extend_from_slice(&0u32.to_be_bytes());
    // Timestamp
    buf.extend_from_slice(&timestamp.to_be_bytes());
    // Payload Size
    buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    // Payload
    buf.extend_from_slice(payload);

    debug_assert_eq!(buf.len(), total_len);
    buf
}

// ─── BinaryProtocol3 ───────────────────────────────────────

/// 尝试解析 BinaryProtocol3
fn try_parse_protocol3(data: &[u8]) -> Result<AudioProtocol, ProtocolError> {
    if data.len() < PROTOCOL3_HEADER {
        return Err(ProtocolError::InsufficientData);
    }

    // 检查 Type（uint8）∈ {0, 1}
    let type_value = data[0];
    if type_value != 0 && type_value != 1 {
        return Err(ProtocolError::InvalidHeader);
    }

    // 读取 Payload Size（uint16 BE）
    let payload_size = u16::from_be_bytes([data[2], data[3]]) as usize;

    // 验证负载长度
    if data.len() < PROTOCOL3_HEADER + payload_size {
        return Err(ProtocolError::InsufficientData);
    }

    let payload = data[PROTOCOL3_HEADER..PROTOCOL3_HEADER + payload_size].to_vec();

    Ok(AudioProtocol::Protocol3 { payload })
}

// ─── Raw Opus 检测 ─────────────────────────────────────────

/// 检查是否为有效的 Opus TOC（Table of Contents）字节
///
/// Opus TOC 格式：
/// - bits 7-3: config (0-23)
/// - bit 2:    stereo flag (0-1)
/// - bits 1-0: frame count code (0-3)
fn is_valid_opus_data(data: &[u8]) -> bool {
    if data.len() < 2 {
        return false;
    }

    let toc = data[0];
    let config = (toc >> 3) & 0x1f;
    let _channels = (toc >> 2) & 0x01;

    // config 必须在 0-23 范围内
    if config > 23 {
        return false;
    }

    true
}

// ─── 测试 ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Protocol2 有效帧（Version=2, Type=0, Timestamp=60, PayloadSize=4）
    /// 负载为 [0xFC, 0xFF, 0x00, 0x00]
    fn make_protocol2_frame() -> Vec<u8> {
        let mut buf = vec![
            0x00, 0x02, // Version = 2
            0x00, 0x00, // Type = 0 (opus)
            0x00, 0x00, 0x00, 0x00, // Reserved = 0
            0x00, 0x00, 0x00, 0x3C, // Timestamp = 60
            0x00, 0x00, 0x00, 0x04, // PayloadSize = 4
        ];
        buf.extend_from_slice(&[0xFC, 0xFF, 0x00, 0x00]);
        buf
    }

    /// Protocol3 有效帧（Type=0, PayloadSize=4）
    fn make_protocol3_frame() -> Vec<u8> {
        let mut buf = vec![
            0x00, // Type = 0 (opus)
            0x00, // Reserved = 0
            0x00, 0x04, // PayloadSize = 4
        ];
        buf.extend_from_slice(&[0xFC, 0xFF, 0x00, 0x00]);
        buf
    }

    #[test]
    fn test_detect_protocol2() {
        let data = make_protocol2_frame();
        let result = detect_and_parse(&data).unwrap();
        match result {
            AudioProtocol::Protocol2 { timestamp, payload } => {
                assert_eq!(timestamp, 60);
                assert_eq!(payload, vec![0xFC, 0xFF, 0x00, 0x00]);
            }
            _ => panic!("Expected Protocol2"),
        }
    }

    #[test]
    fn test_detect_protocol3() {
        let data = make_protocol3_frame();
        let result = detect_and_parse(&data).unwrap();
        match result {
            AudioProtocol::Protocol3 { payload } => {
                assert_eq!(payload, vec![0xFC, 0xFF, 0x00, 0x00]);
            }
            _ => panic!("Expected Protocol3"),
        }
    }

    #[test]
    fn test_detect_raw_opus() {
        // TOC byte: config=24 (0b11000), stereo=0, frames=0
        // config 24 > 23, should NOT be valid Opus
        let invalid = vec![0xC0, 0xFF, 0x00, 0x00];
        assert!(detect_and_parse(&invalid).is_err());

        // Valid TOC: config=16 (0b10000), stereo=0, frames=0 => 0x80
        let valid = vec![0x80, 0xFF, 0x00, 0x00];
        match detect_and_parse(&valid).unwrap() {
            AudioProtocol::RawOpus(d) => {
                assert_eq!(d, valid);
            }
            _ => panic!("Expected RawOpus"),
        }
    }

    #[test]
    fn test_detect_unknown() {
        // Empty data
        let empty: Vec<u8> = vec![];
        assert_eq!(
            detect_and_parse(&empty).unwrap_err(),
            ProtocolError::UnknownProtocol
        );

        // Invalid: single byte that's not valid Opus TOC
        let invalid = vec![0xFF]; // config=31, > 23
        assert_eq!(
            detect_and_parse(&invalid).unwrap_err(),
            ProtocolError::UnknownProtocol
        );
    }

    #[test]
    fn test_encode_protocol2() {
        let payload = vec![0xFC, 0xFF, 0x00, 0x00];
        let timestamp = 120u32;
        let result = encode_protocol2(&payload, timestamp);

        // Verify header
        assert_eq!(&result[0..2], &[0x00, 0x02]); // Version
        assert_eq!(&result[2..4], &[0x00, 0x00]); // Type
        assert_eq!(&result[4..8], &[0x00, 0x00, 0x00, 0x00]); // Reserved
        assert_eq!(&result[8..12], &[0x00, 0x00, 0x00, 0x78]); // Timestamp = 120
        assert_eq!(&result[12..16], &[0x00, 0x00, 0x00, 0x04]); // PayloadSize = 4
        assert_eq!(&result[16..], &payload); // Payload
    }

    #[test]
    fn test_detect_prefers_protocol2_over_protocol3() {
        // A valid Protocol2 frame should be detected as Protocol2, not Protocol3
        let data = make_protocol2_frame();
        match detect_and_parse(&data).unwrap() {
            AudioProtocol::Protocol2 { .. } => {} // OK
            _ => panic!("Protocol2 frame should be detected as Protocol2"),
        }
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let payload = vec![0x80, 0x01, 0x02, 0x03];
        let timestamp = 999u32;

        let encoded = encode_protocol2(&payload, timestamp);
        let decoded = detect_and_parse(&encoded).unwrap();

        match decoded {
            AudioProtocol::Protocol2 {
                timestamp: ts,
                payload: p,
            } => {
                assert_eq!(ts, timestamp);
                assert_eq!(p, payload);
            }
            _ => panic!("Expected Protocol2"),
        }
    }

    #[test]
    fn test_protocol2_insufficient_data() {
        // Only 10 bytes (need 16 for header)
        let short = vec![0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01];
        let result = try_parse_protocol2(&short);
        assert_eq!(result.unwrap_err(), ProtocolError::InsufficientData);
    }

    #[test]
    fn test_protocol2_wrong_version() {
        let mut data = make_protocol2_frame();
        data[0] = 0x00;
        data[1] = 0x03; // Version = 3 instead of 2
        let result = try_parse_protocol2(&data);
        assert_eq!(result.unwrap_err(), ProtocolError::InvalidHeader);
    }

    #[test]
    fn test_protocol3_insufficient_data() {
        // Only 2 bytes (need 4 for header)
        let short = vec![0x00, 0x00];
        let result = try_parse_protocol3(&short);
        assert_eq!(result.unwrap_err(), ProtocolError::InsufficientData);
    }

    #[test]
    fn test_invalid_protocol3_type() {
        let mut data = make_protocol3_frame();
        data[0] = 0xFF; // Invalid type
        let result = try_parse_protocol3(&data);
        assert_eq!(result.unwrap_err(), ProtocolError::InvalidHeader);
    }

    #[test]
    fn test_protocol2_with_json_type() {
        let mut buf = vec![
            0x00, 0x02, // Version = 2
            0x00, 0x01, // Type = 1 (json)
            0x00, 0x00, 0x00, 0x00, // Reserved
            0x00, 0x00, 0x00, 0x00, // Timestamp = 0
            0x00, 0x00, 0x00, 0x05, // PayloadSize = 5
        ];
        buf.extend_from_slice(b"hello");

        let result = detect_and_parse(&buf).unwrap();
        match result {
            AudioProtocol::Protocol2 { payload, .. } => {
                assert_eq!(payload, b"hello");
            }
            _ => panic!("Expected Protocol2"),
        }
    }
}
