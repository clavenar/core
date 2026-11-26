use super::frame::{FRAME_SIZE, Frame, FrameAllocator};

pub(crate) const USER_TEXT_BASE: usize = 0x0040_0000;
pub(super) const MAX_USER_IMAGE_PAGES: usize = 7;
const MAX_USER_IMAGE_SIZE: usize = MAX_USER_IMAGE_PAGES * FRAME_SIZE;

const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
const ET_EXEC: u16 = 2;
const EM_RISCV: u16 = 243;
const PT_NULL: u32 = 0;
const PT_LOAD: u32 = 1;
const PT_RISCV_ATTRIBUTES: u32 = 0x7000_0003;
const PF_READ_EXECUTE: u32 = 5;

pub(crate) struct LoadedImage<'a> {
    pub(crate) entry: usize,
    pub(crate) pages: [Option<&'a Frame>; MAX_USER_IMAGE_PAGES],
}

/// Loads a static RV64 executable, allocating only the frames its segment uses.
pub(crate) fn load<'a>(image: &[u8], frames: &mut FrameAllocator<'a>) -> Option<LoadedImage<'a>> {
    let header = image.get(..ELF_HEADER_SIZE)?;
    // Magic, ELF64 class, little-endian data, and the identification version.
    if !header.starts_with(b"\x7fELF\x02\x01\x01")
        || u16_at(header, 16) != ET_EXEC
        || u16_at(header, 18) != EM_RISCV
        || u32_at(header, 20) != 1
        || usize::from(u16_at(header, 52)) != ELF_HEADER_SIZE
        || usize::from(u16_at(header, 54)) != PROGRAM_HEADER_SIZE
    {
        return None;
    }

    let entry = usize_at(header, 24);
    let table_start = usize_at(header, 32);
    let count = usize::from(u16_at(header, 56));
    if table_start < ELF_HEADER_SIZE {
        return None;
    }
    let table_end = table_start.checked_add(count * PROGRAM_HEADER_SIZE)?;
    let headers = image.get(table_start..table_end)?;
    let mut segment = None;
    for header in headers.chunks_exact(PROGRAM_HEADER_SIZE) {
        match u32_at(header, 0) {
            PT_NULL | PT_RISCV_ATTRIBUTES => {}
            PT_LOAD if segment.is_none() => segment = Some(load_segment(image, header)?),
            _ => return None,
        }
    }

    let (bytes, memory_size) = segment?;
    let entry_offset = entry.checked_sub(USER_TEXT_BASE)?;
    if entry % 2 != 0 || entry_offset.checked_add(2)? > bytes.len() {
        return None;
    }
    let mut pages = [None; MAX_USER_IMAGE_PAGES];
    let mut chunks = bytes.chunks(FRAME_SIZE);
    for page in pages.iter_mut().take(memory_size.div_ceil(FRAME_SIZE)) {
        let frame = frames.allocate_zeroed()?;
        if let Some(chunk) = chunks.next() {
            frame.bytes_mut()[..chunk.len()].copy_from_slice(chunk);
        }
        *page = Some(&*frame);
    }
    Some(LoadedImage { entry, pages })
}

fn load_segment<'a>(image: &'a [u8], header: &[u8]) -> Option<(&'a [u8], usize)> {
    let offset = usize_at(header, 8);
    let file_size = usize_at(header, 32);
    let memory_size = usize_at(header, 40);
    if u32_at(header, 4) != PF_READ_EXECUTE
        || usize_at(header, 16) != USER_TEXT_BASE
        || usize_at(header, 48) != FRAME_SIZE
        || offset % FRAME_SIZE != 0
        || file_size == 0
        || file_size > memory_size
        || memory_size > MAX_USER_IMAGE_SIZE
    {
        return None;
    }
    let bytes = image.get(offset..offset.checked_add(file_size)?)?;
    Some((bytes, memory_size))
}

// These helpers only read fixed offsets in a size-checked ELF or program header.
fn u16_at(header: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(header[offset..offset + 2].try_into().unwrap())
}

fn u32_at(header: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap())
}

fn usize_at(header: &[u8], offset: usize) -> usize {
    usize::from_le_bytes(header[offset..offset + 8].try_into().unwrap())
}
