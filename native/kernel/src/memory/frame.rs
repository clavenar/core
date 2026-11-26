use core::ptr;
use core::slice::IterMut;

use super::user_image::MAX_USER_IMAGE_PAGES;
use crate::programs::TASK_COUNT;

pub(crate) const FRAME_SIZE: usize = 4096;
// Three shared page tables. Each task needs three tables, a stack, and its image.
const FRAME_COUNT: usize = 3 + TASK_COUNT * (4 + MAX_USER_IMAGE_PAGES);
const WORDS_PER_FRAME: usize = FRAME_SIZE / size_of::<u64>();

#[repr(C, align(4096))]
pub(crate) struct Frame([u64; WORDS_PER_FRAME]);

impl Frame {
    // The boot map keeps frame addresses equal to their physical addresses.
    pub(crate) fn physical_address(&self) -> usize {
        ptr::from_ref(self).addr()
    }

    pub(crate) fn words_mut(&mut self) -> &mut [u64; WORDS_PER_FRAME] {
        &mut self.0
    }

    pub(crate) fn bytes_mut(&mut self) -> &mut [u8; FRAME_SIZE] {
        // SAFETY: Both arrays occupy one frame, and every byte pattern is valid.
        unsafe { &mut *ptr::from_mut(&mut self.0).cast() }
    }
}

// SAFETY: The linker places this zero-initialized array in the writable LOAD
// segment. The ELF loader supplies the zero-filled memory for the section.
#[unsafe(link_section = ".frame_pool")]
static mut BOOT_FRAMES: [Frame; FRAME_COUNT] = [const { Frame([0; WORDS_PER_FRAME]) }; FRAME_COUNT];

pub(crate) struct FrameAllocator<'a> {
    remaining: IterMut<'a, Frame>,
}

impl FrameAllocator<'static> {
    /// The boot hart must call this exactly once, with no other pool access.
    pub(crate) unsafe fn from_boot_pool() -> Self {
        // SAFETY: The caller gives this allocator exclusive access to the pool.
        let frames = unsafe { &mut *(&raw mut BOOT_FRAMES) };
        Self {
            remaining: frames.iter_mut(),
        }
    }
}

impl<'a> FrameAllocator<'a> {
    pub(crate) fn allocate_zeroed(&mut self) -> Option<&'a mut Frame> {
        let frame = self.remaining.next()?;
        frame.0.fill(0);
        Some(frame)
    }
}
