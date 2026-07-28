use std::io;

// Everything IPC v1 wire layout:
// https://www.voidtools.com/support/everything/sdk/ipc/
const QUERY_HEADER_SIZE: usize = 5 * size_of::<u32>();
const LIST_HEADER_SIZE: usize = 7 * size_of::<u32>();
const ITEM_SIZE: usize = 3 * size_of::<u32>();
const MAX_ITEM_COUNT: usize = 1_000_000;

pub(super) const ITEM_FLAG_FOLDER: u32 = 0x0000_0001;

#[derive(Debug, Eq, PartialEq)]
pub(super) struct ReplyItem {
    pub(super) flags: u32,
    pub(super) file_name: Vec<u16>,
    pub(super) path: Vec<u16>,
}

fn read_u32(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let value = bytes
        .get(offset..offset + size_of::<u32>())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "truncated Everything reply"))?;
    Ok(u32::from_le_bytes(value.try_into().unwrap()))
}

fn read_utf16_z(bytes: &[u8], offset: u32, data_start: usize) -> io::Result<Vec<u16>> {
    let offset = offset as usize;
    if offset < data_start || !offset.is_multiple_of(2) || offset >= bytes.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid string offset in Everything reply",
        ));
    }

    let mut units = Vec::new();
    let mut cursor = offset;
    loop {
        let encoded = bytes.get(cursor..cursor + 2).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "unterminated UTF-16 string in Everything reply",
            )
        })?;
        let unit = u16::from_le_bytes(encoded.try_into().unwrap());
        if unit == 0 {
            return Ok(units);
        }
        units.push(unit);
        cursor += 2;
    }
}

pub(super) fn encode_query(
    reply_window: u32,
    reply_id: u32,
    search: &[u16],
) -> io::Result<Vec<u8>> {
    if search.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Everything search text contains an embedded null",
        ));
    }

    let byte_len = QUERY_HEADER_SIZE
        .checked_add(
            search
                .len()
                .checked_add(1)
                .and_then(|length| length.checked_mul(size_of::<u16>()))
                .ok_or_else(|| io::Error::other("Everything query is too large"))?,
        )
        .ok_or_else(|| io::Error::other("Everything query is too large"))?;
    if byte_len > u32::MAX as usize {
        return Err(io::Error::other("Everything query is too large"));
    }

    let mut bytes = Vec::with_capacity(byte_len);
    for value in [reply_window, reply_id, 0, 0, u32::MAX] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for unit in search {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    Ok(bytes)
}

pub(super) fn parse_reply(bytes: &[u8]) -> io::Result<Vec<ReplyItem>> {
    if bytes.len() < LIST_HEADER_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated Everything reply header",
        ));
    }

    let item_count = read_u32(bytes, 5 * size_of::<u32>())? as usize;
    if item_count > MAX_ITEM_COUNT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Everything reply contains too many items",
        ));
    }
    let data_start = item_count
        .checked_mul(ITEM_SIZE)
        .and_then(|size| LIST_HEADER_SIZE.checked_add(size))
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid item count in Everything reply",
            )
        })?;

    let mut items = Vec::with_capacity(item_count);
    for index in 0..item_count {
        let item_offset = LIST_HEADER_SIZE + index * ITEM_SIZE;
        let flags = read_u32(bytes, item_offset)?;
        let file_name_offset = read_u32(bytes, item_offset + size_of::<u32>())?;
        let path_offset = read_u32(bytes, item_offset + 2 * size_of::<u32>())?;
        items.push(ReplyItem {
            flags,
            file_name: read_utf16_z(bytes, file_name_offset, data_start)?,
            path: read_utf16_z(bytes, path_offset, data_start)?,
        });
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::{ITEM_FLAG_FOLDER, LIST_HEADER_SIZE, encode_query, parse_reply};

    fn push_utf16_z(bytes: &mut Vec<u8>, text: &str) -> u32 {
        let offset = bytes.len() as u32;
        for unit in text.encode_utf16().chain(std::iter::once(0)) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        offset
    }

    fn reply_with_one_item(path: &str, file_name: &str) -> Vec<u8> {
        let mut bytes = vec![0_u8; LIST_HEADER_SIZE + 12];
        let path_offset = push_utf16_z(&mut bytes, path);
        let file_name_offset = push_utf16_z(&mut bytes, file_name);
        for (index, value) in [0_u32, 1, 1, 0, 1, 1, 0].into_iter().enumerate() {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[LIST_HEADER_SIZE + 4..LIST_HEADER_SIZE + 8]
            .copy_from_slice(&file_name_offset.to_le_bytes());
        bytes[LIST_HEADER_SIZE + 8..LIST_HEADER_SIZE + 12]
            .copy_from_slice(&path_offset.to_le_bytes());
        bytes
    }

    #[test]
    fn query_has_packed_header_and_null_terminated_utf16() {
        let query =
            encode_query(0x1234, 0x4321, &"libcef".encode_utf16().collect::<Vec<_>>()).unwrap();
        assert_eq!(&query[0..4], &0x1234_u32.to_le_bytes());
        assert_eq!(&query[4..8], &0x4321_u32.to_le_bytes());
        assert_eq!(&query[12..20], &[0, 0, 0, 0, 255, 255, 255, 255]);
        assert_eq!(&query[query.len() - 2..], &[0, 0]);
    }

    #[test]
    fn reply_parser_reads_path_and_file_name() {
        let reply = reply_with_one_item(r"C:\Program Files\示例", "libcef.dll");
        let items = parse_reply(&reply).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            String::from_utf16(&items[0].path).unwrap(),
            r"C:\Program Files\示例"
        );
        assert_eq!(
            String::from_utf16(&items[0].file_name).unwrap(),
            "libcef.dll"
        );
    }

    #[test]
    fn reply_parser_preserves_item_flags() {
        let mut reply = reply_with_one_item(r"C:\App", "resources");
        reply[LIST_HEADER_SIZE..LIST_HEADER_SIZE + 4]
            .copy_from_slice(&ITEM_FLAG_FOLDER.to_le_bytes());
        assert_eq!(parse_reply(&reply).unwrap()[0].flags, ITEM_FLAG_FOLDER);
    }

    #[test]
    fn reply_parser_rejects_out_of_bounds_offsets() {
        let mut reply = reply_with_one_item(r"C:\App", "libcef.dll");
        reply[LIST_HEADER_SIZE + 4..LIST_HEADER_SIZE + 8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_reply(&reply).is_err());
    }

    #[test]
    fn reply_parser_rejects_unterminated_strings() {
        let mut reply = reply_with_one_item(r"C:\App", "libcef.dll");
        reply.truncate(reply.len() - 2);
        assert!(parse_reply(&reply).is_err());
    }

    #[test]
    fn reply_parser_limits_item_allocation() {
        let mut reply = vec![0_u8; LIST_HEADER_SIZE];
        reply[20..24].copy_from_slice(&1_000_001_u32.to_le_bytes());
        assert!(parse_reply(&reply).is_err());
    }
}
