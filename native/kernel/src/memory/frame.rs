use core::ptr;
use core::slice::IterMut;

const FRAME_SIZE: usize = 4096;
const FRAME_COUNT: usize = 16;

#[repr(C, align(4096))]
pub(crate) struct Frame([u8; FRAME_SIZE]);

impl Frame {
    // Boot runs with address translation disabled.
    pub(crate) fn physical_address(&self) -> usize {
        ptr::from_ref(self).addr()
    }
}

// SAFETY: The linker places this zero-initialized array in the writable LOAD
// segment. The ELF loader supplies the zero-filled memory for the section.
#[unsafe(link_section = ".frame_pool")]
static mut BOOT_FRAMES: [Frame; FRAME_COUNT] = [const { Frame([0; FRAME_SIZE]) }; FRAME_COUNT];

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
