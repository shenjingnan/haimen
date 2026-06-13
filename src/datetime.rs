/// 日期时间工具模块
///
/// 基于 chrono crate 提供统一的日期时间处理函数。
use chrono::{DateTime, Local};

/// 生成 ISO 8601 时间戳（本地时区）: YYYY-MM-DDTHH:MM:SS+08:00
pub fn iso_timestamp_now() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

/// 将 Unix 秒数转为紧凑时间戳: YYMMDDHHMMSS
pub fn unix_ts_to_compact(secs: u64) -> String {
    let secs_i64 = secs as i64;
    let dt = DateTime::from_timestamp(secs_i64, 0).expect("timestamp should be valid Unix seconds");
    dt.format("%y%m%d%H%M%S").to_string()
}

/// 将 Unix 秒数转为 ISO 8601 时间戳: YYYY-MM-DDTHH:MM:SSZ
pub fn iso_timestamp_at(secs: u64) -> String {
    let secs_i64 = secs as i64;
    let dt = DateTime::from_timestamp(secs_i64, 0).expect("timestamp should be valid Unix seconds");
    dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn test_iso_timestamp_now_format() {
        let now = iso_timestamp_now();
        assert!(now.len() >= 24, "时间戳长度至少 24（含时区偏移）: {}", now);
        assert!(now.contains('T'), "时间戳应包含 T 分隔符: {}", now);
        let offset_start = now.rfind(|c: char| c == '+' || c == '-');
        assert!(
            offset_start.is_some() && offset_start.unwrap() > 10,
            "时间戳应包含时区偏移（如 +08:00）: {}",
            now
        );
        let date_part = &now[..10];
        assert!(
            date_part.chars().filter(|&c| c == '-').count() == 2,
            "日期部分应为 YYYY-MM-DD: {}",
            date_part
        );
    }

    #[test]
    fn test_iso_timestamp_at_epoch() {
        assert_eq!(iso_timestamp_at(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn test_iso_timestamp_at_leap_year() {
        let jan_1_2024: u64 = 1704067200;
        let feb_29_2024_12_00 = jan_1_2024 + 31 * 86400 + 28 * 86400 + 12 * 3600;
        assert_eq!(iso_timestamp_at(feb_29_2024_12_00), "2024-02-29T12:00:00Z");
    }

    #[test]
    fn test_iso_timestamp_at_year_end() {
        let jan_1_2025: u64 = 1735689600;
        let dec_31_2025_23_59_59 = jan_1_2025 + 364 * 86400 + 23 * 3600 + 59 * 60 + 59;
        assert_eq!(
            iso_timestamp_at(dec_31_2025_23_59_59),
            "2025-12-31T23:59:59Z"
        );
    }

    #[test]
    fn test_iso_timestamp_at_year_start() {
        let jan_1_2026: u64 = 1767225600;
        assert_eq!(iso_timestamp_at(jan_1_2026 + 1), "2026-01-01T00:00:01Z");
    }

    #[test]
    fn test_unix_ts_to_compact_format() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let result = unix_ts_to_compact(now.as_secs());
        assert_eq!(result.len(), 12, "YYMMDDHHMMSS 应为 12 位");
        assert!(
            result.chars().all(|c| c.is_ascii_digit()),
            "应全为数字: {}",
            result
        );
    }

    #[test]
    fn test_unix_ts_to_compact_known_value() {
        let secs = 20602u64 * 86400;
        let result = unix_ts_to_compact(secs);
        assert_eq!(result, "260529000000");
    }

    #[test]
    fn test_unix_ts_to_compact_leap_year_feb() {
        let days_to_2024_jan1: u64 = 19723;
        let day_of_year: u64 = 31 + 28;
        let total_days = days_to_2024_jan1 + day_of_year;
        let secs = total_days * 86400 + 12 * 3600;
        let result = unix_ts_to_compact(secs);
        assert_eq!(result, "240229120000");
    }
}
