use super::{mmio::Device, order_device_accesses};

pub(super) const BUFFER_BYTES: usize = clv_abi::bridge::MAX_WIRE_BYTES;
pub(super) const NEXT: u16 = 1;
pub(super) const DEVICE_WRITES: u16 = 2;
const NO_INTERRUPT: u16 = 1;
const NO_NOTIFY: u16 = 1;

#[derive(Clone, Copy)]
#[repr(C, align(16))]
pub(super) struct Descriptor {
    address: u64,
    length: u32,
    flags: u16,
    next: u16,
}

impl Descriptor {
    pub(super) const fn new(address: usize, length: usize, flags: u16, next: u16) -> Self {
        Self {
            address: (address as u64).to_le(),
            length: (length as u32).to_le(),
            flags: flags.to_le(),
            next: next.to_le(),
        }
    }
}

#[repr(C)]
struct Available<const SIZE: usize> {
    flags: u16,
    index: u16,
    ring: [u16; SIZE],
    used_event: u16,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct UsedElement {
    id: u32,
    length: u32,
}

#[repr(C)]
struct Used<const SIZE: usize> {
    flags: u16,
    index: u16,
    ring: [UsedElement; SIZE],
    available_event: u16,
}

#[repr(C)]
pub(super) struct RingMemory<const SIZE: usize> {
    descriptors: [Descriptor; SIZE],
    available: Available<SIZE>,
    used: Used<SIZE>,
}

impl<const SIZE: usize> RingMemory<SIZE> {
    pub(super) const fn empty() -> Self {
        Self {
            descriptors: [Descriptor::new(0, 0, 0, 0); SIZE],
            available: Available {
                flags: NO_INTERRUPT.to_le(),
                index: 0,
                ring: [0; SIZE],
                used_event: 0,
            },
            used: Used {
                flags: 0,
                index: 0,
                ring: [UsedElement { id: 0, length: 0 }; SIZE],
                available_event: 0,
            },
        }
    }
}

pub(super) struct QueueError;

/// A split virtqueue with at most one descriptor chain in flight, always at head zero.
pub(super) struct Ring<const SIZE: usize> {
    memory: *mut RingMemory<SIZE>,
    index: u32,
    completed: u16,
    pending: Option<usize>,
}

impl<const SIZE: usize> Ring<SIZE> {
    /// # Safety
    /// The caller must own the queue memory and its identity mapping for the
    /// lifetime of the device's access, and the device must be in reset.
    pub(super) unsafe fn new(memory: *mut RingMemory<SIZE>, index: u32) -> Self {
        const { assert!(SIZE.is_power_of_two() && SIZE <= 32768) };
        // SAFETY: The reserved storage is idle and exclusively owned by the driver.
        unsafe { memory.write(RingMemory::empty()) };
        Self {
            memory,
            index,
            completed: 0,
            pending: None,
        }
    }

    pub(super) fn configure(&self, device: &Device) -> bool {
        // SAFETY: These fields are in resident, identity-mapped kernel memory.
        let (descriptor, available, used) = unsafe {
            (
                (&raw const (*self.memory).descriptors).addr() as u64,
                (&raw const (*self.memory).available).addr() as u64,
                (&raw const (*self.memory).used).addr() as u64,
            )
        };
        device.set_queue(self.index, SIZE as u32, descriptor, available, used)
    }

    /// # Safety
    /// The descriptors must form one valid chain starting at index zero. Its buffers
    /// must stay resident and device-owned until completion or device reset.
    pub(super) unsafe fn submit(&mut self, descriptors: &[Descriptor]) -> Result<(), QueueError> {
        if self.pending.is_some() || descriptors.is_empty() || descriptors.len() > SIZE {
            return Err(QueueError);
        }
        let capacity = descriptors
            .iter()
            .filter(|entry| u16::from_le(entry.flags) & DEVICE_WRITES != 0)
            .map(|entry| u32::from_le(entry.length) as usize)
            .sum();
        self.pending = Some(capacity);
        // SAFETY: The ring has space for the chain and all available heads are zero.
        unsafe {
            let table = (&raw mut (*self.memory).descriptors).cast::<Descriptor>();
            for (index, &descriptor) in descriptors.iter().enumerate() {
                table.add(index).write_volatile(descriptor);
            }
            order_device_accesses();
            (&raw mut (*self.memory).available.index)
                .write_volatile(self.completed.wrapping_add(1).to_le());
        }
        Ok(())
    }

    pub(super) fn notify(&self, device: &Device) {
        // Publish the available index before looking at notification suppression.
        order_device_accesses();
        // SAFETY: The device may update these flags, so read them with a volatile load.
        let flags = u16::from_le(unsafe { (&raw const (*self.memory).used.flags).read_volatile() });
        if flags & NO_NOTIFY == 0 {
            device.notify(self.index);
        }
    }

    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn complete(&mut self) -> Result<Option<usize>, QueueError> {
        // SAFETY: The used index is an aligned device-owned field.
        let index = u16::from_le(unsafe { (&raw const (*self.memory).used.index).read_volatile() });
        if index == self.completed {
            return Ok(None);
        }
        let capacity = self.pending.ok_or(QueueError)?;
        if index != self.completed.wrapping_add(1) {
            return Err(QueueError);
        }
        order_device_accesses();
        // SAFETY: The device published the index after writing the used entry.
        let element = unsafe {
            let ring = (&raw const (*self.memory).used.ring).cast::<UsedElement>();
            ring.add(self.completed as usize % SIZE).read_volatile()
        };
        let length = u32::from_le(element.length) as usize;
        if u32::from_le(element.id) != 0 || length > capacity {
            return Err(QueueError);
        }
        self.completed = index;
        self.pending = None;
        Ok(Some(length))
    }
}

#[repr(C)]
pub(super) struct QueueMemory {
    ring: RingMemory<1>,
    buffer: [u8; BUFFER_BYTES],
}

impl QueueMemory {
    pub(super) const fn empty() -> Self {
        Self {
            ring: RingMemory::empty(),
            buffer: [0; BUFFER_BYTES],
        }
    }
}

/// The console's single-buffer queue.
pub(super) struct Queue {
    ring: Ring<1>,
    buffer: *mut [u8; BUFFER_BYTES],
}

impl Queue {
    /// # Safety
    /// The caller must own this resident storage and have reset the device.
    pub(super) unsafe fn new(memory: *mut QueueMemory, index: u32) -> Self {
        // SAFETY: The driver has exclusive access until the queue is enabled.
        unsafe {
            let ring = Ring::new(&raw mut (*memory).ring, index);
            let buffer = &raw mut (*memory).buffer;
            buffer.write([0; BUFFER_BYTES]);
            Self { ring, buffer }
        }
    }

    pub(super) fn configure(&self, device: &Device) -> bool {
        self.ring.configure(device)
    }

    pub(super) fn offer_receive(&mut self) -> Result<(), QueueError> {
        self.submit(BUFFER_BYTES, DEVICE_WRITES)
    }

    pub(super) fn offer_send(&mut self, bytes: &[u8]) -> Result<(), QueueError> {
        if self.is_pending() || bytes.is_empty() || bytes.len() > BUFFER_BYTES {
            return Err(QueueError);
        }
        // SAFETY: The device has no outstanding descriptor for this buffer.
        unsafe {
            let buffer = self.buffer.cast::<u8>();
            for (offset, &byte) in bytes.iter().enumerate() {
                buffer.add(offset).write_volatile(byte);
            }
        }
        self.submit(bytes.len(), 0)
    }

    fn submit(&mut self, length: usize, flags: u16) -> Result<(), QueueError> {
        let descriptor = Descriptor::new(self.buffer.addr(), length, flags, 0);
        // SAFETY: The single descriptor fits the resident buffer and the ring tracks its ownership.
        unsafe { self.ring.submit(&[descriptor]) }
    }

    pub(super) fn notify(&self, device: &Device) {
        self.ring.notify(device);
    }

    pub(super) fn is_pending(&self) -> bool {
        self.ring.is_pending()
    }

    pub(super) fn complete(&mut self) -> Result<Option<usize>, QueueError> {
        self.ring.complete()
    }

    pub(super) fn receive(
        &mut self,
        bytes: &mut [u8; BUFFER_BYTES],
    ) -> Result<Option<usize>, QueueError> {
        let Some(length) = self.complete()? else {
            return Ok(None);
        };
        // SAFETY: Completion transfers the buffer back to the driver and validates length.
        unsafe {
            let buffer = self.buffer.cast::<u8>();
            for (offset, byte) in bytes[..length].iter_mut().enumerate() {
                *byte = buffer.add(offset).read_volatile();
            }
        }
        Ok(Some(length))
    }
}
