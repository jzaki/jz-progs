//! A built guest (`.bin`), as `wallet program-loader deploy` needs to know it.
//!
//! The file is risc0's `ProgramBinary`: `R0BF`, a u32 format version, a u32-length-prefixed
//! header, then the u32-length-prefixed user ELF, then the kernel. Only the user ELF is stored
//! on chain, one segment account per `MAX_SEGMENT_DATA_LEN` bytes, and the wallet wants exactly
//! that many segment accounts.

/// `program_loader_core::MAX_SEGMENT_DATA_LEN`.
const MAX_SEGMENT_DATA_LEN: usize = 96 * 1024;

const MAGIC: &[u8] = b"R0BF";

/// How many segment accounts the program needs, or `None` when it isn't a `ProgramBinary`.
pub fn segment_count(bin: &[u8]) -> Option<usize> {
    let user_len = user_elf_len(bin)?;
    (user_len > 0).then(|| user_len.div_ceil(MAX_SEGMENT_DATA_LEN))
}

fn user_elf_len(bin: &[u8]) -> Option<usize> {
    let rest = bin.strip_prefix(MAGIC)?;
    let (_version, rest) = read_u32(rest)?;
    let (header_len, rest) = read_u32(rest)?;
    let rest = rest.get(usize::try_from(header_len).ok()?..)?;
    let (user_len, rest) = read_u32(rest)?;
    let user_len = usize::try_from(user_len).ok()?;
    (rest.len() >= user_len).then_some(user_len)
}

fn read_u32(bytes: &[u8]) -> Option<(u32, &[u8])> {
    let (head, rest) = bytes.split_at_checked(4)?;
    Some((u32::from_le_bytes(head.try_into().ok()?), rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin(header: &[u8], user_len: usize) -> Vec<u8> {
        let mut bin = MAGIC.to_vec();
        bin.extend(1_u32.to_le_bytes());
        bin.extend(u32::try_from(header.len()).unwrap().to_le_bytes());
        bin.extend(header);
        bin.extend(u32::try_from(user_len).unwrap().to_le_bytes());
        bin.extend(vec![7; user_len]);
        bin.extend(b"kernel");
        bin
    }

    #[test]
    fn counts_the_user_elfs_segments() {
        assert_eq!(segment_count(&bin(b"hdr", 10)), Some(1));
        assert_eq!(segment_count(&bin(&[], MAX_SEGMENT_DATA_LEN)), Some(1));
        assert_eq!(segment_count(&bin(b"h", MAX_SEGMENT_DATA_LEN + 1)), Some(2));
        assert_eq!(segment_count(b"\x7fELF not a program binary"), None);
        let mut truncated = bin(b"hdr", 10);
        truncated.truncate(20);
        assert_eq!(segment_count(&truncated), None);
    }
}
